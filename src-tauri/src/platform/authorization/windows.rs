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
    progress(Stage::Registration);
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
    let mut process = process.map(Process::Windows);
    let result = async {
    {
    let wait = server.connect();
    tokio::pin!(wait);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(120);
    loop {
        tokio::select! {result=&mut wait=>{result.map_err(|e|e.to_string())?;break;},_ =tokio::time::sleep(Duration::from_millis(100))=>{
        if cancel.load(Ordering::Acquire){return Err("网络授权已取消".into());}
        if let Some(process) = &mut process {
            if process.exited()? {
            return Err("网络 helper 已退出，请确认其他连接已停止，再重试授权".into());
            }
        }
        if tokio::time::Instant::now()>=deadline{return Err("网络授权超时，请重试".into());}
        }}
    }
    }
    Ok::<_, String>(())
    }.await;
    if let Err(error) = result {
        drop(server);
        if let Some(process) = &mut process {
            process.finish().await;
        }
        return Err(error);
    }
    let mut pid = 0;
    progress(Stage::Authentication);
    let identity = if unsafe { GetNamedPipeClientProcessId(server.as_raw_handle(), &mut pid) } == 0
    {
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
        process,
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
