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
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum Rule {
    Domain {
        value: String,
        #[serde(rename = "includeSubdomains")]
        include_subdomains: bool,
        action: Action,
    },
    Ip {
        value: String,
        action: Action,
    },
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SiteRule {
    pub site: String,
    pub action: Action,
}
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Routing {
    pub default_action: Action,
    pub custom_rules: Vec<Rule>,
    pub site_rules: Vec<SiteRule>,
}

pub const SITE_IDS: &[&str] = &[
    "github",
    "google",
    "youtube",
    "telegram",
    "microsoft",
    "apple",
    "openai",
    "cn",
];
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
        if self.custom_rules.len() > 1000 || self.site_rules.len() > SITE_IDS.len() {
            return Err("分流规则超过上限".into());
        }
        let mut keys = std::collections::BTreeSet::new();
        let mut rules = Vec::new();
        for rule in &self.custom_rules {
            let (key, normalized) = match rule {
                Rule::Domain {
                    value,
                    include_subdomains,
                    action,
                } => {
                    let value = domain(value)?;
                    (
                        format!("domain:{include_subdomains}:{value}"),
                        Rule::Domain {
                            value,
                            include_subdomains: *include_subdomains,
                            action: *action,
                        },
                    )
                }
                Rule::Ip { value, action } => {
                    let parsed = network(value)?;
                    if *action == Action::Proxy
                        && PRIVATE_NETWORKS
                            .iter()
                            .any(|n| overlap(parsed, network(n).unwrap()))
                    {
                        return Err("本机和局域网固定直连，不能添加与之冲突的代理网段".into());
                    }
                    let value = format!("{}/{}", parsed.0, parsed.1);
                    (
                        format!("ip:{value}"),
                        Rule::Ip {
                            value,
                            action: *action,
                        },
                    )
                }
            };
            if !keys.insert(key) {
                return Err("相同规则只能有一个代理或直连动作".into());
            }
            rules.push(normalized);
        }
        let mut sites = std::collections::BTreeSet::new();
        for site in &self.site_rules {
            if !SITE_IDS.contains(&site.site.as_str()) || !sites.insert(&site.site) {
                return Err("站点无效或重复，请为每个站点选择一个动作".into());
            }
        }
        rules.sort_by_key(|r| match r {
            Rule::Domain {
                value,
                include_subdomains,
                ..
            } => (
                u8::from(*include_subdomains),
                usize::MAX - value.len(),
                value.clone(),
            ),
            Rule::Ip { value, .. } => (
                2,
                usize::MAX - network(value).unwrap().1 as usize,
                value.clone(),
            ),
        });
        let mut site_rules = self.site_rules.clone();
        site_rules.sort_by_key(|r| (r.action != Action::Direct, r.site.clone()));
        Ok(Self {
            default_action: self.default_action,
            custom_rules: rules,
            site_rules,
        })
    }
    pub fn validate(&self) -> Result<(), String> {
        self.normalized().map(|_| ())
    }
    pub fn core_rules(&self) -> Result<Vec<serde_json::Value>, String> {
        let normalized = self.normalized()?;
        let mut rules = vec![
            serde_json::json!({"ip_cidr":PRIVATE_NETWORKS,"action":"route","outbound":"direct"}),
        ];
        for rule in normalized.custom_rules {
            rules.push(match rule{Rule::Domain{value,include_subdomains,action}=>if include_subdomains{serde_json::json!({"domain_suffix":[value],"action":"route","outbound":action.tag()})}else{serde_json::json!({"domain":[value],"action":"route","outbound":action.tag()})},Rule::Ip{value,action}=>serde_json::json!({"ip_cidr":[value],"action":"route","outbound":action.tag()})});
        }
        for site in normalized.site_rules {
            rules.push(serde_json::json!({"rule_set":[format!("site-{}",site.site)],"action":"route","outbound":site.action.tag()}));
        }
        Ok(rules)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn normalizes_domains_and_network_boundaries() {
        assert_eq!(domain(" GITHUB.COM. ").unwrap(), "github.com");
        assert!(domain("https://github.com/a").is_err());
        assert_eq!(
            network("203.0.113.199/24").unwrap(),
            ("203.0.113.0".parse().unwrap(), 24)
        );
        assert_eq!(
            network("2001:db8::19/64").unwrap(),
            ("2001:db8::".parse().unwrap(), 64)
        );
        assert!(network("1.2.3.4/33").is_err());
        assert_eq!(network("0.0.0.0/0").unwrap().1, 0);
    }
    #[test]
    fn conflicts_are_rejected_and_order_is_deterministic() {
        let r = Routing {
            default_action: Action::Proxy,
            custom_rules: vec![
                Rule::Ip {
                    value: "203.0.113.0/24".into(),
                    action: Action::Direct,
                },
                Rule::Domain {
                    value: "example.com".into(),
                    include_subdomains: true,
                    action: Action::Proxy,
                },
                Rule::Domain {
                    value: "www.example.com".into(),
                    include_subdomains: false,
                    action: Action::Direct,
                },
            ],
            site_rules: vec![
                SiteRule {
                    site: "github".into(),
                    action: Action::Proxy,
                },
                SiteRule {
                    site: "google".into(),
                    action: Action::Direct,
                },
            ],
        };
        let rules = r.core_rules().unwrap();
        assert_eq!(rules[1]["domain"][0], "www.example.com");
        assert_eq!(rules[2]["domain_suffix"][0], "example.com");
        assert_eq!(rules[4]["outbound"], "direct");
        let mut invalid = r.clone();
        invalid.custom_rules.push(Rule::Domain {
            value: "EXAMPLE.COM.".into(),
            include_subdomains: true,
            action: Action::Direct,
        });
        assert!(invalid.validate().is_err());
        invalid = r;
        invalid.custom_rules.push(Rule::Ip {
            value: "192.168.1.0/24".into(),
            action: Action::Proxy,
        });
        assert!(invalid.validate().is_err());
    }
}
