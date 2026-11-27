//! Read-only native route query for disposable Windows package/network acceptance.
pub fn run() -> bool {
    if std::env::args().nth(1).as_deref() != Some("--route-probe") {
        return false;
    }
    #[cfg(windows)]
    {
        let result = std::env::args()
            .skip(2)
            .map(|address| query(&address))
            .collect::<Result<Vec<_>, _>>();
        match result {
            Ok(rows) => println!("{}", serde_json::to_string(&rows).unwrap()),
            Err(error) => {
                eprintln!("{error}");
                std::process::exit(1);
            }
        }
        true
    }
    #[cfg(not(windows))]
    std::process::exit(1);
}

#[cfg(windows)]
fn query(address: &str) -> Result<serde_json::Value, String> {
    use std::net::IpAddr;
    use windows_sys::Win32::{
        NetworkManagement::IpHelper::{
            ConvertInterfaceLuidToAlias, GetBestRoute2, MIB_IPFORWARD_ROW2,
        },
        Networking::WinSock::{AF_INET, AF_INET6, SOCKADDR_INET},
    };
    let ip: IpAddr = address.parse().map_err(|_| "invalid probe IP")?;
    unsafe {
        let mut destination: SOCKADDR_INET = std::mem::zeroed();
        match ip {
            IpAddr::V4(ip) => {
                destination.Ipv4.sin_family = AF_INET;
                destination.Ipv4.sin_addr.S_un.S_addr = u32::from_ne_bytes(ip.octets());
            }
            IpAddr::V6(ip) => {
                destination.Ipv6.sin6_family = AF_INET6;
                destination.Ipv6.sin6_addr.u.Byte = ip.octets();
            }
        }
        let mut route: MIB_IPFORWARD_ROW2 = std::mem::zeroed();
        let mut source = std::mem::zeroed();
        let code = GetBestRoute2(
            std::ptr::null(),
            0,
            std::ptr::null(),
            &destination,
            0,
            &mut route,
            &mut source,
        );
        if ip.is_ipv6() && [1168, 1231].contains(&code) {
            return Ok(serde_json::json!({"address":address,"interfaces":[],"noRouteCode":code}));
        }
        if code != 0 {
            return Err(format!("GetBestRoute2({address}): {code}"));
        }
        let mut alias = [0u16; 257];
        let code =
            ConvertInterfaceLuidToAlias(&route.InterfaceLuid, alias.as_mut_ptr(), alias.len());
        if code != 0 {
            return Err(format!("ConvertInterfaceLuidToAlias: {code}"));
        }
        let end = alias
            .iter()
            .position(|c| *c == 0)
            .ok_or("unterminated interface alias")?;
        Ok(
            serde_json::json!({"address":address,"interfaces":[String::from_utf16_lossy(&alias[..end])],"interfaceIndex":route.InterfaceIndex}),
        )
    }
}
