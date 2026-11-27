use super::routing::{Match, Routing};
use crate::ip_network::{network, overlap};
use std::{
    collections::{BTreeMap, BTreeSet},
    net::IpAddr,
};

pub const MAX_DOMAINS: usize = 32;
pub const MAX_PREFIXES: usize = 64;

pub fn validate_address(value: &str) -> Result<(), String> {
    if value
        .split('/')
        .next()
        .and_then(|s| s.trim().parse::<std::net::Ipv6Addr>().ok())
        .is_some_and(|ip| ip.to_ipv4_mapped().is_some())
    {
        return Err("绕过 TUN 不接受 IPv4-mapped IPv6，请直接填写 IPv4".into());
    }
    let net = network(value)?;
    if matches!(net.0, IpAddr::V6(ip) if ip.to_ipv4_mapped().is_some()) {
        return Err("绕过 TUN 不接受 IPv4-mapped IPv6，请直接填写 IPv4".into());
    }
    for reserved in super::tun::ADDRESS
        .into_iter()
        .chain([super::config::FAKEIP_RANGE])
    {
        if overlap(net, network(reserved)?) {
            return Err("绕过目标与 TUN / DNS 或 Fake-IP 保留网段冲突".into());
        }
    }
    Ok(())
}

#[derive(Default)]
pub struct Targets {
    // Domain -> rule name, used only for preparation errors.
    pub domains: BTreeMap<String, String>,
    pub prefixes: BTreeSet<String>,
}
impl Targets {
    pub fn collect(routing: &Routing) -> Result<Self, String> {
        let mut targets = Self::default();
        for policy in routing
            .policies
            .iter()
            .filter(|p| p.enabled && p.bypass_tun)
        {
            let Match::Custom { domains, ip_cidrs } = &policy.r#match else {
                return Err("绕过 TUN 仅支持自定义直连规则".into());
            };
            for domain in domains {
                targets
                    .domains
                    .entry(domain.value.clone())
                    .or_insert_with(|| policy.name.clone());
            }
            targets.prefixes.extend(ip_cidrs.iter().cloned());
        }
        if targets.domains.len() > MAX_DOMAINS || targets.prefixes.len() > MAX_PREFIXES {
            return Err("绕过 TUN 最多支持 32 个精确域名和 64 个排除地址 / 网段".into());
        }
        Ok(targets)
    }
}

// Worker-owned, never persisted in user settings or accepted from IPC.
#[derive(Clone, Default, Debug, PartialEq, Eq)]
pub struct Resolved {
    pub hosts: BTreeMap<String, Vec<IpAddr>>,
    pub prefixes: Vec<String>,
}
impl Resolved {
    pub fn new(
        routing: &Routing,
        mut hosts: BTreeMap<String, Vec<IpAddr>>,
    ) -> Result<Self, String> {
        let targets = Targets::collect(routing)?;
        if targets.domains.keys().ne(hosts.keys()) {
            return Err("绕过域名的固定解析结果不完整或包含未知域名".into());
        }
        let mut prefixes = targets.prefixes;
        for (domain, addresses) in &mut hosts {
            addresses.sort_unstable();
            addresses.dedup();
            if addresses.is_empty() {
                return Err(format!(
                    "{}：{domain} 没有可用真实地址，绕过 TUN 未应用",
                    targets.domains[domain]
                ));
            }
            for address in addresses {
                let prefix = format!("{address}/{}", if address.is_ipv4() { 32 } else { 128 });
                validate_address(&prefix)
                    .map_err(|e| format!("{} / {domain}：{e}", targets.domains[domain]))?;
                prefixes.insert(prefix);
            }
        }
        if prefixes.len() > MAX_PREFIXES {
            return Err("绕过 TUN 的解析结果超过 64 个排除地址 / 网段".into());
        }
        for prefix in &prefixes {
            validate_address(prefix)?;
        }
        Ok(Self {
            hosts,
            prefixes: prefixes.into_iter().collect(),
        })
    }
    pub fn validate(&self, routing: &Routing) -> Result<(), String> {
        if &Self::new(routing, self.hosts.clone())? != self {
            return Err("绕过 TUN 的运行结果与规则不一致".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn routing() -> Routing {
        serde_json::from_value(json!({"defaultExit":"proxy","policies":[{"id":"rd","name":"RustDesk","exit":"direct","enabled":true,"bypassTun":true,"match":{"kind":"custom","domains":[{"value":"id.example.test","includeSubdomains":false}],"ipCidrs":["203.0.113.0/24"]}}]})).unwrap()
    }
    #[test]
    fn freezes_complete_answers_and_rejects_missing_unsafe_or_extra_results() {
        let r = routing().normalized().unwrap();
        assert!(Resolved::new(&r, BTreeMap::new()).is_err());
        let hosts = BTreeMap::from([(
            "id.example.test".into(),
            vec!["192.0.2.1".parse().unwrap(), "2001:db8::1".parse().unwrap()],
        )]);
        let resolved = Resolved::new(&r, hosts).unwrap();
        resolved.validate(&r).unwrap();
        assert_eq!(resolved.prefixes.len(), 3);
        for ip in [
            "198.18.0.1",
            "172.19.0.2",
            "fdfe:dcba:9876::2",
            "::ffff:192.0.2.1",
        ] {
            assert!(Resolved::new(
                &r,
                BTreeMap::from([("id.example.test".into(), vec![ip.parse().unwrap()])])
            )
            .is_err());
        }
        let mut changed = resolved.clone();
        changed.prefixes.push("192.0.2.9/32".into());
        assert!(changed.validate(&r).is_err());
    }
    #[test]
    fn bypass_requires_direct_custom_exact_and_preserves_legacy() {
        let mut r = routing();
        r.policies[0].exit = super::super::routing::Action::Proxy;
        assert!(r.validate().is_err());
        let mut r = routing();
        if let Match::Custom { domains, .. } = &mut r.policies[0].r#match {
            domains[0].include_subdomains = true;
        }
        assert!(r.validate().is_err());
        let r: Routing =
            serde_json::from_value(json!({"defaultExit":"proxy","policies":[]})).unwrap();
        assert!(Resolved::new(&r, BTreeMap::new())
            .unwrap()
            .prefixes
            .is_empty());
        for p in [
            "0.0.0.0/0",
            "::/0",
            "198.0.0.0/8",
            "172.19.0.0/16",
            "::ffff:192.0.2.1/64",
        ] {
            assert!(validate_address(p).is_err());
        }
    }
}
