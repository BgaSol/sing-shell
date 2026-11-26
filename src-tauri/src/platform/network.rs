use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
#[derive(Serialize, Deserialize)]
struct OwnedProcess {
    pid: u32,
    executable: PathBuf,
    birth: String,
}
pub fn record(root: &Path, pid: u32, executable: &Path) -> Result<(), String> {
    crate::settings::write_json(
        &root.join("network-process.json"),
        &OwnedProcess {
            pid,
            executable: executable.into(),
            birth: birth(pid)?,
        },
    )
}
pub fn forget(root: &Path) -> Result<(), String> {
    match std::fs::remove_file(root.join("network-process.json")) {
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
pub fn recover(root: &Path) -> Result<(), String> {
    let path = root.join("network-process.json");
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
        Err(_) => {
            require_clear()?;
            return forget(root);
        }
    };
    if current != record.birth {
        require_clear()?;
        return forget(root);
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
    while birth(record.pid).is_ok_and(|b| b == record.birth) {
        if std::time::Instant::now() >= deadline {
            return Err("旧授权核心仍在退出，请重试".into());
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    require_clear()?;
    forget(root)
}
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Observation {
    pub interface: bool,
    pub ipv4: bool,
    pub ipv6: bool,
    pub routes: bool,
}
impl Observation {
    pub fn ready(&self) -> bool {
        self.interface && self.ipv4 && self.ipv6 && self.routes
    }
}
pub fn interface_name() -> &'static str {
    if cfg!(target_os = "macos") {
        "utun223"
    } else {
        "sing-shell"
    }
}

#[cfg(windows)]
pub fn observe() -> Result<Observation, String> {
    use windows_sys::Win32::NetworkManagement::IpHelper::{
        FreeMibTable, GetAdaptersAddresses, GetIpForwardTable2, IP_ADAPTER_ADDRESSES_LH,
        MIB_IPFORWARD_TABLE2,
    };
    use windows_sys::Win32::Networking::WinSock::{AF_INET, AF_INET6, AF_UNSPEC};
    unsafe {
        let mut bytes = 0;
        GetAdaptersAddresses(
            AF_UNSPEC as u32,
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut bytes,
        );
        let mut buffer = vec![0u64; (bytes as usize).div_ceil(8)];
        let first = buffer.as_mut_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>();
        let code =
            GetAdaptersAddresses(AF_UNSPEC as u32, 0, std::ptr::null_mut(), first, &mut bytes);
        if code != 0 {
            return Err(std::io::Error::from_raw_os_error(code as i32).to_string());
        }
        let mut next = first;
        let mut result = Observation::default();
        let mut index = 0;
        while let Some(adapter) = next.as_ref() {
            let name = adapter.FriendlyName;
            if !name.is_null() {
                let mut len = 0;
                while *name.add(len) != 0 && len < 256 {
                    len += 1;
                }
                if String::from_utf16_lossy(std::slice::from_raw_parts(name, len))
                    == interface_name()
                {
                    result.interface = true;
                    index = adapter.Anonymous1.Anonymous.IfIndex;
                    let mut address = adapter.FirstUnicastAddress;
                    while let Some(a) = address.as_ref() {
                        if !a.Address.lpSockaddr.is_null() {
                            match (*a.Address.lpSockaddr).sa_family {
                                AF_INET => result.ipv4 = true,
                                AF_INET6 => result.ipv6 = true,
                                _ => {}
                            }
                        }
                        address = a.Next;
                    }
                }
            }
            next = adapter.Next;
        }
        if result.interface {
            let mut table: *mut MIB_IPFORWARD_TABLE2 = std::ptr::null_mut();
            if GetIpForwardTable2(AF_UNSPEC, &mut table) == 0 && !table.is_null() {
                let routes = std::slice::from_raw_parts(
                    (*table).Table.as_ptr(),
                    (*table).NumEntries as usize,
                );
                let mut v4 = false;
                let mut v6 = false;
                for route in routes
                    .iter()
                    .filter(|r| r.InterfaceIndex == index && r.DestinationPrefix.PrefixLength <= 1)
                {
                    match route.DestinationPrefix.Prefix.si_family {
                        AF_INET => v4 = true,
                        AF_INET6 => v6 = true,
                        _ => {}
                    }
                }
                result.routes = v4 && v6;
                FreeMibTable(table.cast());
            }
        }
        Ok(result)
    }
}
#[cfg(unix)]
pub fn observe() -> Result<Observation, String> {
    let mut result = Observation::default();
    unsafe {
        let mut first = std::ptr::null_mut();
        if libc::getifaddrs(&mut first) != 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        let mut next = first;
        while let Some(item) = next.as_ref() {
            if !item.ifa_name.is_null()
                && std::ffi::CStr::from_ptr(item.ifa_name).to_bytes() == interface_name().as_bytes()
            {
                result.interface = true;
                if !item.ifa_addr.is_null() {
                    match (*item.ifa_addr).sa_family as i32 {
                        libc::AF_INET => result.ipv4 = true,
                        libc::AF_INET6 => result.ipv6 = true,
                        _ => {}
                    }
                }
            }
            next = item.ifa_next;
        }
        libc::freeifaddrs(first);
    }
    if result.interface {
        #[cfg(target_os = "linux")]
        {
            let ip = if std::path::Path::new("/usr/sbin/ip").is_file() {
                "/usr/sbin/ip"
            } else {
                "/sbin/ip"
            };
            let routes = |family: &str| -> Result<bool, String> {
                let output = std::process::Command::new(ip)
                    .args(["-j", family, "route", "show", "table", "all"])
                    .output()
                    .map_err(|e| format!("无法读取路由，请安装 iproute2：{e}"))?;
                if !output.status.success() {
                    return Err("无法读取 TUN 路由表".into());
                }
                let routes: Vec<serde_json::Value> =
                    serde_json::from_slice(&output.stdout).map_err(|_| "系统路由输出无效")?;
                Ok(routes.iter().any(|r| r["dev"] == interface_name()))
            };
            result.routes = routes("-4")? && routes("-6")?;
        }
        #[cfg(target_os = "macos")]
        {
            let routes = |family: &str| -> Result<bool, String> {
                let output = std::process::Command::new("/usr/sbin/netstat")
                    .args(["-rn", "-f", family])
                    .output()
                    .map_err(|e| e.to_string())?;
                if !output.status.success() {
                    return Err("无法读取 TUN 路由状态".into());
                }
                Ok(String::from_utf8_lossy(&output.stdout).lines().any(|line| {
                    line.split_whitespace().any(|part| part == interface_name())
                        && line.split_whitespace().next().is_some_and(|destination| {
                            destination == "default" || destination.ends_with("/1")
                        })
                }))
            };
            result.routes = routes("inet")? && routes("inet6")?;
        }
    }
    Ok(result)
}

pub fn require_clear() -> Result<(), String> {
    if observe()?.interface {
        Err("Sing Shell TUN 接口仍存在，请先完成清理；不会覆盖现有网络接口".into())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
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
