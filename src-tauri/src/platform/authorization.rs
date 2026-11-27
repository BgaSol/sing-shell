use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};
use tokio::io::{AsyncRead, AsyncWrite};
mod handshake;
#[cfg(any(test, target_os = "macos"))]
mod launchd;
#[cfg(target_os = "macos")]
mod macos;
pub trait Stream: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Stream for T {}
pub type Channel = Box<dyn Stream>;
pub struct WorkerPeer {
    pub channel: Channel,
    pub owner_key: String,
}
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
    Mac {
        token: Option<[u32; 8]>,
        launcher: std::process::Child,
    },
}
impl Connection {
    pub async fn finish(self) -> Result<(), String> {
        let Self {
            channel,
            mut process,
        } = self;
        drop(channel);
        if let Some(process) = &mut process {
            process.finish().await;
        }
        Ok(())
    }
}
impl Process {
    fn exited(&mut self) -> Result<bool, String> {
        match self {
            #[cfg(windows)]
            Process::Windows(handle) => {
                use std::os::windows::io::AsRawHandle;
                match unsafe {
                    windows_sys::Win32::System::Threading::WaitForSingleObject(
                        handle.as_raw_handle(),
                        0,
                    )
                } {
                    0 => Ok(true),
                    258 => Ok(false),
                    _ => Err(std::io::Error::last_os_error().to_string()),
                }
            }
            #[cfg(unix)]
            Process::Child(child) => child
                .try_wait()
                .map(|exit| exit.is_some())
                .map_err(|e| e.to_string()),
            #[cfg(target_os = "macos")]
            // An unconfirmed exit must retain the owner; callers have their own deadline.
            Process::Mac { token, launcher } => {
                let launcher_exited = launcher.try_wait().map_err(|e| e.to_string())?.is_some();
                Ok(launcher_exited
                    && match token {
                        Some(token) => macos::exited(token)?,
                        None => true,
                    })
            }
        }
    }
    async fn finish(&mut self) {
        // Unknown exit facts retain the same owner; the controller has its own deadline.
        while !self.exited().unwrap_or(false) {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
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
#[path = "authorization/windows.rs"]
mod native;
#[cfg(unix)]
#[path = "authorization/unix.rs"]
mod native;
#[cfg(not(test))]
pub use native::connect;
pub use native::{elevated, protected_root, worker};
#[cfg(test)]
pub async fn connect(
    _root: &Path,
    _cancel: &AtomicBool,
    _progress: &impl Fn(Stage),
) -> Result<Connection, String> {
    Err("单元测试禁止系统提权，请使用可丢弃运行器的网络集成夹具".into())
}
pub fn can_resume() -> bool {
    false
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

#[cfg(test)]
mod ownership_tests {
    use super::*;
    #[test]
    fn exit_wait_retains_owned_process_when_the_caller_times_out() {
        #[cfg(windows)]
        let child = std::process::Command::new("cmd")
            .args(["/C", "ping -n 3 127.0.0.1 > nul"])
            .spawn()
            .unwrap();
        #[cfg(unix)]
        let child = std::process::Command::new("/bin/sleep")
            .arg("1")
            .spawn()
            .unwrap();
        #[cfg(windows)]
        let mut process = {
            use std::os::windows::io::{FromRawHandle, IntoRawHandle, OwnedHandle};
            Process::Windows(unsafe { OwnedHandle::from_raw_handle(child.into_raw_handle()) })
        };
        #[cfg(unix)]
        let mut process = Process::Child(child);
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                assert!(
                    tokio::time::timeout(Duration::from_millis(10), process.finish())
                        .await
                        .is_err()
                );
                assert!(!process.exited().unwrap());
                process.finish().await;
                assert!(process.exited().unwrap());
            });
    }
}
