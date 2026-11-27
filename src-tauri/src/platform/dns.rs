#[cfg(target_os = "macos")]
mod macos;
pub(crate) mod upstream;

#[derive(Default)]
pub struct Dns {
    #[cfg(target_os = "macos")]
    registration: Option<macos::Registration>,
}
impl Dns {
    pub fn install(&mut self) -> Result<(), String> {
        #[cfg(target_os = "macos")]
        {
            if self.registration.is_some() {
                return Err("本次 DNS 已登记".into());
            }
            self.registration = Some(macos::Registration::install()?);
            self.registration.as_ref().unwrap().activate()?;
        }
        Ok(())
    }
    pub fn check(&self) -> Result<(), String> {
        #[cfg(target_os = "macos")]
        self.registration
            .as_ref()
            .ok_or("本次 DNS 尚未登记")?
            .check()?;
        Ok(())
    }
    pub fn close(&mut self) -> Result<(), String> {
        #[cfg(target_os = "macos")]
        if let Some(registration) = &mut self.registration {
            registration.close()?;
            self.registration = None;
        }
        Ok(())
    }
}

#[cfg(any(test, target_os = "macos"))]
fn resolver_ready(text: &str, physical_name: &str, require_tunnel: bool) -> bool {
    let (global, scoped) = text
        .split_once("DNS configuration (for scoped queries)")
        .unwrap_or((text, ""));
    let physical = scoped.split("resolver #").any(|block| {
        block.lines().any(|line| {
            line.trim_start().starts_with("if_index")
                && line.contains(&format!("({physical_name})"))
                && !line.contains("utun")
        }) && block.lines().any(|line| {
            line.trim_start().starts_with("nameserver[")
                && !crate::profile::tun::DNS
                    .iter()
                    .any(|dns| line.contains(dns))
        })
    });
    let default = global.split("resolver #").find(|block| {
        block
            .lines()
            .any(|line| line.trim_start().starts_with("nameserver["))
            && !block
                .lines()
                .any(|line| line.trim_start().starts_with("domain"))
    });
    let tunnel = default.is_some_and(|block| {
        !block
            .lines()
            .any(|line| line.trim_start().starts_with("domain"))
            && crate::profile::tun::DNS.iter().all(|dns| {
                block.lines().any(|line| {
                    line.trim_start().starts_with("nameserver[")
                        && line.split(": ").last() == Some(*dns)
                })
            })
            && block.lines().any(|line| line.trim() == "order    : 1")
    });
    physical && (!require_tunnel || tunnel)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn default_tunnel_dns_requires_an_independent_scoped_upstream() {
        let physical = "DNS configuration (for scoped queries)\nresolver #1\n nameserver[0] : fe80::1%en0\n if_index : 9 (en0)\n";
        let tunnel = "resolver #1\n nameserver[0] : 172.19.0.2\n nameserver[1] : fdfe:dcba:9876::2\n order    : 1\n";
        assert!(resolver_ready(physical, "en0", false));
        assert!(!resolver_ready(physical, "en0", true));
        assert!(!resolver_ready(tunnel, "en0", true));
        assert!(!resolver_ready(
            &format!("resolver #1\n nameserver[0] : 1.1.1.1\n order    : 0\n{tunnel}{physical}"),
            "en0",
            true
        ));
        assert!(resolver_ready(&format!("{tunnel}{physical}"), "en0", true));
        assert!(!resolver_ready(
            &format!("{tunnel}{}", physical.replace("en0", "utun223")),
            "en0",
            true
        ));
    }
}
