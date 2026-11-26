use serde::{Deserialize, Serialize};
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
