use sing_shell::profile::{import, Node};

const VLESS: &str = "vless://550e8400-e29b-41d4-a716-446655440000@example.com:443";

fn outbound(query: &str) -> serde_json::Value {
    import::link(&format!("{VLESS}?{query}"))
        .unwrap()
        .1
        .outbound()
}

#[test]
fn vless_none_is_a_share_declaration_and_preserves_connection_fields() {
    let query = "security=tls&type=ws&host=cdn.example.com&path=%2Fwsss";
    let plain = import::link(&format!("{VLESS}?{query}#zzz")).unwrap();
    let explicit = import::link(&format!("{VLESS}?encryption=none&{query}#zzz")).unwrap();
    assert_eq!(explicit.0, "zzz");
    assert_eq!(plain.1.fingerprint(), explicit.1.fingerprint());
    let value = explicit.1.outbound();
    assert_eq!(value["server"], "example.com");
    assert_eq!(value["server_port"], 443);
    assert_eq!(value["uuid"], "550e8400-e29b-41d4-a716-446655440000");
    assert_eq!(value["tls"]["enabled"], true);
    assert!(value["tls"].get("server_name").is_none());
    assert_eq!(value["transport"]["headers"]["Host"], "cdn.example.com");
    assert_eq!(value["transport"]["path"], "/wsss");
    assert!(value.get("encryption").is_none());
    let explicit_sni = outbound(&format!("{query}&sni=tls.example.com"));
    assert_eq!(explicit_sni["tls"]["server_name"], "tls.example.com");
    assert_eq!(explicit_sni["transport"], value["transport"]);
    for encryption in ["", "auto", "aes-128-gcm"] {
        assert_eq!(
            import::link(&format!("{VLESS}?encryption={encryption}"))
                .err()
                .unwrap(),
            "VLESS encryption 仅支持 none"
        );
    }
    assert!(import::link("trojan://test@example.com:443?encryption=none").is_err());
}

#[test]
fn query_decoding_is_strict_once_and_keeps_form_plus_semantics() {
    for (encoded, expected) in [
        ("%2Fwsss", "/wsss"),
        ("%252F", "%2F"),
        ("a%2Bb+c", "a+b c"),
        ("%2Fa%26b%3Dc%3Fq%23fragment", "/a&b=c?q#fragment"),
        ("%2F%E4%B8%AD%E6%96%87", "/中文"),
    ] {
        assert_eq!(
            outbound(&format!("type=ws&path={encoded}"))["transport"]["path"],
            expected
        );
    }
    for query in [
        "type=ws&path=%",
        "type=ws&path=%2",
        "type=ws&path=%ZZ",
        "type=ws&path=%FF",
        "type=ws&path=%C0%AF",
        "%FF=x",
        "type=ws&%74ype=ws",
        "encryption=none&encryption=none",
        "=value",
    ] {
        assert!(
            import::link(&format!("{VLESS}?{query}")).is_err(),
            "accepted query: {query}"
        );
    }
    assert!(outbound("&&type=ws&&path=%2F&")["transport"]["path"] == "/");
    for control in ['\t', '\n', '\r', '\0'] {
        assert!(import::link(&format!("{VLESS}?type=ws&path=/a{control}b")).is_err());
    }
    assert!(
        import::link(r"vless\://550e8400-e29b-41d4-a716-446655440000@example.com:443").is_err()
    );
    assert!(import::link(&format!("{VLESS}/address-path?type=ws")).is_err());
    assert!(import::link("trojan://secret%+1@example.com:443").is_err());
    assert!(import::link(&format!("{VLESS}#name%+1")).is_err());
}

#[test]
fn existing_protocols_keep_credentials_tls_and_transport() {
    use base64::Engine;
    let legacy_ss =
        base64::engine::general_purpose::STANDARD.encode("aes-128-gcm:p\tass@example.com:443");
    assert!(import::link(&format!("ss://{legacy_ss}")).is_err());
    for (link, protocol) in [
        ("ss://YWVzLTEyOC1nY206cDphc3M@[2001:db8::1]:443#ss", "Shadowsocks"),
        ("trojan://p%3Aass@example.com?type=ws&path=%2Fws&host=cdn.example.com", "Trojan"),
        ("hysteria2://test@example.com?obfs=salamander&obfs-password=obfs-test&alpn=h3", "Hysteria 2"),
        ("tuic://550e8400-e29b-41d4-a716-446655440000:test@example.com?congestion_control=bbr&udp_relay_mode=native", "TUIC"),
    ] {
        let (_, node) = import::link(link).unwrap();
        assert_eq!(node.protocol(), protocol);
        assert_eq!(node.endpoint().1, 443);
    }
    let vmess = serde_json::json!({"v":"2","ps":"vmess","add":"example.com","port":"443","id":"550e8400-e29b-41d4-a716-446655440000","aid":"0","net":"ws","path":"/ws","host":"cdn.example.com","tls":"tls","sni":"tls.example.com"});
    let sample = format!(
        "vmess://{}",
        base64::engine::general_purpose::STANDARD.encode(vmess.to_string())
    );
    let (_, node) = import::link(&sample).unwrap();
    assert_eq!(node.outbound()["tls"]["server_name"], "tls.example.com");
    assert_eq!(
        node.outbound()["transport"]["headers"]["Host"],
        "cdn.example.com"
    );
    assert_eq!(node.outbound()["transport"]["path"], "/ws");
    let reality = outbound("security=reality&sni=example.com&fp=chrome&pbk=test-public-key&sid=0123&flow=xtls-rprx-vision");
    assert_eq!(reality["tls"]["reality"]["short_id"], "0123");
    assert_eq!(reality["flow"], "xtls-rprx-vision");
}

#[test]
fn tls_aliases_are_both_validated_and_must_agree() {
    for (query, insecure) in [
        ("insecure=1", true),
        ("allowInsecure=true", true),
        ("insecure=1&allowInsecure=true", true),
        ("insecure=0&allowInsecure=false", false),
    ] {
        assert_eq!(
            outbound(&format!("security=tls&{query}"))["tls"]
                .get("insecure")
                .is_some(),
            insecure
        );
    }
    for query in [
        "security=tls&insecure=0&allowInsecure=1",
        "security=tls&insecure=1&allowInsecure=false",
        "security=tls&insecure=0&allowInsecure=invalid",
        "security=tls&insecure=invalid&allowInsecure=0",
        "security=tls&insecure=",
        "security=none&allowInsecure=0",
        "security=none&sni=example.com",
        "security=tls&pbk=public",
        "security=reality",
    ] {
        assert!(
            import::link(&format!("{VLESS}?{query}")).is_err(),
            "accepted query: {query}"
        );
    }
}

#[test]
fn transport_capabilities_and_empty_fields_keep_existing_boundaries() {
    for query in ["", "type=tcp", "type=none&path=&host=&serviceName="] {
        assert!(outbound(query).get("transport").is_none());
    }
    assert_eq!(outbound("type=ws")["transport"]["path"], "/");
    assert_eq!(outbound("type=ws&path=&host=")["transport"]["path"], "");
    assert_eq!(
        outbound("type=grpc&serviceName=test&host=&path=")["transport"]["service_name"],
        "test"
    );
    for query in [
        "type=tcp&path=/x",
        "type=tcp&host=example.com",
        "type=tcp&serviceName=test",
        "type=ws&serviceName=",
        "type=grpc&path=/x",
        "type=grpc&host=example.com",
        "type=",
        "type=http",
        "flow=unsupported",
    ] {
        assert!(
            import::link(&format!("{VLESS}?{query}")).is_err(),
            "accepted query: {query}"
        );
    }
    for link in [
        "ss://YWVzLTEyOC1nY206cGFzcw@host:443?security=tls",
        "hysteria2://test@example.com:443?type=ws",
        "tuic://550e8400-e29b-41d4-a716-446655440000:test@example.com:443?host=example.com",
    ] {
        assert!(import::link(link).is_err());
    }
}

#[test]
fn preview_deduplicates_share_declarations_and_keeps_source_lines_and_safe_errors() {
    let plain = format!("{VLESS}?security=tls&type=ws&path=%2Fx#first");
    let explicit = format!("{VLESS}?encryption=none&security=tls&type=ws&path=%2Fx#second");
    let input = format!("\n\n{plain}\n{explicit}\n{VLESS}?secret-key=secret-value");
    let preview = import::preview(&input, &[]).unwrap();
    assert_eq!(preview.nodes.len(), 1);
    assert_eq!(preview.items[0].line, 3);
    assert_eq!(preview.items[1].line, 4);
    assert!(preview.items[1].duplicate);
    let public = serde_json::to_string(&preview.items).unwrap();
    for secret in [
        "secret-key",
        "secret-value",
        "550e8400-e29b-41d4-a716-446655440000",
    ] {
        assert!(!public.contains(secret));
    }
    let node = Node {
        tags: vec![],
        id: "existing".into(),
        name: "existing".into(),
        source: "test".into(),
        spec: import::link(&plain).unwrap().1,
    };
    assert!(import::preview(&explicit, &[node.clone()]).unwrap().items[0].duplicate);
    assert!(
        import::preview(&node.spec.outbound().to_string(), &[node.clone()])
            .unwrap()
            .items[0]
            .duplicate
    );
    let mut invalid_json = node.spec.outbound();
    invalid_json["encryption"] = serde_json::json!("none");
    assert!(import::preview(&invalid_json.to_string(), &[])
        .unwrap()
        .items[0]
        .error
        .is_some());
}
