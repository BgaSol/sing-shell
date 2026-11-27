use super::{
    routing::{Action, Match, Routing},
    ConnectionInput, Profile,
};
use crate::{
    rulesets::{self, Version},
    settings,
};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};
pub const FAKEIP_RANGE: &str = "198.18.0.0/15";
pub struct Generated {
    pub path: PathBuf,
    pub digest: String,
}
pub struct Bindings {
    pub rules: BTreeMap<String, PathBuf>,
    pub cache: PathBuf,
    pub activity: Option<(u16, String)>,
}
impl Bindings {
    pub fn prepare(root: &Path, versions: &[Version], cache: PathBuf) -> Result<Self, String> {
        let rules = versions
            .iter()
            .map(|v| Ok((v.source_id.clone(), rulesets::materialize_at(root, v)?)))
            .collect::<Result<_, String>>()?;
        Ok(Self {
            rules,
            cache,
            activity: None,
        })
    }
}
fn dns_route(rules: &mut Vec<Value>, condition: Value, exit: Action) {
    if exit == Action::Proxy {
        rules.push(json!({"type":"logical","mode":"and","rules":[condition.clone(),{"query_type":["A","AAAA"]}],"action":"route","server":"fakeip"}));
    }
    let mut real = condition;
    real["action"] = json!("route");
    real["server"] = json!(if exit == Action::Direct {
        "local"
    } else {
        "proxy-dns"
    });
    rules.push(real);
}
fn policies(
    routing: &Routing,
    bindings: &Bindings,
) -> Result<(Vec<Value>, Vec<Value>, Vec<Value>), String> {
    let mut routes = Vec::new();
    let mut dns = Vec::new();
    let mut sets = Vec::new();
    for policy in routing.policies.iter().filter(|p| p.enabled) {
        let tag = policy.tag();
        match &policy.r#match {
            Match::Custom { domains, ip_cidrs } => {
                let domain_items: Vec<_> = domains
                    .iter()
                    .map(|d| {
                        if d.include_subdomains {
                            json!({"domain_suffix":[d.value]})
                        } else {
                            json!({"domain":[d.value]})
                        }
                    })
                    .collect();
                let mut items = domain_items.clone();
                if !ip_cidrs.is_empty() {
                    items.push(json!({"ip_cidr":ip_cidrs}));
                }
                sets.push(json!({"type":"inline","tag":tag,"rules":[{"type":"logical","mode":"or","rules":items}]}));
                if !domain_items.is_empty() {
                    dns_route(
                        &mut dns,
                        json!({"type":"logical","mode":"or","rules":domain_items}),
                        policy.exit,
                    );
                }
            }
            Match::Source { source_id } => {
                let path = bindings
                    .rules
                    .get(source_id)
                    .ok_or_else(|| format!("{} 没有固定可用版本", policy.name))?;
                sets.push(json!({"type":"local","tag":tag,"format":"binary","path":path}));
                if rulesets::definition(source_id)?.domains {
                    dns_route(&mut dns, json!({"rule_set":[tag]}), policy.exit);
                }
            }
        }
        routes.push(json!({"rule_set":[tag],"action":"route","outbound":policy.exit.tag()}));
    }
    Ok((routes, dns, sets))
}
fn dns(input: &ConnectionInput, projected: Vec<Value>) -> Value {
    let mut rules = vec![
        json!({"type":"logical","mode":"or","rules":[{"domain":["localhost"]},{"domain_suffix":[".localhost",".local",".in-addr.arpa",".ip6.arpa"]},{"domain_regex":["^[^.]+$"]}],"action":"route","server":"local"}),
    ];
    if input.options.ipv4_compatibility {
        rules.push(json!({"query_type":["AAAA"],"action":"predefined","rcode":"NOERROR"}));
    }
    rules.extend(projected);
    if input.routing.default_exit == Action::Proxy {
        rules.push(json!({"query_type":["A","AAAA"],"action":"route","server":"fakeip"}));
    }
    json!({"servers":[
        {"type":"local","tag":"local"},
        {"type":"https","tag":"proxy-dns","server":"1.1.1.1","path":"/dns-query","tls":{"enabled":true,"server_name":"cloudflare-dns.com"},"detour":"proxy"},
        {"type":"fakeip","tag":"fakeip","inet4_range":FAKEIP_RANGE}
    ],"rules":rules,"final":if input.routing.default_exit==Action::Direct{"local"}else{"proxy-dns"}})
}
pub fn compile(input: &ConnectionInput, bindings: &Bindings) -> Result<Value, String> {
    input.validate()?;
    if input
        .node
        .spec
        .endpoint()
        .0
        .parse::<std::net::Ipv4Addr>()
        .is_ok_and(fakeip)
    {
        return Err("节点服务器地址与 Fake-IP 保留网段冲突".into());
    }
    let routing = input.routing.normalized()?;
    for policy in routing.policies.iter().filter(|p| p.enabled) {
        if let Match::Custom { ip_cidrs, .. } = &policy.r#match {
            for cidr in ip_cidrs {
                let (ip, prefix) = cidr.split_once('/').ok_or("IP 规则无效")?;
                if let Ok(ip) = ip.parse::<std::net::Ipv4Addr>() {
                    if prefix.parse::<u8>().unwrap_or(0) >= 15 && fakeip(ip) {
                        return Err(format!(
                            "{} 的真实 IP {} 与 Fake-IP 保留网段冲突，请修改该规则",
                            policy.name, cidr
                        ));
                    }
                }
            }
        }
    }
    let (policies, projected_dns, sets) = policies(&routing, bindings)?;
    let protected =
        json!({"ip_cidr":super::routing::PRIVATE_NETWORKS,"action":"route","outbound":"direct"});
    let mut rules = vec![
        json!({"action":"sniff"}),
        json!({"protocol":"dns","action":"hijack-dns"}),
        protected.clone(),
    ];
    if input.options.ipv4_compatibility {
        rules.push(json!({"ip_version":6,"action":"reject","no_drop":true}));
        rules.push(json!({"action":"resolve","strategy":"ipv4_only"}));
    } else {
        rules.push(json!({"action":"resolve"}));
    }
    rules.push(protected);
    rules.extend(policies);
    let mut outbound = input.node.spec.outbound();
    outbound["domain_resolver"] = json!("local");
    let mut experimental = json!({"cache_file":{"enabled":true,"path":bindings.cache,"cache_id":"fakeip-v1","store_fakeip":true}});
    if let Some((port, secret)) = &bindings.activity {
        experimental["clash_api"] =
            json!({"external_controller":format!("127.0.0.1:{port}"),"secret":secret});
    }
    Ok(json!({
        "log":{"level":"info","disabled":false,"timestamp":false},
        "dns":dns(input, projected_dns),
        "inbounds":[{"type":"tun","tag":"tun-in","interface_name":super::tun::INTERFACE,"address":super::tun::ADDRESS,"auto_route":true,"strict_route":true,"stack":"mixed","mtu":1400,"dns_mode":"hijack","dns_address":super::tun::DNS}],
        "outbounds":[outbound,{"type":"direct","tag":"direct","domain_resolver":"local"}],
        "route":{"auto_detect_interface":true,"default_domain_resolver":"local","rules":rules,"rule_set":sets,"final":routing.default_exit.tag()},
        "experimental":experimental
    }))
}
pub fn fakeip(ip: std::net::Ipv4Addr) -> bool {
    let [a, b, ..] = ip.octets();
    a == 198 && matches!(b, 18 | 19)
}
pub fn build(profile: &Profile, root: &Path, version: &str) -> Result<Generated, String> {
    let versions = rulesets::selected(root, &profile.routing)?;
    let node = profile.selected().ok_or("请先选择节点")?.clone();
    let input = ConnectionInput::from_profile(profile, node, versions, version.into())?;
    let directory = root.join("runtime");
    crate::private_files::directory(&directory)?;
    let bindings = Bindings::prepare(root, &input.versions, directory.join("check-cache.db"))?;
    let value = compile(&input, &bindings)?;
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
    fn compilation_preserves_policy_order_and_combines_runtime_features() {
        let node = super::super::import::preview("trojan://secret@example.com:443", &[])
            .unwrap()
            .nodes
            .remove(0);
        let input=ConnectionInput { node, routing:serde_json::from_value(json!({"defaultExit":"proxy","policies":[{"id":"work","name":"工作","enabled":true,"exit":"direct","match":{"kind":"custom","domains":[{"value":"example.com","includeSubdomains":true}],"ipCidrs":["192.0.2.0/24"]}}]})).unwrap(), versions:vec![], core_version:"1.14.4".into(), options:Default::default() };
        let mut bindings = Bindings {
            rules: Default::default(),
            cache: PathBuf::from("cache.db"),
            activity: Some((12345, "test-secret".into())),
        };
        let v = compile(&input, &bindings).unwrap();
        assert_eq!(v["route"]["rule_set"][0]["rules"][0]["mode"], "or");
        assert_eq!(v["route"]["rules"][5]["rule_set"][0], "policy-work");
        assert!(v["experimental"]["cache_file"]["store_fakeip"]
            .as_bool()
            .unwrap());
        assert_eq!(v["experimental"]["clash_api"]["secret"], "test-secret");
        bindings.activity = None;
        let mut compatible = input.clone();
        compatible.options.ipv4_compatibility = true;
        let v = compile(&compatible, &bindings).unwrap();
        assert!(v["experimental"].get("clash_api").is_none());
        assert!(v["experimental"]["cache_file"].is_object());
        assert_eq!(v["route"]["rules"][4]["strategy"], "ipv4_only");
        assert_eq!(v["outbounds"][0]["domain_resolver"], "local");
    }
}
