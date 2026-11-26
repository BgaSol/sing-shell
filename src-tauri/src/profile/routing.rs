use serde::{Deserialize, Serialize};
use std::net::IpAddr;
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Action {
    #[default]
    Proxy,
    Direct,
}
impl Action {
    pub fn tag(self) -> &'static str {
        match self {
            Self::Proxy => "proxy",
            Self::Direct => "direct",
        }
    }
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Domain {
    pub value: String,
    pub include_subdomains: bool,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum Match {
    Custom {
        domains: Vec<Domain>,
        #[serde(rename = "ipCidrs")]
        ip_cidrs: Vec<String>,
    },
    Source {
        #[serde(rename = "sourceId")]
        source_id: String,
    },
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Policy {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub exit: Action,
    pub r#match: Match,
}
impl Policy {
    pub fn tag(&self) -> String {
        format!("policy-{}", self.id)
    }
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Routing {
    pub default_exit: Action,
    pub policies: Vec<Policy>,
}
impl Default for Routing {
    fn default() -> Self {
        Self {
            default_exit: Action::Proxy,
            policies: [("cn-domain", "国内站点"), ("cn-ip", "中国大陆 IP")]
                .into_iter()
                .map(|(id, name)| Policy {
                    id: id.into(),
                    name: name.into(),
                    enabled: true,
                    exit: Action::Direct,
                    r#match: Match::Source {
                        source_id: id.into(),
                    },
                })
                .collect(),
        }
    }
}
pub const PRIVATE_NETWORKS: &[&str] = &[
    "127.0.0.0/8",
    "10.0.0.0/8",
    "172.16.0.0/12",
    "192.168.0.0/16",
    "169.254.0.0/16",
    "::1/128",
    "fc00::/7",
    "fe80::/10",
];
pub fn domain(value: &str) -> Result<String, String> {
    let value = value.trim().trim_end_matches('.');
    if value.is_empty()
        || value
            .chars()
            .any(|c| c.is_whitespace() || matches!(c, '/' | '\\' | ':' | '@' | '?' | '#' | '*'))
    {
        return Err("请输入主机域名，不包含 URL、端口或通配符".into());
    }
    let url = reqwest::Url::parse(&format!("https://{value}/")).map_err(|_| "域名格式无效")?;
    let host = url.host_str().ok_or("域名为空")?.to_ascii_lowercase();
    if host.len() > 253
        || host.parse::<IpAddr>().is_ok()
        || host.split('.').any(|label| {
            label.is_empty()
                || label.len() > 63
                || label.starts_with('-')
                || label.ends_with('-')
                || !label
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || c == b'-')
        })
    {
        return Err("域名格式无效，IP 请使用 IP 规则".into());
    }
    Ok(host)
}
pub fn network(value: &str) -> Result<(IpAddr, u8), String> {
    let (address, prefix) = value
        .trim()
        .split_once('/')
        .map(|(a, p)| (a, Some(p)))
        .unwrap_or((value.trim(), None));
    let address = address.parse::<IpAddr>().map_err(|_| "IP 地址格式无效")?;
    let max = if address.is_ipv4() { 32 } else { 128 };
    let prefix = prefix
        .map(|p| p.parse::<u8>().map_err(|_| "网段前缀无效"))
        .transpose()?
        .unwrap_or(max);
    if prefix > max {
        return Err("网段前缀超出地址长度".into());
    }
    let masked = match address {
        IpAddr::V4(a) => IpAddr::V4(
            (u32::from(a)
                & if prefix == 0 {
                    0
                } else {
                    u32::MAX << (32 - prefix)
                })
            .into(),
        ),
        IpAddr::V6(a) => IpAddr::V6(
            (u128::from(a)
                & if prefix == 0 {
                    0
                } else {
                    u128::MAX << (128 - prefix)
                })
            .into(),
        ),
    };
    Ok((masked, prefix))
}
fn overlap(a: (IpAddr, u8), b: (IpAddr, u8)) -> bool {
    if a.0.is_ipv4() != b.0.is_ipv4() {
        return false;
    }
    let bits = a.1.min(b.1);
    match (a.0, b.0) {
        (IpAddr::V4(a), IpAddr::V4(b)) => {
            bits == 0 || (u32::from(a) >> (32 - bits)) == (u32::from(b) >> (32 - bits))
        }
        (IpAddr::V6(a), IpAddr::V6(b)) => {
            bits == 0 || (u128::from(a) >> (128 - bits)) == (u128::from(b) >> (128 - bits))
        }
        _ => false,
    }
}
impl Routing {
    pub fn normalized(&self) -> Result<Self, String> {
        if self.policies.len() > 100 {
            return Err("最多保存 100 条策略".into());
        }
        let mut out = self.clone();
        let mut ids = std::collections::BTreeSet::new();
        let mut sources = std::collections::BTreeSet::new();
        let mut count = 0;
        for (index, p) in out.policies.iter_mut().enumerate() {
            if p.id.is_empty()
                || p.id.len() > 64
                || !p.id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
                || !ids.insert(p.id.clone())
            {
                return Err(format!("第 {} 条策略标识无效或重复", index + 1));
            }
            p.name = p.name.trim().into();
            if p.name.is_empty()
                || p.name.chars().count() > 64
                || p.name.chars().any(char::is_control)
            {
                return Err(format!("第 {} 条策略名称无效", index + 1));
            }
            let name = p.name.clone();
            let issue = |message: &str| format!("{name}：{message}");
            match &mut p.r#match {
                Match::Source { source_id } => {
                    crate::rulesets::definition(source_id)?;
                    if !sources.insert(source_id.clone()) {
                        return Err(issue("同一开源来源只能添加一次"));
                    }
                }
                Match::Custom { domains, ip_cidrs } => {
                    count += domains.len() + ip_cidrs.len();
                    if domains.is_empty() && ip_cidrs.is_empty() {
                        return Err(issue("至少填写一个域名或 IP"));
                    }
                    let mut keys = std::collections::BTreeSet::new();
                    for (i, d) in domains.iter_mut().enumerate() {
                        d.value = domain(&d.value)
                            .map_err(|e| format!("{name} / 域名 {}：{e}", i + 1))?;
                        if !keys.insert(format!("{}:{}", d.include_subdomains, d.value)) {
                            return Err(format!("{name} / 域名 {}：条件重复", i + 1));
                        }
                    }
                    let mut keys = std::collections::BTreeSet::new();
                    for (i, value) in ip_cidrs.iter_mut().enumerate() {
                        let net =
                            network(value).map_err(|e| format!("{name} / IP {}：{e}", i + 1))?;
                        if p.exit == Action::Proxy
                            && PRIVATE_NETWORKS
                                .iter()
                                .any(|n| overlap(net, network(n).unwrap()))
                        {
                            return Err(format!(
                                "{name} / IP {}：本机和局域网固定直连；全局出口请修改默认规则",
                                i + 1
                            ));
                        }
                        *value = format!("{}/{}", net.0, net.1);
                        if !keys.insert(value.clone()) {
                            return Err(format!("{name} / IP {}：条件重复", i + 1));
                        }
                    }
                    domains.sort_by(|a, b| {
                        (&a.value, a.include_subdomains).cmp(&(&b.value, b.include_subdomains))
                    });
                    ip_cidrs.sort();
                }
            }
        }
        if count > 1000 {
            return Err("自定义匹配项总数最多 1,000 个".into());
        }
        Ok(out)
    }
    pub fn validate(&self) -> Result<(), String> {
        self.normalized().map(|_| ())
    }
    pub fn effective(&self) -> Self {
        let mut v = self.clone();
        v.policies.retain(|p| p.enabled);
        for p in &mut v.policies {
            p.name.clear();
        }
        v
    }
    pub fn source_ids(&self) -> Vec<String> {
        self.policies
            .iter()
            .filter_map(|p| {
                if let Match::Source { source_id } = &p.r#match {
                    Some(source_id.clone())
                } else {
                    None
                }
            })
            .collect()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn custom(id: &str) -> Policy {
        Policy {
            id: id.into(),
            name: id.into(),
            enabled: true,
            exit: Action::Direct,
            r#match: Match::Custom {
                domains: vec![Domain {
                    value: " Example.COM. ".into(),
                    include_subdomains: true,
                }],
                ip_cidrs: vec!["192.0.2.3/24".into()],
            },
        }
    }
    #[test]
    fn normalization_preserves_priority_and_overlap() {
        let r = Routing {
            default_exit: Action::Proxy,
            policies: vec![custom("b"), custom("a")],
        }
        .normalized()
        .unwrap();
        assert_eq!(r.policies[0].id, "b");
        if let Match::Custom { domains, ip_cidrs } = &r.policies[0].r#match {
            assert_eq!(domains[0].value, "example.com");
            assert_eq!(ip_cidrs[0], "192.0.2.0/24");
        } else {
            panic!()
        }
    }
    #[test]
    fn duplicate_and_protected_ranges_are_rejected() {
        let mut p = custom("a");
        if let Match::Custom { domains, .. } = &mut p.r#match {
            domains.push(domains[0].clone());
        }
        assert!(Routing {
            default_exit: Action::Proxy,
            policies: vec![p]
        }
        .validate()
        .is_err());
        let mut p = custom("a");
        p.exit = Action::Proxy;
        if let Match::Custom { ip_cidrs, .. } = &mut p.r#match {
            *ip_cidrs = vec!["0.0.0.0/0".into()];
        }
        assert!(Routing {
            default_exit: Action::Proxy,
            policies: vec![p]
        }
        .validate()
        .is_err());
    }
    #[test]
    fn metadata_and_disabled_policies_do_not_change_effective_input() {
        let mut a = Routing::default();
        let before = a.effective();
        a.policies[0].name = "改名".into();
        let mut p = custom("off");
        p.enabled = false;
        a.policies.push(p);
        assert_eq!(before, a.effective());
    }
}
