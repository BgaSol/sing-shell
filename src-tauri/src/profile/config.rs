use super::{
    routing::{Action, Match},
    Mode, Profile,
};
use crate::{
    rulesets::{self, Version},
    settings,
};
use serde_json::{json, Value};
use std::{
    fs,
    path::{Path, PathBuf},
};
pub struct Generated {
    pub path: PathBuf,
    pub digest: String,
}
// Compilation uses only the supplied profile and fixed content versions.
pub fn compile(profile: &Profile, versions: &[Version], root: &Path) -> Result<Value, String> {
    if profile.mode != Mode::Generated {
        return Err("当前为原始配置模式".into());
    }
    let node = profile.selected().ok_or("请先选择节点")?;
    node.spec.validate()?;
    let routing = profile.routing.normalized()?;
    let protected =
        json!({"ip_cidr":super::routing::PRIVATE_NETWORKS,"action":"route","outbound":"direct"});
    let mut rules = vec![
        json!({"action":"sniff"}),
        json!({"protocol":"dns","action":"hijack-dns"}),
        protected.clone(),
        json!({"action":"resolve"}),
        protected,
    ];
    let mut sets = Vec::new();
    let mut dns = Vec::new();
    for p in routing.policies.iter().filter(|p| p.enabled) {
        let tag = p.tag();
        let server = if p.exit == Action::Direct {
            "local"
        } else {
            "proxy-dns"
        };
        match &p.r#match {
            Match::Custom { domains, ip_cidrs } => {
                let mut items = Vec::new();
                let mut dns_items = Vec::new();
                for d in domains {
                    let r = if d.include_subdomains {
                        json!({"domain_suffix":[d.value]})
                    } else {
                        json!({"domain":[d.value]})
                    };
                    items.push(r.clone());
                    dns_items.push(r);
                }
                if !ip_cidrs.is_empty() {
                    items.push(json!({"ip_cidr":ip_cidrs}));
                }
                sets.push(json!({"type":"inline","tag":tag,"rules":[{"type":"logical","mode":"or","rules":items}]}));
                if !dns_items.is_empty() {
                    dns.push(
                        json!({"type":"logical","mode":"or","rules":dns_items,"server":server}),
                    );
                }
            }
            Match::Source { source_id } => {
                let version = versions
                    .iter()
                    .find(|v| &v.source_id == source_id)
                    .ok_or_else(|| format!("{} 没有固定可用版本", p.name))?;
                let path = rulesets::materialize_at(root, version)?;
                sets.push(json!({"type":"local","tag":tag,"format":"binary","path":path}));
                if rulesets::definition(source_id)?.domains {
                    dns.push(json!({"rule_set":[tag],"server":server}));
                }
            }
        }
        rules.push(json!({"rule_set":[tag],"action":"route","outbound":p.exit.tag()}));
    }
    let mut outbound = node.spec.outbound();
    outbound["domain_resolver"] = json!("local");
    Ok(
        json!({"log":{"level":"info","disabled":false,"timestamp":false},"dns":{"servers":[{"type":"local","tag":"local"},{"type":"https","tag":"proxy-dns","server":"1.1.1.1","path":"/dns-query","tls":{"enabled":true,"server_name":"cloudflare-dns.com"},"detour":"proxy"}],"rules":dns,"final":if routing.default_exit==Action::Direct{"local"}else{"proxy-dns"}},"inbounds":[{"type":"tun","tag":"tun-in","interface_name":super::tun::INTERFACE,"address":super::tun::ADDRESS,"auto_route":true,"strict_route":true,"stack":"mixed","mtu":1400,"dns_mode":"hijack","dns_address":super::tun::DNS}],"outbounds":[outbound,{"type":"direct","tag":"direct","domain_resolver":"local"}],"route":{"auto_detect_interface":true,"default_domain_resolver":"local","rules":rules,"rule_set":sets,"final":routing.default_exit.tag()}}),
    )
}
pub fn build(profile: &Profile, root: &Path) -> Result<Generated, String> {
    let versions = rulesets::selected(root, &profile.routing)?;
    let value = compile(profile, &versions, root)?;
    let directory = root.join("runtime");
    crate::private_files::directory(&directory)?;
    let path = directory.join("generated.json");
    settings::write_json(&path, &value)?;
    Ok(Generated {
        path,
        digest: rulesets::digest(&serde_json::to_vec(&value).unwrap()),
    })
}
pub fn remove_generated(root: &Path) -> Result<(), String> {
    match fs::remove_file(root.join("runtime/generated.json")) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err("生成配置未能清理".into()),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mixed_conditions_are_explicit_or_and_policy_order_is_preserved() {
        let root = std::env::temp_dir().join(format!(
            "sing-shell-compile-{}-{}",
            std::process::id(),
            crate::logs::now()
        ));
        let node = super::super::import::preview("trojan://secret@example.com:443", &[])
            .unwrap()
            .nodes
            .remove(0);
        let profile=Profile{selected_node:Some(node.id.clone()),nodes:vec![node],routing:serde_json::from_value(json!({"defaultExit":"proxy","policies":[{"id":"work","name":"工作","enabled":true,"exit":"direct","match":{"kind":"custom","domains":[{"value":"example.com","includeSubdomains":true}],"ipCidrs":["192.0.2.0/24"]}}]})).unwrap(),..Default::default()};
        let v = compile(&profile, &[], &root).unwrap();
        assert_eq!(v["route"]["rule_set"][0]["rules"][0]["mode"], "or");
        assert_eq!(
            v["route"]["rule_set"][0]["rules"][0]["rules"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        assert_eq!(v["route"]["rules"][3]["action"], "resolve");
        assert_eq!(v["route"]["rules"][5]["rule_set"][0], "policy-work");
        assert_eq!(v["dns"]["rules"][0]["rules"].as_array().unwrap().len(), 1);
    }
}
