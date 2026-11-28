pub mod bypass;
pub mod change;
pub mod config;
pub mod connection;
pub use connection::{ConnectionInput, ConnectionOptions};
pub mod import;
pub mod routing;
pub mod service;
pub mod tun;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Clone, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Utls {
    pub enabled: bool,
    pub fingerprint: String,
}
#[derive(Clone, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Reality {
    pub enabled: bool,
    pub public_key: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub short_id: String,
}
#[derive(Clone, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Tls {
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_name: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub alpn: Vec<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub insecure: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub utls: Option<Utls>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reality: Option<Reality>,
}
#[derive(Clone, Deserialize, Serialize, PartialEq)]
#[serde(tag = "type", deny_unknown_fields)]
pub enum Transport {
    #[serde(rename = "ws")]
    WebSocket {
        #[serde(default)]
        path: String,
        #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
        headers: std::collections::BTreeMap<String, String>,
    },
    #[serde(rename = "grpc")]
    Grpc { service_name: String },
}

// This closed outbound subset is also the input boundary of the authorized worker.
#[derive(Clone, Deserialize, Serialize, PartialEq)]
#[serde(tag = "type", deny_unknown_fields)]
pub enum NodeSpec {
    #[serde(rename = "shadowsocks")]
    Shadowsocks {
        server: String,
        server_port: u16,
        method: String,
        password: String,
    },
    #[serde(rename = "trojan")]
    Trojan {
        server: String,
        server_port: u16,
        password: String,
        tls: Tls,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        transport: Option<Transport>,
    },
    #[serde(rename = "vless")]
    Vless {
        server: String,
        server_port: u16,
        uuid: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        flow: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tls: Option<Tls>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        transport: Option<Transport>,
    },
    #[serde(rename = "vmess")]
    Vmess {
        server: String,
        server_port: u16,
        uuid: String,
        #[serde(default = "auto_security")]
        security: String,
        #[serde(default)]
        alter_id: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tls: Option<Tls>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        transport: Option<Transport>,
    },
    #[serde(rename = "hysteria2")]
    Hysteria2 {
        server: String,
        server_port: u16,
        password: String,
        tls: Tls,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        obfs: Option<Obfs>,
    },
    #[serde(rename = "tuic")]
    Tuic {
        server: String,
        server_port: u16,
        uuid: String,
        password: String,
        tls: Tls,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        congestion_control: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        udp_relay_mode: Option<String>,
    },
}
fn auto_security() -> String {
    "auto".into()
}
#[derive(Clone, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Obfs {
    pub r#type: String,
    pub password: String,
}

impl NodeSpec {
    pub fn secrets(&self) -> Vec<String> {
        match self {
            Self::Shadowsocks { password, .. } | Self::Trojan { password, .. } => {
                vec![password.clone()]
            }
            Self::Hysteria2 { password, obfs, .. } => std::iter::once(password.clone())
                .chain(obfs.iter().map(|o| o.password.clone()))
                .collect(),
            Self::Vless { uuid, .. } | Self::Vmess { uuid, .. } => vec![uuid.clone()],
            Self::Tuic { uuid, password, .. } => vec![uuid.clone(), password.clone()],
        }
    }
    pub fn endpoint(&self) -> (&str, u16) {
        match self {
            Self::Shadowsocks {
                server,
                server_port,
                ..
            }
            | Self::Trojan {
                server,
                server_port,
                ..
            }
            | Self::Vless {
                server,
                server_port,
                ..
            }
            | Self::Vmess {
                server,
                server_port,
                ..
            }
            | Self::Hysteria2 {
                server,
                server_port,
                ..
            }
            | Self::Tuic {
                server,
                server_port,
                ..
            } => (server, *server_port),
        }
    }
    pub fn protocol(&self) -> &'static str {
        match self {
            Self::Shadowsocks { .. } => "Shadowsocks",
            Self::Trojan { .. } => "Trojan",
            Self::Vless { .. } => "VLESS",
            Self::Vmess { .. } => "VMess",
            Self::Hysteria2 { .. } => "Hysteria 2",
            Self::Tuic { .. } => "TUIC",
        }
    }
    pub fn validate(&self) -> Result<(), String> {
        let (server, port) = self.endpoint();
        if server.is_empty()
            || server.len() > 253
            || server.chars().any(|c| {
                c.is_whitespace() || c.is_control() || matches!(c, '/' | '\\' | '@' | '#' | '?')
            })
            || port == 0
        {
            return Err("节点地址或端口无效".into());
        }
        match self {
            Self::Shadowsocks {
                method, password, ..
            } => {
                if ![
                    "aes-128-gcm",
                    "aes-256-gcm",
                    "chacha20-ietf-poly1305",
                    "2022-blake3-aes-128-gcm",
                    "2022-blake3-aes-256-gcm",
                    "2022-blake3-chacha20-poly1305",
                ]
                .contains(&method.as_str())
                    || password.is_empty()
                {
                    return Err("不支持此 Shadowsocks 加密方式或密码为空".into());
                }
            }
            Self::Vless { uuid, flow, .. } => {
                validate_uuid(uuid)?;
                if flow.as_deref().is_some_and(|f| f != "xtls-rprx-vision") {
                    return Err("不支持此 VLESS flow".into());
                }
            }
            Self::Vmess { uuid, security, .. } => {
                validate_uuid(uuid)?;
                if !["auto", "none", "zero", "aes-128-gcm", "chacha20-poly1305"]
                    .contains(&security.as_str())
                {
                    return Err("不支持此 VMess 加密方式".into());
                }
            }
            Self::Tuic {
                uuid,
                password,
                congestion_control,
                udp_relay_mode,
                ..
            } => {
                validate_uuid(uuid)?;
                if password.is_empty()
                    || congestion_control
                        .as_deref()
                        .is_some_and(|v| !["cubic", "new_reno", "bbr"].contains(&v))
                    || udp_relay_mode
                        .as_deref()
                        .is_some_and(|v| !["native", "quic"].contains(&v))
                {
                    return Err("TUIC 参数无效或不受支持".into());
                }
            }
            Self::Trojan { password, .. } | Self::Hysteria2 { password, .. }
                if password.is_empty() =>
            {
                return Err("节点密码为空".into())
            }
            _ => {}
        }
        if let Self::Hysteria2 { obfs: Some(o), .. } = self {
            if o.r#type != "salamander" || o.password.is_empty() {
                return Err("不支持此 Hysteria 2 混淆方式".into());
            }
        }
        let tls = match self {
            Self::Trojan { tls, .. } | Self::Hysteria2 { tls, .. } | Self::Tuic { tls, .. } => {
                Some(tls)
            }
            Self::Vless { tls, .. } | Self::Vmess { tls, .. } => tls.as_ref(),
            _ => None,
        };
        if let Some(t) = tls {
            if !t.enabled {
                return Err("此节点 TLS 配置未启用".into());
            }
            if t.server_name.as_ref().is_some_and(|v| {
                v.is_empty()
                    || v.len() > 253
                    || v.chars().any(|c| c.is_control() || c.is_whitespace())
            }) {
                return Err("TLS 服务器名称无效".into());
            }
            if t.alpn.len() > 8 || t.alpn.iter().any(|v| v.is_empty() || v.len() > 255) {
                return Err("ALPN 参数无效".into());
            }
            if t.utls.as_ref().is_some_and(|u| {
                !u.enabled
                    || ![
                        "chrome",
                        "firefox",
                        "edge",
                        "safari",
                        "ios",
                        "android",
                        "random",
                        "randomized",
                    ]
                    .contains(&u.fingerprint.as_str())
            }) {
                return Err("不支持此 TLS 指纹".into());
            }
            if let Some(r) = &t.reality {
                if !r.enabled
                    || r.public_key.is_empty()
                    || r.short_id.len() > 16
                    || !r.short_id.bytes().all(|b| b.is_ascii_hexdigit())
                {
                    return Err("Reality 参数无效".into());
                }
            }
        }
        let transport = match self {
            Self::Trojan { transport, .. }
            | Self::Vless { transport, .. }
            | Self::Vmess { transport, .. } => transport.as_ref(),
            _ => None,
        };
        if let Some(transport) = transport {
            match transport {
                Transport::WebSocket { path, headers } => {
                    if path.len() > 4096
                        || path.chars().any(char::is_control)
                        || headers.len() > 1
                        || headers.iter().any(|(key, value)| {
                            !key.eq_ignore_ascii_case("host")
                                || value.is_empty()
                                || value.len() > 253
                                || value.chars().any(|c| c.is_control() || c.is_whitespace())
                        })
                    {
                        return Err("WebSocket 路径或 Host 无效".into());
                    }
                }
                Transport::Grpc { service_name } => {
                    if service_name.len() > 512 || service_name.chars().any(char::is_control) {
                        return Err("gRPC 服务名称无效".into());
                    }
                }
            }
        }
        Ok(())
    }
    pub fn fingerprint(&self) -> String {
        Sha256::digest(serde_json::to_vec(self).expect("closed node serialization"))
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }
    pub fn outbound(&self) -> serde_json::Value {
        let mut value = serde_json::to_value(self).expect("closed node serialization");
        value["tag"] = serde_json::json!("proxy");
        value
    }
}
fn validate_uuid(value: &str) -> Result<(), String> {
    if value.len() != 36
        || !value.bytes().enumerate().all(|(i, b)| {
            if [8, 13, 18, 23].contains(&i) {
                b == b'-'
            } else {
                b.is_ascii_hexdigit()
            }
        })
    {
        Err("节点 UUID 格式无效".into())
    } else {
        Ok(())
    }
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Node {
    pub id: String,
    pub name: String,
    pub source: String,
    pub tags: Vec<String>,
    pub spec: NodeSpec,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeSummary {
    pub id: String,
    pub name: String,
    pub source: String,
    pub tags: Vec<String>,
    pub protocol: String,
    pub server: String,
    pub port: u16,
}
impl Node {
    pub fn summary(&self) -> NodeSummary {
        let (server, port) = self.spec.endpoint();
        NodeSummary {
            id: self.id.clone(),
            name: self.name.clone(),
            source: self.source.clone(),
            tags: self.tags.clone(),
            protocol: self.spec.protocol().into(),
            server: server.into(),
            port,
        }
    }
}

#[derive(Clone, Copy, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum Mode {
    #[default]
    Generated,
    ExternalConfig,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Profile {
    pub mode: Mode,
    pub nodes: Vec<Node>,
    pub selected_node: Option<String>,
    pub routing: routing::Routing,
    #[serde(default)]
    pub connection: ConnectionOptions,
}
impl Default for Profile {
    fn default() -> Self {
        Self {
            mode: Mode::Generated,
            nodes: vec![],
            selected_node: None,
            routing: Default::default(),
            connection: Default::default(),
        }
    }
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicProfile {
    pub mode: Mode,
    pub nodes: Vec<NodeSummary>,
    pub selected_node: Option<String>,
    pub routing: routing::Routing,
    pub connection: ConnectionOptions,
}
impl Profile {
    pub fn public(&self) -> PublicProfile {
        PublicProfile {
            mode: self.mode,
            nodes: self.nodes.iter().map(Node::summary).collect(),
            selected_node: self.selected_node.clone(),
            routing: self.routing.clone(),
            connection: self.connection,
        }
    }
}
impl Profile {
    pub fn selected(&self) -> Option<&Node> {
        self.selected_node
            .as_ref()
            .and_then(|id| self.nodes.iter().find(|n| &n.id == id))
    }
    pub fn validate(&self) -> Result<(), String> {
        if self.nodes.len() > 2000 {
            return Err("最多保存 2,000 个节点".into());
        }
        let mut ids = std::collections::BTreeSet::new();
        for node in &self.nodes {
            if node.id.is_empty()
                || !ids.insert(&node.id)
                || node.name.is_empty()
                || node.name.len() > 256
            {
                return Err("节点标识或名称无效".into());
            }
            normalize_tags(node.tags.clone())?;
            node.spec.validate()?;
        }
        if self.selected_node.is_some() && self.selected().is_none() {
            return Err("所选节点不存在".into());
        }
        self.routing.validate()
    }
}

pub fn normalize_tags(tags: Vec<String>) -> Result<Vec<String>, String> {
    if tags.len() > 20 {
        return Err("每个节点最多 20 个标签".into());
    }
    let mut out = Vec::new();
    for tag in tags {
        let tag = tag.trim();
        if tag.is_empty() {
            continue;
        }
        if tag.chars().count() > 32 || tag.chars().any(char::is_control) {
            return Err("标签最多 32 个字符，不能包含控制字符".into());
        }
        if !out.iter().any(|t| t == tag) {
            out.push(tag.to_string());
        }
    }
    Ok(out)
}
