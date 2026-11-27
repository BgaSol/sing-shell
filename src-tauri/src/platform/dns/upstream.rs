use serde_json::{json, Value};

pub fn resolver() -> Result<Value, String> {
    #[cfg(target_os = "linux")]
    {
        let routes =
            crate::platform::command::output("/sbin/ip", &["-j", "route", "show", "default"])?;
        if !routes.status.success() {
            return Err("无法确认物理默认接口".into());
        }
        let rows: Value = serde_json::from_slice(&routes.stdout).map_err(|_| "物理路由输出无效")?;
        let interface = rows
            .as_array()
            .and_then(|a| {
                a.iter()
                    .filter_map(|v| v["dev"].as_str())
                    .find(|n| *n != crate::profile::tun::INTERFACE && *n != "lo")
            })
            .ok_or("没有独立的默认网络接口")?;
        let output = crate::platform::command::output("/usr/bin/resolvectl", &["dns", interface]);
        let addresses = match output {
            Ok(output) if output.status.success() => String::from_utf8(output.stdout)
                .map_err(|_| "系统 DNS 输出无效")?
                .split_once(": ")
                .ok_or("物理接口没有 DNS")?
                .1
                .split_whitespace()
                .map(str::to_owned)
                .collect::<Vec<_>>(),
            _ => std::fs::read_to_string("/etc/resolv.conf")
                .map_err(|_| "无法读取物理 DNS")?
                .lines()
                .filter_map(|line| {
                    let mut p = line.split_whitespace();
                    (p.next() == Some("nameserver"))
                        .then(|| p.next().map(str::to_owned))
                        .flatten()
                })
                .collect(),
        };
        let server = addresses
            .into_iter()
            .find(|a| {
                a.split('%')
                    .next()
                    .and_then(|s| s.parse::<std::net::IpAddr>().ok())
                    .is_some_and(|ip| {
                        !ip.is_loopback()
                            && !ip.is_unspecified()
                            && !crate::profile::tun::DNS.contains(&ip.to_string().as_str())
                    })
            })
            .ok_or("物理接口没有独立 DNS，请检查网络后重试")?;
        Ok(json!({"type":"udp","tag":"physical","server":server,"bind_interface":interface}))
    }
    #[cfg(not(target_os = "linux"))]
    {
        Ok(json!({"type":"local","tag":"physical"}))
    }
}
