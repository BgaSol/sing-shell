use super::{Mode, Profile};
use crate::settings;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
};

pub struct Generated {
    pub path: PathBuf,
    pub digest: String,
}

pub fn build(profile: &Profile, root: &Path) -> Result<Generated, String> {
    if profile.mode != Mode::Generated {
        return Err("当前使用的是原始配置模式".into());
    }
    let node = profile.selected().ok_or("请先选择节点")?;
    node.spec.validate()?;
    let mut rules = profile.routing.core_rules()?;
    let resolve_at = rules
        .iter()
        .enumerate()
        .find(|(index, rule)| *index > 0 && rule.get("ip_cidr").is_some())
        .map(|(index, _)| index)
        .unwrap_or_else(|| {
            rules
                .iter()
                .position(|rule| rule.get("rule_set").is_some())
                .unwrap_or(rules.len())
        });
    rules.insert(resolve_at, json!({"action":"resolve"}));
    rules.insert(
        resolve_at + 1,
        json!({"ip_cidr":super::routing::PRIVATE_NETWORKS,"action":"route","outbound":"direct"}),
    );
    rules.insert(0, json!({"action":"sniff"}));
    rules.insert(1, json!({"protocol":"dns","action":"hijack-dns"}));
    let mut rule_sets = Vec::new();
    for site in &profile.routing.site_rules {
        let path = crate::rulesets::materialize(root, &site.site)?;
        rule_sets.push(json!({"type":"local","tag":format!("site-{}",site.site),"format":"binary","path":path}));
    }
    let mut dns_rules = Vec::new();
    for rule in &profile.routing.normalized()?.custom_rules {
        if let super::routing::Rule::Domain {
            value,
            include_subdomains,
            action,
        } = rule
        {
            let mut rule = if *include_subdomains {
                json!({"domain_suffix":[value]})
            } else {
                json!({"domain":[value]})
            };
            rule["server"] = json!(if *action == super::routing::Action::Direct {
                "local"
            } else {
                "proxy-dns"
            });
            dns_rules.push(rule);
        }
    }
    for site in &profile.routing.normalized()?.site_rules {
        dns_rules.push(json!({"rule_set":[format!("site-{}",site.site)],"server":if site.action==super::routing::Action::Direct{"local"}else{"proxy-dns"}}));
    }
    let mut outbound = node.spec.outbound();
    outbound["domain_resolver"] = json!("local");
    let value = json!({
        "log":{"level":"info","disabled":false,"timestamp":false},
        "dns":{"servers":[{"type":"local","tag":"local"},{"type":"https","tag":"proxy-dns","server":"1.1.1.1","path":"/dns-query","tls":{"enabled":true,"server_name":"cloudflare-dns.com"},"detour":"proxy"}],"rules":dns_rules,"final":if profile.routing.default_action==super::routing::Action::Direct{"local"}else{"proxy-dns"}},
        "inbounds":[{"type":"tun","tag":"tun-in","interface_name":if cfg!(target_os="macos"){"utun223"}else{"sing-shell"},"address":["172.19.0.1/30","fdfe:dcba:9876::1/126"],"auto_route":true,"strict_route":true,"stack":"mixed","mtu":1400,"dns_mode":"hijack","dns_address":["172.19.0.2","fdfe:dcba:9876::2"]}],
        "outbounds":[outbound,{"type":"direct","tag":"direct","domain_resolver":"local"}],
        "route":{"auto_detect_interface":true,"default_domain_resolver":"local","rules":rules,"rule_set":rule_sets,"final":profile.routing.default_action.tag()}
    });
    let bytes = serde_json::to_vec_pretty(&value).map_err(|e| e.to_string())?;
    let digest = Sha256::digest(&bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    let dir = root.join("runtime");
    crate::private_files::directory(&dir)?;
    let path = dir.join("generated.json");
    settings::write_json(&path, &value)?;
    Ok(Generated { path, digest })
}

pub fn remove_generated(root: &Path) {
    let _ = fs::remove_file(root.join("runtime/generated.json"));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::{Node, NodeSpec, Profile};
    use serde_json::Value;
    #[test]
    fn generated_config_is_deterministic_and_has_tun_and_default_proxy() {
        let node = Node {
            id: "n".into(),
            name: "test".into(),
            source: "test".into(),
            spec: NodeSpec::Trojan {
                server: "example.com".into(),
                server_port: 443,
                password: "secret".into(),
                tls: crate::profile::Tls {
                    enabled: true,
                    server_name: Some("example.com".into()),
                    alpn: vec![],
                    insecure: false,
                    utls: None,
                    reality: None,
                },
                transport: None,
            },
        };
        let mut profile = Profile::default();
        profile.nodes.push(node);
        profile.selected_node = Some("n".into());
        let root = std::env::temp_dir().join(format!("sing-shell-profile-{}", std::process::id()));
        let a = build(&profile, &root).unwrap();
        let b = build(&profile, &root).unwrap();
        assert_eq!(a.digest, b.digest);
        let value: Value = serde_json::from_slice(&fs::read(a.path).unwrap()).unwrap();
        assert_eq!(value["inbounds"][0]["type"], "tun");
        assert_eq!(value["route"]["final"], "proxy");
        let _ = fs::remove_dir_all(root);
    }
}
