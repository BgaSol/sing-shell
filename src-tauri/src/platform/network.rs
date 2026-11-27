mod routes;
use super::process::record::{self, Kind};
use serde::{Deserialize, Serialize};
use std::path::Path;
pub fn record(root: &Path, pid: u32, executable: &Path) -> Result<(), String> {
    record::record(root, Kind::Tunnel, pid, executable)
}
pub fn forget(root: &Path) -> Result<(), String> {
    record::forget(root, Kind::Tunnel)
}
pub fn recover(root: &Path) -> Result<(), String> {
    record::recover(root, Kind::Tunnel)?;
    clear_record(root)
}
fn clear_record(root: &Path) -> Result<(), String> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        match require_clear() {
            Ok(()) => return forget(root),
            Err(e) if std::time::Instant::now() >= deadline => return Err(e),
            Err(_) => std::thread::sleep(std::time::Duration::from_millis(100)),
        }
    }
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
pub fn observe() -> Result<Observation, String> {
    observe_expected(&[])
}
pub fn interface_name() -> &'static str {
    crate::profile::tun::INTERFACE
}

#[cfg(windows)]
pub fn observe_expected(excluded: &[String]) -> Result<Observation, String> {
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
                                AF_INET => {
                                    let address = &*(a.Address.lpSockaddr as *const windows_sys::Win32::Networking::WinSock::SOCKADDR_IN);
                                    result.ipv4 |= std::net::Ipv4Addr::from(
                                        address.sin_addr.S_un.S_addr.to_ne_bytes(),
                                    )
                                    .to_string()
                                        == crate::profile::tun::ADDRESS[0]
                                            .split('/')
                                            .next()
                                            .unwrap();
                                }
                                AF_INET6 => {
                                    let address = &*(a.Address.lpSockaddr as *const windows_sys::Win32::Networking::WinSock::SOCKADDR_IN6);
                                    result.ipv6 |=
                                        std::net::Ipv6Addr::from(address.sin6_addr.u.Byte)
                                            .to_string()
                                            == crate::profile::tun::ADDRESS[1]
                                                .split('/')
                                                .next()
                                                .unwrap();
                                }
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
                let mut v4 = Vec::new();
                let mut v6 = Vec::new();
                for route in routes.iter().filter(|r| r.InterfaceIndex == index) {
                    let prefix = route.DestinationPrefix.PrefixLength;
                    let address = route.DestinationPrefix.Prefix;
                    match address.si_family {
                        AF_INET => v4.push(format!(
                            "{}/{}",
                            std::net::Ipv4Addr::from(
                                address.Ipv4.sin_addr.S_un.S_addr.to_ne_bytes()
                            ),
                            prefix
                        )),
                        AF_INET6 => v6.push(format!(
                            "{}/{}",
                            std::net::Ipv6Addr::from(address.Ipv6.sin6_addr.u.Byte),
                            prefix
                        )),
                        _ => {}
                    }
                }
                result.routes = routes::matches(v4.iter().map(String::as_str), excluded, false)
                    && routes::matches(v6.iter().map(String::as_str), excluded, true);
                FreeMibTable(table.cast());
            }
        }
        Ok(result)
    }
}
#[cfg(unix)]
pub fn observe_expected(excluded: &[String]) -> Result<Observation, String> {
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
                        libc::AF_INET => {
                            let address = &*(item.ifa_addr as *const libc::sockaddr_in);
                            result.ipv4 |=
                                std::net::Ipv4Addr::from(address.sin_addr.s_addr.to_ne_bytes())
                                    .to_string()
                                    == crate::profile::tun::ADDRESS[0].split('/').next().unwrap();
                        }
                        libc::AF_INET6 => {
                            let address = &*(item.ifa_addr as *const libc::sockaddr_in6);
                            result.ipv6 |= std::net::Ipv6Addr::from(address.sin6_addr.s6_addr)
                                .to_string()
                                == crate::profile::tun::ADDRESS[1].split('/').next().unwrap();
                        }
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
                let output = crate::platform::command::output(
                    ip,
                    &["-j", family, "route", "show", "table", "all"],
                )
                .map_err(|e| format!("无法读取路由，请安装 iproute2：{e}"))?;
                if !output.status.success() {
                    return Err("无法读取 TUN 路由表".into());
                }
                let rules = crate::platform::command::output(ip, &["-j", family, "rule", "show"])?;
                if !rules.status.success() {
                    return Err("无法读取 TUN 策略规则".into());
                }
                routes::linux_expected(
                    &String::from_utf8_lossy(&output.stdout),
                    &String::from_utf8_lossy(&rules.stdout),
                    interface_name(),
                    family == "-6",
                    excluded,
                )
            };
            result.routes = routes("-4")? && routes("-6")?;
        }
        #[cfg(target_os = "macos")]
        {
            let routes = |family: &str| -> Result<bool, String> {
                let output =
                    crate::platform::command::output("/usr/sbin/netstat", &["-rn", "-f", family])
                        .map_err(|e| e.to_string())?;
                if !output.status.success() {
                    return Err("无法读取 TUN 路由状态".into());
                }
                Ok(routes::macos_expected(
                    &String::from_utf8_lossy(&output.stdout),
                    interface_name(),
                    family == "inet6",
                    excluded,
                ))
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

pub fn require_fakeip_clear() -> Result<(), String> {
    #[cfg(windows)]
    let conflict = unsafe {
        use windows_sys::Win32::NetworkManagement::IpHelper::{
            FreeMibTable, GetIfEntry2, GetIpForwardTable2, MIB_IF_ROW2, MIB_IPFORWARD_TABLE2,
        };
        use windows_sys::Win32::Networking::WinSock::AF_INET;
        let mut table: *mut MIB_IPFORWARD_TABLE2 = std::ptr::null_mut();
        let status = GetIpForwardTable2(AF_INET, &mut table);
        if status != 0 || table.is_null() {
            return Err("无法核对 Fake-IP 地址池的路由占用".into());
        }
        let rows =
            std::slice::from_raw_parts((*table).Table.as_ptr(), (*table).NumEntries as usize);
        let conflict = rows.iter().any(|route| {
            let mut interface: MIB_IF_ROW2 = std::mem::zeroed();
            interface.InterfaceIndex = route.InterfaceIndex;
            let own = GetIfEntry2(&mut interface) == 0
                && String::from_utf16_lossy(
                    &interface.Alias[..interface
                        .Alias
                        .iter()
                        .position(|&c| c == 0)
                        .unwrap_or(interface.Alias.len())],
                ) == interface_name();
            !own && routes::conflicts_with_fakeip(&format!(
                "{}/{}",
                std::net::Ipv4Addr::from(
                    route
                        .DestinationPrefix
                        .Prefix
                        .Ipv4
                        .sin_addr
                        .S_un
                        .S_addr
                        .to_ne_bytes()
                ),
                route.DestinationPrefix.PrefixLength
            ))
        });
        FreeMibTable(table.cast());
        conflict
    };
    #[cfg(target_os = "linux")]
    let conflict = {
        let ip = if Path::new("/usr/sbin/ip").is_file() {
            "/usr/sbin/ip"
        } else {
            "/sbin/ip"
        };
        let output =
            crate::platform::command::output(ip, &["-j", "-4", "route", "show", "table", "all"])?;
        if !output.status.success() {
            return Err("无法核对 Fake-IP 地址池的路由占用".into());
        }
        let rows: Vec<serde_json::Value> =
            serde_json::from_slice(&output.stdout).map_err(|_| "系统路由输出无效")?;
        rows.iter().any(|row| {
            row["dev"].as_str() != Some(interface_name())
                && row["dst"]
                    .as_str()
                    .is_some_and(routes::conflicts_with_fakeip)
        })
    };
    #[cfg(target_os = "macos")]
    let conflict = {
        let output = crate::platform::command::output("/usr/sbin/netstat", &["-rn", "-f", "inet"])?;
        if !output.status.success() {
            return Err("无法核对 Fake-IP 地址池的路由占用".into());
        }
        String::from_utf8_lossy(&output.stdout).lines().any(|line| {
            let mut columns = line.split_whitespace();
            let destination = columns.next().unwrap_or("");
            !columns.any(|part| part == interface_name())
                && routes::conflicts_with_fakeip(destination)
        })
    };
    if conflict {
        Err("当前网络已使用 Fake-IP 保留网段 198.18.0.0/15，请处理地址冲突后重新连接".into())
    } else {
        Ok(())
    }
}
