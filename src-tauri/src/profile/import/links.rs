use super::{finish, NodeSpec};
use base64::{
    engine::general_purpose::{STANDARD, STANDARD_NO_PAD, URL_SAFE, URL_SAFE_NO_PAD},
    Engine,
};
use serde_json::{json, Value};
use std::collections::BTreeMap;

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
            if !bytes[i + 1..i + 3].iter().all(u8::is_ascii_hexdigit) {
                return Err("节点 URL 编码无效".into());
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
fn query(input: &str) -> Result<BTreeMap<String, String>, String> {
    let mut params = BTreeMap::new();
    for pair in input.split('&').filter(|pair| !pair.is_empty()) {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        let key = decode_percent(&key.replace('+', " "))?;
        let value = decode_percent(&value.replace('+', " "))?;
        if key.is_empty() {
            return Err("节点链接包含空参数名".into());
        }
        if params.insert(key, value).is_some() {
            return Err("节点链接包含重复参数".into());
        }
    }
    Ok(params)
}
fn insecure(value: String) -> Result<bool, String> {
    match value.as_str() {
        "1" | "true" => Ok(true),
        "0" | "false" => Ok(false),
        _ => Err("TLS 不安全开关格式无效".into()),
    }
}
fn tls(params: &mut BTreeMap<String, String>, required: bool) -> Result<Option<Value>, String> {
    let security = params
        .remove("security")
        .unwrap_or_else(|| if required { "tls" } else { "none" }.into());
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
    if !["tls", "reality"].contains(&security.as_str()) {
        return Err("不支持此 TLS 安全方式".into());
    }
    let mut result = json!({"enabled":true});
    if let Some(sni) = params.remove("sni").filter(|v| !v.is_empty()) {
        result["server_name"] = json!(sni);
    }
    if let Some(alpn) = params.remove("alpn").filter(|v| !v.is_empty()) {
        result["alpn"] = json!(alpn.split(',').collect::<Vec<_>>());
    }
    let primary = params.remove("insecure").map(insecure).transpose()?;
    let alias = params.remove("allowInsecure").map(insecure).transpose()?;
    if matches!((primary, alias), (Some(a), Some(b)) if a != b) {
        return Err("TLS 不安全开关参数冲突".into());
    }
    if primary.or(alias) == Some(true) {
        result["insecure"] = json!(true);
    }
    if let Some(fp) = params.remove("fp").filter(|v| !v.is_empty()) {
        result["utls"] = json!({"enabled":true,"fingerprint":fp});
    }
    if security == "reality" {
        result["reality"] = json!({"enabled":true,"public_key":params.remove("pbk").ok_or("Reality 缺少公钥")?,"short_id":params.remove("sid").unwrap_or_default()});
    } else if params.contains_key("pbk") || params.contains_key("sid") {
        return Err("Reality 参数需要 security=reality".into());
    }
    Ok(Some(result))
}
fn transport(params: &mut BTreeMap<String, String>) -> Result<Option<Value>, String> {
    let kind = params.remove("type").unwrap_or_else(|| "tcp".into());
    let path = params.remove("path");
    let host = params.remove("host");
    let service = params.remove("serviceName");
    match kind.as_str() {
        "tcp" | "none" => {
            if [&path, &host, &service]
                .iter()
                .any(|value| value.as_ref().is_some_and(|v| !v.is_empty()))
            {
                return Err("TCP 传输包含无法采用的附加参数".into());
            }
            Ok(None)
        }
        "ws" => {
            if service.is_some() {
                return Err("WebSocket 不接受 gRPC 服务名称".into());
            }
            let mut result = json!({"type":"ws","path":path.as_deref().unwrap_or("/")});
            if let Some(host) = host.filter(|v| !v.is_empty()) {
                result["headers"] = json!({"Host":host});
            }
            Ok(Some(result))
        }
        "grpc" => {
            if host.is_some_and(|v| !v.is_empty()) || path.is_some_and(|v| !v.is_empty()) {
                return Err("gRPC 请使用 serviceName，不支持附加 host/path".into());
            }
            Ok(Some(
                json!({"type":"grpc","service_name":service.unwrap_or_default()}),
            ))
        }
        _ => Err("首版传输仅支持 TCP、WebSocket 和 gRPC".into()),
    }
}
pub fn link(input: &str) -> Result<(String, NodeSpec), String> {
    if input.len() > 16 * 1024 {
        return Err("单条节点链接超过 16 KiB".into());
    }
    if input.chars().any(char::is_control) {
        return Err("节点链接包含控制字符".into());
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
        if let Some(t) = tls(&mut params, false)? {
            spec["tls"] = t;
        }
        if let Some(t) = transport(&mut params)? {
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
    let mut params = query(url.query().unwrap_or_default())?;
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
            if params.remove("encryption").is_some_and(|v| v != "none") {
                return Err("VLESS encryption 仅支持 none".into());
            }
            if let Some(flow) = params.remove("flow").filter(|v| !v.is_empty()) {
                spec["flow"] = json!(flow);
            }
        }
        "tuic" => {
            spec["uuid"] = json!(user);
            spec["password"] = json!(password.ok_or("TUIC 缺少密码")?);
            for key in ["congestion_control", "udp_relay_mode"] {
                if let Some(value) = params.remove(key) {
                    spec[key] = json!(value);
                }
            }
        }
        _ => unreachable!(),
    }
    if protocol != "shadowsocks" {
        if let Some(t) = tls(&mut params, protocol != "vless")? {
            spec["tls"] = t;
        }
        if ["vless", "trojan"].contains(&protocol) {
            if let Some(t) = transport(&mut params)? {
                spec["transport"] = t;
            }
        }
    }
    if protocol == "hysteria2" {
        if let Some(obfs) = params.remove("obfs") {
            spec["obfs"] = json!({"type":obfs,"password":params.remove("obfs-password").ok_or("缺少混淆密码")?});
        } else if params.contains_key("obfs-password") {
            return Err("混淆密码缺少对应的混淆方式".into());
        }
    }
    if !params.is_empty() {
        return Err("节点包含不支持的参数".into());
    }
    finish(
        url.fragment()
            .map(decode_percent)
            .transpose()?
            .unwrap_or_default(),
        spec,
    )
}
