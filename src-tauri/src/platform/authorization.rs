use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};
use tokio::io::{AsyncRead, AsyncWrite};
mod handshake;
#[cfg(target_os = "macos")]
mod macos;
pub trait Stream: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Stream for T {}
pub type Channel = Box<dyn Stream>;
#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Stage {
    Registration,
    Activation,
    Authentication,
    Protocol,
}
pub struct Connection {
    pub channel: Channel,
    process: Option<Process>,
}
enum Process {
    #[cfg(windows)]
    Windows(std::os::windows::io::OwnedHandle),
    #[cfg(unix)]
    Child(std::process::Child),
    #[cfg(target_os = "macos")]
    Mac([u32; 8]),
}
impl Connection {
    // Keep the controller thread (and its owner) alive until the helper really exits.
    pub async fn finish(self) -> Result<(), String> {
        let Self {
            channel,
            mut process,
        } = self;
        drop(channel);
        while let Some(process) = &mut process {
            let exited = match process {
                #[cfg(windows)]
                Process::Windows(handle) => {
                    use std::os::windows::io::AsRawHandle;
                    let mut code = 259;
                    if unsafe {
                        windows_sys::Win32::System::Threading::GetExitCodeProcess(
                            handle.as_raw_handle(),
                            &mut code,
                        )
                    } == 0
                    {
                        return Err(std::io::Error::last_os_error().to_string());
                    }
                    code != 259
                }
                #[cfg(unix)]
                Process::Child(child) => child.try_wait().map_err(|e| e.to_string())?.is_some(),
                #[cfg(target_os = "macos")]
                Process::Mac(token) => macos::exited(token)?,
            };
            if exited {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        Ok(())
    }
}

pub fn nonce() -> Result<String, String> {
    let mut bytes = [0u8; 24];
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::Security::Cryptography::{
            BCryptGenRandom, BCRYPT_USE_SYSTEM_PREFERRED_RNG,
        };
        if BCryptGenRandom(
            std::ptr::null_mut(),
            bytes.as_mut_ptr(),
            bytes.len() as u32,
            BCRYPT_USE_SYSTEM_PREFERRED_RNG,
        ) < 0
        {
            return Err("无法生成网络会话标识".into());
        }
    }
    #[cfg(unix)]
    {
        use std::io::Read;
        std::fs::File::open("/dev/urandom")
            .and_then(|mut f| f.read_exact(&mut bytes))
            .map_err(|e| e.to_string())?;
    }
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}
fn same_executable(path: &Path) -> Result<(), String> {
    let current = std::env::current_exe().map_err(|e| e.to_string())?;
    if crate::core::package::hash(&current)? != crate::core::package::hash(path)? {
        return Err("网络控制端与已授权程序身份不符".into());
    }
    Ok(())
}

#[cfg(windows)]
mod native {
    use super::*;
    use std::os::windows::{
        ffi::OsStrExt,
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
    };
    use windows_sys::Win32::{
        Foundation::CloseHandle,
        Security::{GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY},
        System::{
            Pipes::{GetNamedPipeClientProcessId, GetNamedPipeServerProcessId},
            Threading::{
                GetCurrentProcess, OpenProcess, OpenProcessToken, QueryFullProcessImageNameW,
                PROCESS_QUERY_LIMITED_INFORMATION,
            },
        },
        UI::{
            Shell::{ShellExecuteExW, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW},
            WindowsAndMessaging::SW_HIDE,
        },
    };
    fn process_path(pid: u32, require_elevated: bool) -> Result<PathBuf, String> {
        unsafe {
            let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if process.is_null() {
                return Err(std::io::Error::last_os_error().to_string());
            }
            let result = (|| {
                if require_elevated {
                    let mut token = std::ptr::null_mut();
                    if OpenProcessToken(process, TOKEN_QUERY, &mut token) == 0 {
                        return Err(std::io::Error::last_os_error().to_string());
                    }
                    let mut elevation: TOKEN_ELEVATION = std::mem::zeroed();
                    let mut size = 0;
                    let ok = GetTokenInformation(
                        token,
                        TokenElevation,
                        (&mut elevation as *mut TOKEN_ELEVATION).cast(),
                        std::mem::size_of_val(&elevation) as u32,
                        &mut size,
                    );
                    CloseHandle(token);
                    if ok == 0 || elevation.TokenIsElevated == 0 {
                        return Err("网络进程没有管理员授权".into());
                    }
                }
                let mut path = vec![0u16; 32768];
                let mut size = path.len() as u32;
                if QueryFullProcessImageNameW(process, 0, path.as_mut_ptr(), &mut size) == 0 {
                    return Err(std::io::Error::last_os_error().to_string());
                }
                Ok(PathBuf::from(String::from_utf16_lossy(
                    &path[..size as usize],
                )))
            })();
            CloseHandle(process);
            result
        }
    }
    fn wide(s: impl AsRef<std::ffi::OsStr>) -> Vec<u16> {
        s.as_ref().encode_wide().chain(Some(0)).collect()
    }
    pub fn elevated() -> bool {
        unsafe {
            let mut token = std::ptr::null_mut();
            if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
                return false;
            }
            let mut elevation: TOKEN_ELEVATION = std::mem::zeroed();
            let mut size = 0;
            let result = GetTokenInformation(
                token,
                TokenElevation,
                (&mut elevation as *mut TOKEN_ELEVATION).cast(),
                std::mem::size_of_val(&elevation) as u32,
                &mut size,
            ) != 0
                && elevation.TokenIsElevated != 0;
            CloseHandle(token);
            result
        }
    }
    pub async fn connect(
        _root: &Path,
        cancel: &AtomicBool,
        progress: &impl Fn(Stage),
    ) -> Result<Connection, String> {
        progress(Stage::Activation);
        let pipe = PathBuf::from(format!("\\\\.\\pipe\\sing-shell-network-{}", nonce()?));
        let server = crate::instance::network_pipe(&pipe)?;
        let executable = wide(std::env::current_exe().map_err(|e| e.to_string())?);
        let parameters = wide(format!(
            "--network-worker {} {}",
            pipe.display(),
            std::process::id()
        ));
        let verb = wide(if elevated() { "open" } else { "runas" });
        let mut info: SHELLEXECUTEINFOW = unsafe { std::mem::zeroed() };
        info.cbSize = std::mem::size_of_val(&info) as u32;
        info.fMask = SEE_MASK_NOCLOSEPROCESS;
        info.lpVerb = verb.as_ptr();
        info.lpFile = executable.as_ptr();
        info.lpParameters = parameters.as_ptr();
        info.nShow = SW_HIDE;
        if unsafe { ShellExecuteExW(&mut info) } == 0 {
            return Err(format!(
                "管理员授权未完成：{}",
                std::io::Error::last_os_error()
            ));
        }
        let process = if info.hProcess.is_null() {
            None
        } else {
            Some(unsafe { OwnedHandle::from_raw_handle(info.hProcess) })
        };
        {
            let wait = server.connect();
            tokio::pin!(wait);
            let deadline = tokio::time::Instant::now() + Duration::from_secs(120);
            loop {
                tokio::select! {result=&mut wait=>{result.map_err(|e|e.to_string())?;break;},_ =tokio::time::sleep(Duration::from_millis(100))=>{
                    if cancel.load(Ordering::Acquire){return Err("网络授权已取消".into());}
                    if let Some(process) = &process {
                        let mut code = 259;
                        if unsafe {windows_sys::Win32::System::Threading::GetExitCodeProcess(process.as_raw_handle(), &mut code)} != 0 && code != 259 {
                            return Err("网络 helper 已退出，请确认其他连接已停止，再重试授权".into());
                        }
                    }
                    if tokio::time::Instant::now()>=deadline{return Err("网络授权超时，请重试".into());}
                }}
            }
        }
        let mut pid = 0;
        progress(Stage::Authentication);
        let identity =
            if unsafe { GetNamedPipeClientProcessId(server.as_raw_handle(), &mut pid) } == 0 {
                Err("无法核对网络进程身份".into())
            } else {
                process_path(pid, true).and_then(|path| same_executable(&path))
            };
        let mut server = server;
        let result = if let Err(error) = identity {
            Err(error)
        } else {
            handshake::bounded(
                cancel,
                handshake::ACTIVATION,
                handshake::client(&mut server, |_| {
                    progress(Stage::Protocol);
                    Ok(())
                }),
            )
            .await
        };
        let connection = Connection {
            channel: Box::new(server),
            process: process.map(Process::Windows),
        };
        if let Err(error) = result {
            connection.finish().await?;
            return Err(error);
        }
        Ok(connection)
    }
    pub async fn worker(endpoint: &Path, parent: u32) -> Result<Channel, String> {
        if !elevated() {
            return Err("网络进程必须由系统授权启动".into());
        }
        if !endpoint
            .to_string_lossy()
            .starts_with("\\\\.\\pipe\\sing-shell-network-")
        {
            return Err("网络管道标识无效".into());
        }
        let client = tokio::net::windows::named_pipe::ClientOptions::new()
            .open(endpoint)
            .map_err(|e| e.to_string())?;
        let mut pid = 0;
        if unsafe { GetNamedPipeServerProcessId(client.as_raw_handle(), &mut pid) } == 0
            || pid != parent
        {
            return Err("网络控制端身份无效".into());
        }
        same_executable(&process_path(pid, false)?)?;
        let mut client = client;
        handshake::bounded(
            &AtomicBool::new(false),
            Duration::from_secs(3),
            handshake::server(&mut client),
        )
        .await?;
        Ok(Box::new(client))
    }
    pub fn protected_root() -> Result<PathBuf, String> {
        let mut path = vec![0u16; 32768];
        let result = unsafe {
            windows_sys::Win32::UI::Shell::SHGetFolderPathW(
                std::ptr::null_mut(),
                windows_sys::Win32::UI::Shell::CSIDL_COMMON_APPDATA as i32,
                std::ptr::null_mut(),
                0,
                path.as_mut_ptr(),
            )
        };
        if result < 0 {
            return Err("无法获取受保护程序数据目录".into());
        }
        let length = path
            .iter()
            .position(|v| *v == 0)
            .ok_or("程序数据目录过长")?;
        Ok(PathBuf::from(String::from_utf16_lossy(&path[..length])).join("Sing Shell Network"))
    }
}
#[cfg(unix)]
mod native {
    use super::*;
    use std::os::unix::fs::MetadataExt;
    pub fn elevated() -> bool {
        unsafe { libc::geteuid() == 0 }
    }
    fn verify(
        stream: &tokio::net::UnixStream,
        expected_pid: Option<u32>,
        root: bool,
    ) -> Result<(), String> {
        let credential = stream.peer_cred().map_err(|e| e.to_string())?;
        if root && credential.uid() != 0 {
            return Err("网络进程不属于系统管理员".into());
        }
        #[cfg(target_os = "linux")]
        let pid = credential
            .pid()
            .map(|p| p as u32)
            .ok_or("无法核对网络控制端进程")?;
        #[cfg(target_os = "macos")]
        let pid = {
            use std::os::fd::AsRawFd;
            let mut pid: libc::pid_t = 0;
            let mut length = std::mem::size_of_val(&pid) as libc::socklen_t;
            if unsafe {
                libc::getsockopt(
                    stream.as_raw_fd(),
                    libc::SOL_LOCAL,
                    libc::LOCAL_PEERPID,
                    (&mut pid as *mut libc::pid_t).cast(),
                    &mut length,
                )
            } != 0
            {
                return Err(std::io::Error::last_os_error().to_string());
            }
            pid as u32
        };
        if expected_pid.is_some_and(|expected| expected != pid) {
            return Err("网络控制端进程已改变".into());
        }
        #[cfg(target_os = "linux")]
        let path = std::fs::read_link(format!("/proc/{pid}/exe")).map_err(|e| e.to_string())?;
        #[cfg(target_os = "macos")]
        return super::macos::verify(stream, pid, root);
        #[cfg(target_os = "linux")]
        same_executable(&path)
    }
    pub async fn direct_connect(
        root: &Path,
        cancel: &AtomicBool,
        progress: &impl Fn(Stage),
    ) -> Result<Connection, String> {
        progress(Stage::Activation);
        let _ = root;
        use std::os::unix::fs::DirBuilderExt;
        let directory = PathBuf::from("/tmp").join(format!(
            "ssn-{}-{}",
            unsafe { libc::geteuid() },
            &nonce()?[..16]
        ));
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&directory)
            .map_err(|e| e.to_string())?;
        let endpoint = directory.join("control.sock");
        let listener = tokio::net::UnixListener::bind(&endpoint).map_err(|e| e.to_string())?;
        struct Cleanup(PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = std::fs::remove_file(&self.0);
                if let Some(parent) = self.0.parent() {
                    let _ = std::fs::remove_dir(parent);
                }
            }
        }
        let _cleanup = Cleanup(endpoint.clone());
        let executable = std::env::current_exe().map_err(|e| e.to_string())?;
        let mut command = if elevated() {
            std::process::Command::new(&executable)
        } else {
            let mut command = std::process::Command::new("/usr/bin/pkexec");
            command.arg(&executable);
            command
        };
        let mut child = command
            .arg("--network-worker")
            .arg(&endpoint)
            .arg(std::process::id().to_string())
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| format!("无法请求系统授权，请安装并启用 polkit：{e}"))?;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(120);
        let stream = loop {
            tokio::select! {result=listener.accept()=>{break result.map_err(|e|e.to_string())?.0},_=tokio::time::sleep(Duration::from_millis(100))=>{if cancel.load(Ordering::Acquire){let _=child.kill();let _=child.wait();return Err("网络授权已取消".into());}if child.try_wait().map_err(|e|e.to_string())?.is_some(){return Err("系统未允许网络授权，请重试".into());}if tokio::time::Instant::now()>=deadline{let _=child.kill();let _=child.wait();return Err("网络授权超时，请重试".into());}}}
        };
        let mut stream = stream;
        let result = handshake::bounded(
            cancel,
            handshake::ACTIVATION,
            handshake::client(&mut stream, |s| {
                progress(Stage::Authentication);
                verify(s, None, true)?;
                progress(Stage::Protocol);
                Ok(())
            }),
        )
        .await;
        let connection = Connection {
            channel: Box::new(stream),
            process: Some(Process::Child(child)),
        };
        if let Err(error) = result {
            connection.finish().await?;
            return Err(error);
        }
        Ok(connection)
    }
    #[cfg(target_os = "linux")]
    pub async fn connect(
        root: &Path,
        cancel: &AtomicBool,
        progress: &impl Fn(Stage),
    ) -> Result<Connection, String> {
        direct_connect(root, cancel, progress).await
    }
    #[cfg(target_os = "macos")]
    pub async fn connect(
        _root: &Path,
        cancel: &AtomicBool,
        progress: &impl Fn(Stage),
    ) -> Result<Connection, String> {
        if elevated() {
            if std::env::var("SING_SHELL_NETWORK_PREFLIGHT").as_deref() == Ok("1") {
                return direct_connect(_root, cancel, progress).await;
            }
            return Err("请以普通用户运行 Sing Shell，TUN 将单独请求系统授权".into());
        }
        progress(Stage::Registration);
        super::macos::ensure_registered()?;
        if cancel.load(Ordering::Acquire) {
            return Err("网络授权已取消".into());
        }
        progress(Stage::Activation);
        let mut stream = tokio::net::UnixStream::connect(super::macos::SOCKET)
            .await
            .map_err(|e| format!("网络授权进程尚未就绪：{e}"))?;
        handshake::bounded(
            cancel,
            handshake::ACTIVATION,
            handshake::client(&mut stream, |s| {
                progress(Stage::Authentication);
                verify(s, None, true)?;
                progress(Stage::Protocol);
                Ok(())
            }),
        )
        .await?;
        let process = super::macos::token(&stream)?;
        Ok(Connection {
            channel: Box::new(stream),
            process: Some(Process::Mac(process)),
        })
    }
    pub async fn worker(endpoint: &Path, parent: u32) -> Result<Channel, String> {
        if !elevated() {
            return Err("网络进程必须由系统授权启动".into());
        }
        let owner = std::fs::metadata(endpoint.parent().ok_or("网络端点无父目录")?)
            .map_err(|e| e.to_string())?;
        if owner.mode() & 0o077 != 0 {
            return Err("网络端点目录权限无效".into());
        }
        let mut stream = tokio::net::UnixStream::connect(endpoint)
            .await
            .map_err(|e| e.to_string())?;
        verify(&stream, Some(parent), false)?;
        handshake::bounded(
            &AtomicBool::new(false),
            Duration::from_secs(3),
            handshake::server(&mut stream),
        )
        .await?;
        Ok(Box::new(stream))
    }
    #[cfg(target_os = "macos")]
    pub async fn daemon() -> Result<Channel, String> {
        use std::os::fd::FromRawFd;
        extern "C" {
            fn launch_activate_socket(
                name: *const std::ffi::c_char,
                fds: *mut *mut i32,
                count: *mut usize,
            ) -> i32;
        }
        let mut fds = std::ptr::null_mut();
        let mut count = 0;
        let status = unsafe { launch_activate_socket(c"Control".as_ptr(), &mut fds, &mut count) };
        if status != 0 || count != 1 || fds.is_null() {
            return Err("网络 helper 缺少系统控制端点".into());
        }
        let listener = unsafe { std::os::unix::net::UnixListener::from_raw_fd(*fds) };
        unsafe { libc::free(fds.cast()) };
        listener.set_nonblocking(true).map_err(|e| e.to_string())?;
        let listener = tokio::net::UnixListener::from_std(listener).map_err(|e| e.to_string())?;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
        loop {
            let (mut stream, _) = tokio::time::timeout_at(deadline, listener.accept())
                .await
                .map_err(|_| "网络授权进程等待超时")?
                .map_err(|e| e.to_string())?;
            if verify(&stream, None, false).is_err() {
                continue;
            }
            let result = tokio::time::timeout_at(
                deadline,
                handshake::bounded(
                    &AtomicBool::new(false),
                    Duration::from_secs(3),
                    handshake::server(&mut stream),
                ),
            )
            .await;
            if matches!(result, Ok(Ok(()))) {
                return Ok(Box::new(stream));
            }
        }
    }
    pub fn protected_root() -> Result<PathBuf, String> {
        Ok(PathBuf::from(if cfg!(target_os = "macos") {
            "/Library/Application Support/Sing Shell Network"
        } else {
            "/var/lib/sing-shell-network"
        }))
    }
}
#[cfg(target_os = "macos")]
pub use native::daemon;
pub use native::{connect, elevated, protected_root, worker};
pub fn can_resume() -> bool {
    #[cfg(target_os = "macos")]
    {
        macos::already_allowed()
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}
pub fn open_settings() -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        macos::open_settings()
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err("请在连接时完成系统网络授权；Linux 需要 polkit 授权代理".into())
    }
}
pub fn prepare_root() -> Result<PathBuf, String> {
    let root = protected_root()?;
    if let Some(parent) = root.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let created = match std::fs::create_dir(&root) {
        Ok(()) => true,
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => false,
        Err(e) => return Err(e.to_string()),
    };
    let metadata = std::fs::symlink_metadata(&root).map_err(|e| e.to_string())?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err("授权核心目录不安全".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.uid() != 0 || !created && metadata.mode() & 0o077 != 0 {
            return Err("授权核心目录必须归 root 所有且仅 root 可访问".into());
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::{ffi::OsStrExt, fs::MetadataExt};
        use windows_sys::Win32::{
            Foundation::LocalFree,
            Security::{
                Authorization::{
                    ConvertSidToStringSidW, ConvertStringSidToSidW, GetNamedSecurityInfoW,
                    SetNamedSecurityInfoW, SE_FILE_OBJECT,
                },
                OWNER_SECURITY_INFORMATION,
            },
        };
        if metadata.file_attributes() & 0x400 != 0 {
            return Err("授权核心目录不能是重解析点".into());
        }
        let path: Vec<u16> = root.as_os_str().encode_wide().chain(Some(0)).collect();
        unsafe {
            if created {
                let admin: Vec<u16> = "S-1-5-32-544".encode_utf16().chain(Some(0)).collect();
                let mut sid = std::ptr::null_mut();
                if ConvertStringSidToSidW(admin.as_ptr(), &mut sid) == 0 {
                    return Err(std::io::Error::last_os_error().to_string());
                }
                let status = SetNamedSecurityInfoW(
                    path.as_ptr(),
                    SE_FILE_OBJECT,
                    OWNER_SECURITY_INFORMATION,
                    sid,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                );
                LocalFree(sid);
                if status != 0 {
                    return Err("无法设置授权核心目录的管理员所有权".into());
                }
            }
            let mut owner = std::ptr::null_mut();
            let mut security = std::ptr::null_mut();
            let status = GetNamedSecurityInfoW(
                path.as_ptr(),
                SE_FILE_OBJECT,
                OWNER_SECURITY_INFORMATION,
                &mut owner,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut security,
            );
            if status != 0 {
                return Err("无法核对授权核心目录所有者".into());
            }
            let mut sid_text = std::ptr::null_mut();
            let ok = ConvertSidToStringSidW(owner, &mut sid_text);
            if ok == 0 {
                LocalFree(security);
                return Err("授权目录所有者无效".into());
            }
            let mut len = 0;
            while *sid_text.add(len) != 0 {
                len += 1;
            }
            let sid = String::from_utf16_lossy(std::slice::from_raw_parts(sid_text, len));
            LocalFree(sid_text.cast());
            LocalFree(security);
            if sid != "S-1-5-32-544" && sid != "S-1-5-18" {
                return Err("授权核心目录已被普通用户占用，请移除后重试".into());
            }
        }
    }
    crate::private_files::directory(&root)?;
    Ok(root)
}
