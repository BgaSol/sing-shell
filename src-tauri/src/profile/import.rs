use super::{Node, NodeSpec, NodeSummary};
mod links;
pub use links::link;
use serde::Serialize;
use serde_json::Value;

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
    let trimmed = input.trim();
    let mut parsed = Vec::new();
    if trimmed.starts_with('{') || trimmed.starts_with('[') {
        let value: Value = serde_json::from_str(trimmed)
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
                    tags: vec![],
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
