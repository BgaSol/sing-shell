use super::{Node, NodeSpec, NodeSummary};
use base64::{
    engine::general_purpose::{STANDARD, STANDARD_NO_PAD, URL_SAFE, URL_SAFE_NO_PAD},
    Engine,
};
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::BTreeMap;

pub const MAX_BYTES: usize = 2 * 1024 * 1024;
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportItem {
    pub line: usize,
    pub node: Option<NodeSummary>,
    pub error: Option<String>,
    pub duplicate: bool,
}
pub struct Preview {
    pub nodes: Vec<Node>,
    pub items: Vec<ImportItem>,
}

fn decode_base64(value: &str) -> Result<String, String> {
    for engine in [&STANDARD, &STANDARD_NO_PAD, &URL_SAFE, &URL_SAFE_NO_PAD] {
        if let Ok(bytes) = engine.decode(value) {
            return String::from_utf8(bytes).map_err(|_| "节点内容不是 UTF-8 文本".into());
        }
    }
    Err("节点 Base64 格式无效".into())
}
fn decode_percent(value: &str) -> Result<String, String> {
    let mut result = Vec::with_capacity(value.len());
    let bytes = value.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if i + 2 >= bytes.len() {
                return Err("节点 URL 编码不完整".into());
            }
            let text =
                std::str::from_utf8(&bytes[i + 1..i + 3]).map_err(|_| "节点 URL 编码无效")?;
            result.push(u8::from_str_radix(text, 16).map_err(|_| "节点 URL 编码无效")?);
            i += 3;
        } else {
            result.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(result).map_err(|_| "节点文本编码无效".into())
}
fn text(value: &Value, key: &str) -> String {
    value[key].as_str().map(str::to_owned).unwrap_or_else(|| {
        value[key]
            .as_u64()
            .map(|v| v.to_string())
            .unwrap_or_default()
    })
}
fn tls(params: &BTreeMap<String, String>, required: bool) -> Result<Option<Value>, String> {
    let security = params
        .get("security")
        .map(String::as_str)
        .unwrap_or(if required { "tls" } else { "none" });
    if security == "none" || security.is_empty() {
        if [
            "sni",
            "alpn",
            "fp",
            "pbk",
            "sid",
            "insecure",
            "allowInsecure",
        ]
        .iter()
        .any(|key| params.contains_key(*key))
        {
            return Err("未启用 TLS，却包含 TLS 参数".into());
        }
        return Ok(None);
    }
    if !["tls", "reality"].contains(&security) {
        return Err("不支持此 TLS 安全方式".into());
    }
    let mut result = json!({"enabled":true});
    if let Some(sni) = params.get("sni").filter(|v| !v.is_empty()) {
        result["server_name"] = json!(sni);
    }
    if let Some(alpn) = params.get("alpn").filter(|v| !v.is_empty()) {
        result["alpn"] = json!(alpn.split(',').collect::<Vec<_>>());
    }
    let insecure = params
        .get("insecure")
        .or_else(|| params.get("allowInsecure"));
    if let Some(value) = insecure {
        match value.as_str() {
            "1" | "true" => result["insecure"] = json!(true),
            "0" | "false" => {}
            _ => return Err("TLS 不安全开关格式无效".into()),
        }
    }
    if let Some(fp) = params.get("fp").filter(|v| !v.is_empty()) {
        result["utls"] = json!({"enabled":true,"fingerprint":fp});
    }
    if security == "reality" {
        result["reality"] = json!({"enabled":true,"public_key":params.get("pbk").ok_or("Reality 缺少公钥")?,"short_id":params.get("sid").map(String::as_str).unwrap_or("")});
    } else if params.contains_key("pbk") || params.contains_key("sid") {
        return Err("Reality 参数需要 security=reality".into());
    }
    Ok(Some(result))
}
fn transport(params: &BTreeMap<String, String>) -> Result<Option<Value>, String> {
    match params.get("type").map(String::as_str).unwrap_or("tcp") {
        "tcp" | "none" => {
            if ["path", "host", "serviceName"]
                .iter()
                .any(|key| params.get(*key).is_some_and(|v| !v.is_empty()))
            {
                return Err("TCP 传输包含无法采用的附加参数".into());
            }
            Ok(None)
        }
        "ws" => {
            if params.contains_key("serviceName") {
                return Err("WebSocket 不接受 gRPC 服务名称".into());
            }
            let mut result =
                json!({"type":"ws","path":params.get("path").map(String::as_str).unwrap_or("/")});
            if let Some(host) = params.get("host").filter(|v| !v.is_empty()) {
                result["headers"] = json!({"Host":host});
            }
            Ok(Some(result))
        }
        "grpc" => {
            if params.get("host").is_some_and(|v| !v.is_empty())
                || params.get("path").is_some_and(|v| !v.is_empty())
            {
                return Err("gRPC 请使用 serviceName，不支持附加 host/path".into());
            }
            Ok(Some(
                json!({"type":"grpc","service_name":params.get("serviceName").map(String::as_str).unwrap_or("")}),
            ))
        }
        _ => Err("首版传输仅支持 TCP、WebSocket 和 gRPC".into()),
    }
}
pub fn link(input: &str) -> Result<(String, NodeSpec), String> {
    if input.len() > 16 * 1024 {
        return Err("单条节点链接超过 16 KiB".into());
    }
    if let Some(value) = input.strip_prefix("vmess://") {
        let value: Value =
            serde_json::from_str(&decode_base64(value)?).map_err(|_| "VMess 节点 JSON 无效")?;
        let object = value.as_object().ok_or("VMess 节点不是对象")?;
        for key in object.keys() {
            if ![
                "v", "ps", "add", "port", "id", "aid", "scy", "net", "type", "host", "path", "tls",
                "sni", "alpn", "fp",
            ]
            .contains(&key.as_str())
            {
                return Err("VMess 包含不支持的字段".into());
            }
        }
        if !["", "none"].contains(&text(&value, "type").as_str()) {
            return Err("不支持此 VMess 伪装类型".into());
        }
        let mut params = BTreeMap::new();
        for (key, to) in [
            ("net", "type"),
            ("host", "host"),
            ("path", "path"),
            ("sni", "sni"),
            ("alpn", "alpn"),
            ("fp", "fp"),
            ("tls", "security"),
        ] {
            let s = text(&value, key);
            if !s.is_empty() {
                params.insert(to.into(), s);
            }
        }
        let mut spec = json!({"type":"vmess","server":text(&value,"add"),"server_port":text(&value,"port").parse::<u16>().map_err(|_|"节点端口无效")?,"uuid":text(&value,"id"),"security":if text(&value,"scy").is_empty(){"auto".into()}else{text(&value,"scy")},"alter_id":if text(&value,"aid").is_empty(){0}else{text(&value,"aid").parse::<u32>().map_err(|_|"VMess alter_id 无效")?}});
        if let Some(t) = tls(&params, false)? {
            spec["tls"] = t;
        }
        if let Some(t) = transport(&params)? {
            spec["transport"] = t;
        }
        return finish(text(&value, "ps"), spec);
    }
    let mut input = input.to_string();
    if let Some(value) = input.strip_prefix("ss://") {
        let main = value.split('#').next().unwrap_or(value);
        if !main.contains('@') {
            let (value, fragment) = value.split_once('#').unwrap_or((value, ""));
            input = format!("ss://{}#{}", decode_base64(value)?, fragment);
        }
    }
    let url = reqwest::Url::parse(&input).map_err(|_| "节点链接格式无效")?;
    if !["", "/"].contains(&url.path()) {
        return Err("节点地址不能包含路径，请使用传输参数 path".into());
    }
    let protocol = match url.scheme() {
        "ss" => "shadowsocks",
        "trojan" => "trojan",
        "vless" => "vless",
        "hy2" | "hysteria2" => "hysteria2",
        "tuic" => "tuic",
        _ => return Err("不支持此节点链接协议".into()),
    };
    let server = url
        .host_str()
        .ok_or("节点缺少服务器地址")?
        .trim_start_matches('[')
        .trim_end_matches(']');
    let port = url
        .port()
        .or(if ["trojan", "hysteria2", "tuic"].contains(&protocol) {
            Some(443)
        } else {
            None
        })
        .ok_or("节点缺少端口")?;
    let mut params = BTreeMap::new();
    for (key, value) in url.query_pairs() {
        if params.insert(key.to_string(), value.to_string()).is_some() {
            return Err("节点链接包含重复参数".into());
        }
    }
    let common = [
        "security",
        "sni",
        "alpn",
        "insecure",
        "allowInsecure",
        "fp",
        "pbk",
        "sid",
        "type",
        "path",
        "host",
        "serviceName",
    ];
    for key in params.keys() {
        if !(common.contains(&key.as_str())
            || protocol == "vless" && key == "flow"
            || protocol == "hysteria2" && ["obfs", "obfs-password"].contains(&key.as_str())
            || protocol == "tuic"
                && ["congestion_control", "udp_relay_mode"].contains(&key.as_str()))
        {
            return Err("节点包含不支持的参数".into());
        }
    }
    if protocol == "shadowsocks" && !params.is_empty() {
        return Err("首版 Shadowsocks 不支持插件或附加参数".into());
    }
    let user = decode_percent(url.username())?;
    let password = url.password().map(decode_percent).transpose()?;
    let mut spec = json!({"type":protocol,"server":server,"server_port":port});
    match protocol {
        "shadowsocks" => {
            let auth = if let Some(password) = password {
                format!("{user}:{password}")
            } else {
                decode_base64(&user)?
            };
            let (method, password) = auth
                .split_once(':')
                .ok_or("Shadowsocks 缺少加密方式或密码")?;
            spec["method"] = json!(method);
            spec["password"] = json!(password);
        }
        "trojan" | "hysteria2" => {
            spec["password"] = json!(if let Some(password) = password {
                format!("{user}:{password}")
            } else {
                user
            });
        }
        "vless" => {
            if password.is_some() {
                return Err("VLESS 链接不接受密码字段".into());
            }
            spec["uuid"] = json!(user);
            if let Some(flow) = params.get("flow").filter(|v| !v.is_empty()) {
                spec["flow"] = json!(flow);
            }
        }
        "tuic" => {
            spec["uuid"] = json!(user);
            spec["password"] = json!(password.ok_or("TUIC 缺少密码")?);
            for key in ["congestion_control", "udp_relay_mode"] {
                if let Some(value) = params.get(key) {
                    spec[key] = json!(value);
                }
            }
        }
        _ => unreachable!(),
    }
    if protocol != "shadowsocks" {
        if let Some(t) = tls(&params, protocol != "vless")? {
            spec["tls"] = t;
        }
        if ["vless", "trojan"].contains(&protocol) {
            if let Some(t) = transport(&params)? {
                spec["transport"] = t;
            }
        } else if common
            .iter()
            .filter(|k| ["type", "path", "host", "serviceName"].contains(k))
            .any(|k| params.contains_key(*k))
        {
            return Err("此协议不支持传输层附加参数".into());
        }
    }
    if protocol == "hysteria2" {
        if let Some(obfs) = params.get("obfs") {
            spec["obfs"] =
                json!({"type":obfs,"password":params.get("obfs-password").ok_or("缺少混淆密码")?});
        } else if params.contains_key("obfs-password") {
            return Err("混淆密码缺少对应的混淆方式".into());
        }
    }
    finish(
        url.fragment()
            .map(decode_percent)
            .transpose()?
            .unwrap_or_default(),
        spec,
    )
}
fn finish(name: String, value: Value) -> Result<(String, NodeSpec), String> {
    let spec: NodeSpec = serde_json::from_value(value)
        .map_err(|_| "节点包含不支持的字段、缺少字段或格式错误".to_string())?;
    spec.validate()?;
    let name = if name.trim().is_empty() {
        format!("{} · {}", spec.protocol(), spec.endpoint().0)
    } else {
        name.trim().to_string()
    };
    if name.len() > 256 || name.chars().any(char::is_control) {
        return Err("节点名称过长或包含控制字符".into());
    }
    Ok((name, spec))
}
pub fn preview(input: &str, existing: &[Node]) -> Result<Preview, String> {
    if input.len() > MAX_BYTES {
        return Err("导入内容超过 2 MiB".into());
    }
    let input = input.trim();
    let mut parsed = Vec::new();
    if input.starts_with('{') || input.starts_with('[') {
        let value: Value = serde_json::from_str(input)
            .map_err(|_| "JSON 无效；JSONC 请使用文件导入并等待核心格式化")?;
        let entries = if let Some(outbounds) = value.get("outbounds") {
            outbounds.as_array().ok_or("outbounds 不是列表")?.clone()
        } else if let Some(array) = value.as_array() {
            array.clone()
        } else {
            vec![value]
        };
        for (i, mut entry) in entries.into_iter().enumerate() {
            let name = entry
                .as_object_mut()
                .and_then(|o| o.remove("tag"))
                .and_then(|v| v.as_str().map(str::to_owned))
                .unwrap_or_default();
            parsed.push((i + 1, finish(name, entry)));
        }
    } else {
        for (i, line) in input.lines().enumerate() {
            let line = line.trim();
            if !line.is_empty() {
                parsed.push((i + 1, link(line)));
            }
        }
    }
    if parsed.len() > 500 {
        return Err("单次最多导入 500 个节点".into());
    }
    if parsed.is_empty() {
        return Err("请输入节点链接或节点 JSON".into());
    }
    let mut fingerprints: std::collections::BTreeSet<_> =
        existing.iter().map(|n| n.spec.fingerprint()).collect();
    let mut result = Preview {
        nodes: Vec::new(),
        items: Vec::new(),
    };
    for (line, entry) in parsed {
        match entry {
            Ok((name, spec)) => {
                let duplicate = !fingerprints.insert(spec.fingerprint());
                let node = Node {
                    id: format!("import-{line}"),
                    name,
                    source: "本地导入".into(),
                    spec,
                };
                result.items.push(ImportItem {
                    line,
                    node: Some(node.summary()),
                    error: None,
                    duplicate,
                });
                if !duplicate {
                    result.nodes.push(node);
                }
            }
            Err(error) => result.items.push(ImportItem {
                line,
                node: None,
                error: Some(error),
                duplicate: false,
            }),
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_sip002_ipv6_and_credentials_without_loss() {
        let (name, node) = link("ss://YWVzLTEyOC1nY206cDphc3M@[2001:db8::1]:443#test").unwrap();
        assert_eq!(name, "test");
        assert_eq!(node.endpoint(), ("2001:db8::1", 443));
        assert_eq!(node.outbound()["password"], "p:ass");
        assert!(link("ss://YWVzLTEyOC1nY206cGFzcw@host:443?plugin=x").is_err());
    }
    #[test]
    fn keeps_tls_and_transport_and_rejects_unknown_fields() {
        let (_,node)=link("vless://550e8400-e29b-41d4-a716-446655440000@example.com:443?security=tls&sni=example.com&type=ws&path=%2Fx&host=cdn.example.com#node").unwrap();
        assert_eq!(node.outbound()["transport"]["path"], "/x");
        assert_eq!(
            node.outbound()["transport"]["headers"]["Host"],
            "cdn.example.com"
        );
        let input = r#"[{"type":"trojan","server":"a","server_port":443,"password":"secret","tls":{"enabled":true,"certificate_path":"/tmp/key"}}]"#;
        assert!(preview(input, &[]).unwrap().items[0].error.is_some());
    }
    #[test]
    fn preview_deduplicates_and_does_not_echo_invalid_secrets() {
        let link = "trojan://test-secret@example.com:443#n";
        let p = preview(&format!("{link}\n{link}\nunknown://secret-value"), &[]).unwrap();
        assert_eq!(p.nodes.len(), 1);
        assert!(p.items[1].duplicate);
        let serialized = serde_json::to_string(&p.items).unwrap();
        assert!(!serialized.contains("test-secret"));
        assert!(!serialized.contains("secret-value"));
    }
}
