use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
#[derive(Clone, Copy)]
pub enum Kind {
    Tunnel,
    Resolver,
}
impl Kind {
    fn file(self) -> &'static str {
        match self {
            Self::Tunnel => "network-process.json",
            Self::Resolver => "preparation-process.json",
        }
    }
}
#[derive(Serialize, Deserialize)]
struct OwnedProcess {
    pid: u32,
    executable: PathBuf,
    birth: String,
}
pub fn record(root: &Path, kind: Kind, pid: u32, executable: &Path) -> Result<(), String> {
    crate::settings::write_json(
        &root.join(kind.file()),
        &OwnedProcess {
            pid,
            executable: executable.into(),
            birth: birth(pid)?,
        },
    )
}
pub fn forget(root: &Path, kind: Kind) -> Result<(), String> {
    match std::fs::remove_file(root.join(kind.file())) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}
fn executable(pid: u32) -> Result<PathBuf, String> {
    #[cfg(target_os = "linux")]
    {
        std::fs::read_link(format!("/proc/{pid}/exe")).map_err(|e| e.to_string())
    }
    #[cfg(target_os = "macos")]
    {
        extern "C" {
            fn proc_pidpath(pid: i32, buffer: *mut std::ffi::c_void, size: u32) -> i32;
        }
        let mut buffer = vec![0u8; 4096];
        let size =
            unsafe { proc_pidpath(pid as i32, buffer.as_mut_ptr().cast(), buffer.len() as u32) };
        if size <= 0 {
            return Err("无法核对旧核心程序路径".into());
        }
        let value = std::ffi::CStr::from_bytes_until_nul(&buffer).map_err(|e| e.to_string())?;
        use std::os::unix::ffi::OsStrExt;
        Ok(PathBuf::from(std::ffi::OsStr::from_bytes(value.to_bytes())))
    }
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::{
            Foundation::CloseHandle,
            System::Threading::{
                OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
            },
        };
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return Err(std::io::Error::last_os_error().to_string());
        }
        let mut buffer = vec![0u16; 32768];
        let mut size = buffer.len() as u32;
        let ok = QueryFullProcessImageNameW(handle, 0, buffer.as_mut_ptr(), &mut size);
        CloseHandle(handle);
        if ok == 0 {
            return Err("无法核对旧核心程序路径".into());
        }
        use std::os::windows::ffi::OsStringExt;
        Ok(PathBuf::from(std::ffi::OsString::from_wide(
            &buffer[..size as usize],
        )))
    }
}
#[cfg(windows)]
fn birth(pid: u32) -> Result<String, String> {
    use windows_sys::Win32::{
        Foundation::{CloseHandle, FILETIME},
        System::Threading::{GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION},
    };
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return Err(std::io::Error::last_os_error().to_string());
        }
        let mut created: FILETIME = std::mem::zeroed();
        let mut exit = created;
        let mut kernel = created;
        let mut user = created;
        let ok = GetProcessTimes(handle, &mut created, &mut exit, &mut kernel, &mut user);
        CloseHandle(handle);
        if ok == 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        if exit.dwHighDateTime != 0 || exit.dwLowDateTime != 0 {
            return Err("核心进程已经退出".into());
        }
        Ok(format!(
            "{}:{}",
            created.dwHighDateTime, created.dwLowDateTime
        ))
    }
}
#[cfg(target_os = "linux")]
fn birth(pid: u32) -> Result<String, String> {
    let value = std::fs::read_to_string(format!("/proc/{pid}/stat")).map_err(|e| e.to_string())?;
    value
        .rsplit_once(')')
        .and_then(|(_, parts)| parts.split_whitespace().nth(19))
        .map(str::to_owned)
        .ok_or("无法核对核心启动身份".into())
}
#[cfg(target_os = "macos")]
fn birth(pid: u32) -> Result<String, String> {
    let output = std::process::Command::new("/bin/ps")
        .args(["-o", "lstart=", "-p", &pid.to_string()])
        .output()
        .map_err(|e| e.to_string())?;
    let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if output.status.success() && !value.is_empty() {
        Ok(value)
    } else {
        Err("无法核对核心启动身份".into())
    }
}
pub fn recover(root: &Path, kind: Kind) -> Result<(), String> {
    let path = root.join(kind.file());
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.to_string()),
    };
    let record: OwnedProcess =
        serde_json::from_slice(&bytes).map_err(|_| "核心恢复记录损坏，已保留")?;
    if !record.executable.starts_with(root.join("engines/sing-box")) {
        return Err("核心恢复路径不属于授权目录".into());
    }
    let current = match birth(record.pid) {
        Ok(birth) => birth,
        Err(_) if gone(record.pid)? => return Ok(()),
        Err(error) => return Err(error),
    };
    if current != record.birth {
        return Ok(());
    }
    let expected = std::fs::canonicalize(&record.executable).map_err(|e| e.to_string())?;
    let actual = std::fs::canonicalize(executable(record.pid)?).map_err(|e| e.to_string())?;
    if actual != expected || birth(record.pid)? != record.birth {
        return Err("旧核心程序身份不符，不会终止它".into());
    }
    #[cfg(unix)]
    {
        if unsafe { libc::getpgid(record.pid as i32) } != record.pid as i32 {
            return Err("旧核心进程组不符，不会终止它".into());
        }
        unsafe {
            libc::kill(-(record.pid as i32), libc::SIGTERM);
        }
    }
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::{
            Foundation::CloseHandle,
            System::Threading::{OpenProcess, TerminateProcess, PROCESS_TERMINATE},
        };
        let handle = OpenProcess(PROCESS_TERMINATE, 0, record.pid);
        if handle.is_null() {
            return Err("无法停止旧授权核心".into());
        }
        let result = TerminateProcess(handle, 1);
        CloseHandle(handle);
        if result == 0 {
            return Err("旧核心未能停止".into());
        }
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !gone(record.pid)? && birth(record.pid)? == record.birth {
        if std::time::Instant::now() >= deadline {
            return Err("旧授权核心仍在退出，请重试".into());
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    Ok(())
}
fn gone(pid: u32) -> Result<bool, String> {
    #[cfg(unix)]
    {
        if pid == 0 || pid > i32::MAX as u32 {
            return Ok(true);
        }
        if unsafe { libc::kill(pid as i32, 0) } == 0 {
            return Ok(false);
        }
        let error = std::io::Error::last_os_error();
        match error.raw_os_error() {
            Some(libc::ESRCH) => Ok(true),
            Some(libc::EPERM) => Ok(false),
            _ => Err(error.to_string()),
        }
    }
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::{
            Foundation::CloseHandle,
            System::Threading::{
                GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
            },
        };
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            let error = std::io::Error::last_os_error();
            return if error.raw_os_error() == Some(87) {
                Ok(true)
            } else {
                Err(error.to_string())
            };
        }
        let mut exit = 259;
        let ok = GetExitCodeProcess(handle, &mut exit);
        let error = std::io::Error::last_os_error();
        CloseHandle(handle);
        if ok == 0 {
            Err(error.to_string())
        } else {
            Ok(exit != 259)
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn exited_process_is_not_a_live_recovery_target_even_with_an_open_child_handle() {
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .arg("--help")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        child.wait().unwrap();
        assert!(super::birth(child.id()).is_err());
    }
    #[test]
    fn process_identity_matches_current_executable() {
        let pid = std::process::id();
        let first = super::birth(pid).unwrap();
        assert!(!first.is_empty());
        assert_eq!(first, super::birth(pid).unwrap());
        assert_eq!(
            std::fs::canonicalize(super::executable(pid).unwrap()).unwrap(),
            std::fs::canonicalize(std::env::current_exe().unwrap()).unwrap()
        );
        assert!(super::birth(u32::MAX).is_err());
    }
}
