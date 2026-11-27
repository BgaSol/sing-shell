use sing_shell::profile::{self, Node, Profile};
use std::{
    fs,
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
fn directory() -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "sing-shell-profile-{}-{}-{}",
        std::process::id(),
        sing_shell::logs::now(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir(&path).unwrap();
    path
}
#[test]
fn explicit_selection_is_persistent_without_changing_successful_restore_input() {
    let root = directory();
    let app = sing_shell::app::Application::open(root.clone(), "0.9.0".into()).unwrap();
    app.session.initialize().unwrap();
    let preview = app
        .profiles
        .preview("trojan://private-a@example.test:443#A\ntrojan://private-b@example.test:444#B")
        .unwrap();
    app.profiles.commit(preview.ticket, 0).unwrap();
    let first = app.store.snapshot();
    let a = first.profile.nodes[0].clone();
    let b = first.profile.nodes[1].clone();
    assert!(app.session.snapshot().can_select_node);
    assert!(!app.session.snapshot().can_connect);
    assert!(app.session.start_external().is_err());
    assert!(!app.gate.runtime_busy());
    app.session
        .select_node(a.id.clone(), first.nodes_revision)
        .unwrap();
    let input = profile::ConnectionInput::from_profile(
        &first.profile,
        a.clone(),
        sing_shell::rulesets::selected(&root, &first.profile.routing).unwrap(),
        "1.14.4".into(),
    )
    .unwrap();
    app.store
        .confirm_connection(first.core_revision, Some(input.clone()))
        .unwrap();
    let selected_a = app.store.snapshot();
    app.session
        .select_node(b.id.clone(), selected_a.nodes_revision)
        .unwrap();
    let selected_b = app.store.snapshot();
    assert_eq!(selected_b.profile.selected_node.as_ref(), Some(&b.id));
    assert_eq!(selected_b.routing_revision, first.routing_revision);
    assert_eq!(selected_b.connection_revision, first.connection_revision);
    assert_eq!(
        selected_b.startup.last_connection.as_ref().unwrap().node.id,
        a.id
    );
    app.session
        .select_node(b.id.clone(), selected_b.nodes_revision)
        .unwrap();
    assert_eq!(
        app.store.snapshot().nodes_revision,
        selected_b.nodes_revision
    );
    assert!(app
        .session
        .select_node(a.id.clone(), first.nodes_revision)
        .is_err());
    assert!(app
        .session
        .select_node("missing".into(), selected_b.nodes_revision)
        .is_err());
    app.store
        .confirm_connection(first.core_revision, Some(input))
        .unwrap();
    assert_eq!(
        app.store.snapshot().profile.selected_node.as_ref(),
        Some(&b.id)
    );
    let operation = app.gate.begin(sing_shell::app::Kind::Runtime).unwrap();
    assert!(!app.session.snapshot().can_select_node);
    assert!(app
        .session
        .select_node(a.id.clone(), selected_b.nodes_revision)
        .is_err());
    drop(operation);
    let stop = app.gate.begin(sing_shell::app::Kind::Stop).unwrap();
    assert!(!app.session.snapshot().can_stop);
    assert_eq!(app.session.snapshot().phase, "stopping");
    drop(stop);
    // A failed atomic write must not publish a new in-memory selection.
    fs::rename(root.join("settings.json"), root.join("saved.json")).unwrap();
    fs::create_dir(root.join("settings.json")).unwrap();
    assert!(app
        .session
        .select_node(a.id, selected_b.nodes_revision)
        .is_err());
    assert_eq!(
        app.store.snapshot().profile.selected_node.as_ref(),
        Some(&b.id)
    );
    fs::remove_dir(root.join("settings.json")).unwrap();
    fs::rename(root.join("saved.json"), root.join("settings.json")).unwrap();
    drop(app);
    let reopened = sing_shell::settings::SettingsStore::open(root.clone()).unwrap();
    assert_eq!(
        reopened.snapshot().profile.selected_node.as_ref(),
        Some(&b.id)
    );
    drop(reopened);
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn application_drop_removes_generated_credentials_without_an_active_process() {
    let root = directory();
    let app = sing_shell::app::Application::open(root.clone(), "0.5.0".into()).unwrap();
    fs::create_dir_all(root.join("runtime")).unwrap();
    fs::write(root.join("runtime/generated.json"), b"private candidate").unwrap();
    drop(app);
    assert!(!root.join("runtime/generated.json").exists());
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn deletion_cannot_race_a_start_or_an_authorized_connection() {
    use std::sync::Arc;
    let root = directory();
    let store = Arc::new(sing_shell::settings::SettingsStore::open(root.clone()).unwrap());
    let gate = Arc::new(sing_shell::app::Admission::default());
    let runtime = Arc::new(sing_shell::runtime::RuntimeService::default());
    let service = profile::service::Service::new(store.clone(), gate.clone(), runtime.clone());
    let preview = service
        .preview("ss://YWVzLTEyOC1nY206cGFzcw@127.0.0.1:443#test")
        .unwrap();
    service.commit(preview.ticket, 0).unwrap();
    let id = store.snapshot().profile.nodes[0].id.clone();
    let revision = store.snapshot().nodes_revision;
    let starting = gate.begin(sing_shell::app::Kind::Runtime).unwrap();
    assert!(service.delete(id.clone(), revision).is_err());
    drop(starting);
    runtime.network_owned(true);
    assert!(service.delete(id.clone(), revision).is_err());
    runtime.network_owned(false);
    assert!(service.delete(id, revision).is_ok());
    drop(service);
    drop(store);
    fs::remove_dir_all(root).unwrap();
}

#[test]
#[ignore = "requires official core; measures generated routing through two controlled encrypted exits without changing system networking"]
fn official_core_routes_domains_cidrs_sites_and_default_to_expected_exit() {
    use sing_shell::{
        profile::routing::{Action, Domain, Match, Policy},
        runtime::{RuntimeService, StartSpec},
    };
    use std::{
        io::{Read, Write},
        net::{TcpListener, TcpStream},
        sync::{atomic::AtomicBool, Arc},
        thread,
        time::Duration,
    };
    let core = std::path::PathBuf::from(std::env::var_os("SING_SHELL_TEST_CORE").unwrap());
    let root = directory();
    let port = || {
        TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    };
    let proxy_port = port();
    let direct_port = port();
    let mixed_port = port();
    let controller_port = port();
    let stopped = Arc::new(AtomicBool::new(false));
    let mut echoes = Vec::new();
    let mut echo_ports = Vec::new();
    for label in [b'P', b'D'] {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        echo_ports.push(listener.local_addr().unwrap().port());
        listener.set_nonblocking(true).unwrap();
        let stop = stopped.clone();
        echoes.push(thread::spawn(move || {
            while !stop.load(Ordering::Acquire) {
                if let Ok((mut stream, _)) = listener.accept() {
                    thread::spawn(move || {
                        stream
                            .set_read_timeout(Some(Duration::from_secs(10)))
                            .unwrap();
                        let mut bytes = [0; 4];
                        while stream.read_exact(&mut bytes).is_ok() {
                            if stream.write_all(&[label]).is_err() {
                                break;
                            }
                        }
                    });
                } else {
                    thread::sleep(Duration::from_millis(10));
                }
            }
        }));
    }
    let server = RuntimeService::default();
    let client = RuntimeService::default();
    let run = |runtime: &RuntimeService, file: &str, value: &serde_json::Value| {
        let path = root.join(file);
        fs::write(&path, value.to_string()).unwrap();
        runtime
            .start(
                &StartSpec {
                    executable: core.clone(),
                    config: path,
                    version: Some(runtime.probe(&core, &AtomicBool::new(false)).unwrap()),
                },
                Arc::new(AtomicBool::new(false)),
            )
            .unwrap();
    };
    run(
        &server,
        "server.json",
        &serde_json::json!({
            "inbounds":[{"type":"shadowsocks","tag":"proxy-in","listen":"127.0.0.1","listen_port":proxy_port,"method":"aes-128-gcm","password":"test-password"},{"type":"shadowsocks","tag":"direct-in","listen":"127.0.0.1","listen_port":direct_port,"method":"aes-128-gcm","password":"test-password"}],
            "outbounds":[{"type":"direct","tag":"echo"}],
            "route":{"rules":[{"inbound":["proxy-in"],"action":"route","outbound":"echo","override_address":"127.0.0.1","override_port":echo_ports[0]},{"inbound":["direct-in"],"action":"route","outbound":"echo","override_address":"127.0.0.1","override_port":echo_ports[1]}],"final":"echo"}
        }),
    );
    let spec = |port| profile::NodeSpec::Shadowsocks {
        server: "127.0.0.1".into(),
        server_port: port,
        method: "aes-128-gcm".into(),
        password: "test-password".into(),
    };
    let mut profile = Profile::default();
    profile.nodes.push(Node {
        tags: vec![],
        id: "n".into(),
        name: "test".into(),
        source: "fixture".into(),
        spec: spec(proxy_port),
    });
    profile.selected_node = Some("n".into());
    let domain = |id: &str, value: &str, suffix: bool, exit| Policy {
        id: id.into(),
        name: id.into(),
        enabled: true,
        exit,
        r#match: Match::Custom {
            domains: vec![Domain {
                value: value.into(),
                include_subdomains: suffix,
            }],
            ip_cidrs: vec![],
        },
    };
    let ip = |id: &str, value: &str, exit| Policy {
        id: id.into(),
        name: id.into(),
        enabled: true,
        exit,
        r#match: Match::Custom {
            domains: vec![],
            ip_cidrs: vec![value.into()],
        },
    };
    profile.routing.policies = vec![
        domain("www", "www.example.test", false, Action::Proxy),
        domain("deep", "deep.example.test", true, Action::Proxy),
        domain("github-custom", "github.com", false, Action::Proxy),
        domain("example", "example.test", true, Action::Direct),
        ip("specific", "203.0.113.1/32", Action::Proxy),
        ip("range", "203.0.113.0/24", Action::Direct),
    ];
    if let Match::Custom { domains, ip_cidrs } = &mut profile.routing.policies[5].r#match {
        domains.push(Domain {
            value: "or.example.net".into(),
            include_subdomains: false,
        });
        ip_cidrs.push("2001:db8::/32".into());
    }
    for id in ["github", "google"] {
        profile.routing.policies.push(Policy {
            id: id.into(),
            name: id.into(),
            enabled: true,
            exit: if id == "github" {
                Action::Direct
            } else {
                Action::Proxy
            },
            r#match: Match::Source {
                source_id: id.into(),
            },
        });
    }
    let generated = profile::config::build(&profile, &root, "1.14.4").unwrap();
    let mut config: serde_json::Value =
        serde_json::from_slice(&fs::read(generated.path).unwrap()).unwrap();
    // Keep generated routing and SRS unchanged; replace only ingress, DNS transport and exit probes.
    config["inbounds"] =
        serde_json::json!([{"type":"mixed","listen":"127.0.0.1","listen_port":mixed_port}]);
    let mut direct = spec(direct_port).outbound();
    direct["tag"] = serde_json::json!("direct");
    config["outbounds"][1] = direct;
    let hosts = serde_json::json!({"example.test":["203.0.113.1"],"www.example.test":["203.0.113.1"],"a.example.test":["203.0.113.1"],"a.deep.example.test":["203.0.113.2"],"github.com":["198.51.100.1"],"api.github.com":["198.51.100.1"],"google.com":["198.51.100.1"],"unmatched.test":["198.51.100.1"],"or.example.net":["198.51.100.1"]});
    config["dns"]["servers"][0] =
        serde_json::json!({"type":"hosts","tag":"local","predefined":hosts});
    config["dns"]["servers"][1] =
        serde_json::json!({"type":"hosts","tag":"proxy-dns","predefined":hosts});
    config["experimental"]["clash_api"] = serde_json::json!({"external_controller":format!("127.0.0.1:{controller_port}"),"secret":"isolated-test-controller"});
    run(&client, "client.json", &config);
    let held = std::sync::Mutex::new(Vec::new());
    let request = |host: &str, expected: u8| {
        let mut stream = TcpStream::connect(("127.0.0.1", mixed_port)).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(8)))
            .unwrap();
        stream.write_all(&[5, 1, 0]).unwrap();
        let mut greeting = [0; 2];
        stream.read_exact(&mut greeting).unwrap();
        assert_eq!(greeting, [5, 0]);
        let mut address = vec![5, 1, 0];
        if let Ok(ip) = host.parse::<std::net::IpAddr>() {
            match ip {
                std::net::IpAddr::V4(ip) => {
                    address.push(1);
                    address.extend(ip.octets());
                }
                std::net::IpAddr::V6(ip) => {
                    address.push(4);
                    address.extend(ip.octets());
                }
            }
        } else {
            address.extend([3, host.len() as u8]);
            address.extend(host.as_bytes());
        }
        address.extend(80u16.to_be_bytes());
        stream.write_all(&address).unwrap();
        let mut reply = [0; 10];
        stream.read_exact(&mut reply).unwrap();
        assert_eq!(reply[1], 0, "{host}");
        stream.write_all(b"ping").unwrap();
        let mut marker = [0; 1];
        stream.read_exact(&mut marker).unwrap_or_else(|error| {
            panic!(
                "{host}: {error}; client: {}; server: {}",
                client
                    .logs
                    .snapshot()
                    .iter()
                    .map(|l| l.message.as_str())
                    .collect::<Vec<_>>()
                    .join("\n"),
                server
                    .logs
                    .snapshot()
                    .iter()
                    .map(|l| l.message.as_str())
                    .collect::<Vec<_>>()
                    .join("\n")
            )
        });
        assert_eq!(marker[0], expected, "wrong exit for {host}");
        held.lock().unwrap().push(stream);
    };
    for (host, expected) in [
        ("127.0.0.1", b'D'),
        ("198.51.100.1", b'P'),
        ("203.0.113.2", b'D'),
        ("2001:db8::5", b'D'),
        ("or.example.net", b'D'),
        ("203.0.113.1", b'P'),
        ("example.test", b'D'),
        ("a.example.test", b'D'),
        ("www.example.test", b'P'),
        ("a.deep.example.test", b'P'),
        ("github.com", b'P'),
        ("api.github.com", b'D'),
        ("google.com", b'P'),
        ("unmatched.test", b'P'),
    ] {
        request(host, expected);
    }
    let api = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    api.block_on(async {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let http = reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(3))
            .build()
            .unwrap();
        let url = format!("http://127.0.0.1:{controller_port}/connections");
        assert_eq!(http.get(&url).send().await.unwrap().status(), 401);
        let data: serde_json::Value = http
            .get(&url)
            .bearer_auth("isolated-test-controller")
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let connections = data["connections"].as_array().unwrap();
        assert!(connections.iter().any(|c| c["rule"]
            .as_str()
            .is_some_and(|r| r.starts_with("rule_set=policy-range => route("))));
        assert!(connections.iter().any(|c| c["rule"] == "final"));
        assert!(connections
            .iter()
            .any(|c| c["download"].as_u64().unwrap_or(0) > 0));
    });
    client.stop().unwrap();
    config["route"]["final"] = serde_json::json!("direct");
    run(&client, "client.json", &config);
    request("unmatched.test", b'D');
    client.stop().unwrap();
    profile.routing.policies.swap(0, 3);
    let reversed_path = profile::config::build(&profile, &root, "1.14.4")
        .unwrap()
        .path;
    let reversed: serde_json::Value =
        serde_json::from_slice(&fs::read(reversed_path).unwrap()).unwrap();
    config["route"] = reversed["route"].clone();
    run(&client, "client.json", &config);
    request("www.example.test", b'D');
    request("unmatched.test", b'P');
    client.stop().unwrap();
    held.lock().unwrap().clear();
    server.stop().unwrap();
    stopped.store(true, Ordering::Release);
    for echo in echoes {
        echo.join().unwrap();
    }
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn revisions_are_independent_and_old_settings_require_explicit_backup_reset() {
    let root = directory();
    let original = br#"{"schemaVersion":4,"private":"preserve exact bytes"}"#;
    fs::write(root.join("settings.json"), original).unwrap();
    let store = sing_shell::settings::SettingsStore::open(root.clone()).unwrap();
    assert!(store.error().is_some());
    assert!(store.profile(0, |_| Ok(())).is_err());
    assert_eq!(fs::read(root.join("settings.json")).unwrap(), original);
    let backup = store.backup_reset().unwrap();
    assert_eq!(fs::read(backup).unwrap(), original);
    assert_eq!(store.snapshot().schema_version, 5);
    store.profile(0, |_| Ok(())).unwrap();
    let mut routing = store.snapshot().profile.routing;
    routing.default_exit = profile::routing::Action::Direct;
    store.routing(0, routing).unwrap();
    assert_eq!(store.snapshot().nodes_revision, 1);
    assert_eq!(store.snapshot().routing_revision, 1);
    assert_eq!(store.snapshot().preferences_revision, 0);
    assert!(store.routing(0, Default::default()).is_err());
    drop(store);
    fs::remove_dir_all(root).unwrap();
}
#[test]
#[ignore = "requires official core in SING_SHELL_TEST_CORE; validates generated TUN configuration without starting it"]
fn official_core_checks_every_supported_protocol_and_site_assets() {
    let core = std::env::var_os("SING_SHELL_TEST_CORE").unwrap();
    let mut samples: Vec<String> = [
        "ss://YWVzLTEyOC1nY206cGFzcw@127.0.0.1:443#ss",
        "trojan://test@127.0.0.1:443#trojan",
        "vless://550e8400-e29b-41d4-a716-446655440000@127.0.0.1:443?security=tls#vless",
        "vless://550e8400-e29b-41d4-a716-446655440000@example.com:443?encryption=none&security=tls&type=ws&host=cdn.example.com&path=%2Fwsss#vless-ws",
        "vless://550e8400-e29b-41d4-a716-446655440000@example.com:443?encryption=none&security=tls&sni=tls.example.com&type=grpc&serviceName=test#vless-grpc",
        "hysteria2://test@127.0.0.1:443#hy2",
        "tuic://550e8400-e29b-41d4-a716-446655440000:test@127.0.0.1:443#tuic",
        "trojan://test@127.0.0.1:443?type=ws&path=%2Fconnect&host=example.com#ws",
        "trojan://test@127.0.0.1:443?type=grpc&serviceName=test#grpc",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    use base64::Engine;
    let vmess = serde_json::json!({"v":"2","ps":"vmess","add":"127.0.0.1","port":"443","id":"550e8400-e29b-41d4-a716-446655440000","aid":"0","net":"ws","path":"/connect","host":"example.com","tls":"tls","sni":"example.com"});
    samples.push(format!(
        "vmess://{}",
        base64::engine::general_purpose::STANDARD.encode(vmess.to_string())
    ));
    samples.push(format!("vless://550e8400-e29b-41d4-a716-446655440000@127.0.0.1:443?security=reality&sni=example.com&fp=chrome&pbk={}&sid=0123",base64::engine::general_purpose::URL_SAFE_NO_PAD.encode([1u8;32])));
    let root = directory();
    for sample in samples {
        let (name, spec) = profile::import::link(&sample).unwrap();
        let mut profile = Profile::default();
        profile.nodes = vec![Node {
            tags: vec![],
            id: "n".into(),
            name,
            source: "test".into(),
            spec,
        }];
        profile.selected_node = Some("n".into());
        profile.routing.policies = sing_shell::rulesets::CATALOG
            .iter()
            .map(|source| profile::routing::Policy {
                id: source.id.into(),
                name: source.name.into(),
                enabled: true,
                exit: profile::routing::Action::Direct,
                r#match: profile::routing::Match::Source {
                    source_id: source.id.into(),
                },
            })
            .collect();
        let config = profile::config::build(&profile, &root, "1.14.4").unwrap();
        let output = std::process::Command::new(&core)
            .args(["check", "-c"])
            .arg(config.path)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn imports_are_atomic_replay_safe_and_public_settings_hide_credentials() {
    let root = directory();
    let store =
        std::sync::Arc::new(sing_shell::settings::SettingsStore::open(root.clone()).unwrap());
    let gate = std::sync::Arc::new(sing_shell::app::Admission::default());
    let runtime = std::sync::Arc::new(sing_shell::runtime::RuntimeService::default());
    let service = profile::service::Service::new(store.clone(), gate.clone(), runtime);
    let preview = service
        .preview("trojan://private-test-password@example.com:443#first")
        .unwrap();
    service.commit(preview.ticket, 0).unwrap();
    assert!(service.commit(preview.ticket, 1).is_err());
    let first = service.snapshot();
    assert_eq!(first.profile.nodes.len(), 1);
    let id = first.profile.nodes[0].id.clone();
    let second = service
        .preview("trojan://another-password@example.com:443#second")
        .unwrap();
    assert!(service.commit(second.ticket, 0).is_err());
    service.commit(second.ticket, 1).unwrap();
    let second = service.snapshot();
    assert_ne!(id, second.profile.nodes[1].id);
    assert_eq!(store.snapshot().nodes_revision, 2);
    assert!(!store
        .snapshot()
        .public()
        .to_string()
        .contains("private-test-password"));
    assert!(!serde_json::to_string(&second)
        .unwrap()
        .contains("another-password"));
    store
        .profile(2, |p| {
            p.selected_node = Some(id);
            Ok(())
        })
        .unwrap();
    store
        .confirm_connection(
            0,
            Some(sing_shell::profile::ConnectionInput {
                options: Default::default(),
                node: store.snapshot().profile.selected().unwrap().clone(),
                routing: Default::default(),
                versions: sing_shell::rulesets::selected(&root, &store.snapshot().profile.routing)
                    .unwrap(),
                core_version: "1.14.4".into(),
            }),
        )
        .unwrap();
    assert!(!store
        .snapshot()
        .public()
        .to_string()
        .contains("private-test-password"));
    service
        .delete(first.profile.nodes[0].id.clone(), 3)
        .unwrap();
    assert!(!store.snapshot().startup.resume_core);
    assert!(store.snapshot().startup.last_connection.is_none());
    drop(service);
    drop(store);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn vless_import_survives_storage_reopen_and_generates_the_same_connection() {
    use std::sync::Arc;
    let sample = "vless://550e8400-e29b-41d4-a716-446655440000@example.com:443?encryption=none&security=tls&sni=tls.example.com&type=ws&host=cdn.example.com&path=%2Fwsss#zzz";
    let root = directory();
    let store = Arc::new(sing_shell::settings::SettingsStore::open(root.clone()).unwrap());
    let service = profile::service::Service::new(
        store.clone(),
        Arc::new(Default::default()),
        Arc::new(Default::default()),
    );
    let preview = service
        .preview(&format!("{sample}\n{sample}\nunknown://private-value"))
        .unwrap();
    assert_eq!(preview.items.len(), 3);
    assert!(preview.items[1].duplicate);
    assert!(preview.items[2].error.is_some());
    service.commit(preview.ticket, 0).unwrap();
    let snapshot = store.snapshot();
    assert_eq!(snapshot.profile.nodes.len(), 1);
    assert!(snapshot.profile.selected_node.is_none());
    store
        .profile(1, |p| {
            p.selected_node = Some(snapshot.profile.nodes[0].id.clone());
            Ok(())
        })
        .unwrap();
    let spec = snapshot.profile.nodes[0].spec.clone();
    for equivalent in [
        sample.replace("encryption=none&", ""),
        spec.outbound().to_string(),
    ] {
        let duplicate = service.preview(&equivalent).unwrap();
        assert!(duplicate.items[0].duplicate);
        service
            .commit(duplicate.ticket, store.snapshot().nodes_revision)
            .unwrap();
        assert_eq!(store.snapshot().profile.nodes.len(), 1);
    }
    let public = serde_json::to_string(&service.snapshot()).unwrap();
    assert!(!public.contains("550e8400"));
    assert!(!store.snapshot().public().to_string().contains("550e8400"));
    drop(service);
    drop(store);
    let reopened = sing_shell::settings::SettingsStore::open(root.clone()).unwrap();
    assert!(reopened.error().is_none());
    let selected = reopened.snapshot();
    assert_eq!(selected.profile.selected().unwrap().name, "zzz");
    assert!(selected.profile.selected().unwrap().spec == spec);
    let generated = profile::config::build(&selected.profile, &root, "1.14.4").unwrap();
    let value: serde_json::Value =
        serde_json::from_slice(&fs::read(generated.path).unwrap()).unwrap();
    assert_eq!(
        value["outbounds"][0]["tls"]["server_name"],
        "tls.example.com"
    );
    assert_eq!(
        value["outbounds"][0]["transport"]["headers"]["Host"],
        "cdn.example.com"
    );
    assert_eq!(value["outbounds"][0]["transport"]["path"], "/wsss");
    assert!(value["outbounds"][0].get("encryption").is_none());
    drop(reopened);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn new_previews_and_cancellation_invalidate_old_tickets_but_write_failures_allow_retry() {
    use std::sync::Arc;
    let root = directory();
    let store = Arc::new(sing_shell::settings::SettingsStore::open(root.clone()).unwrap());
    let service = profile::service::Service::new(
        store.clone(),
        Arc::new(Default::default()),
        Arc::new(Default::default()),
    );
    let link = "trojan://private-test@example.com:443";
    for failed in [
        String::new(),
        "{".into(),
        "x".repeat(profile::import::MAX_BYTES + 1),
    ] {
        let old = service.preview(link).unwrap();
        assert!(service.preview(&failed).is_err());
        assert!(service.commit(old.ticket, 0).is_err());
        assert_eq!(store.snapshot().nodes_revision, 0);
    }
    let old = service.preview(link).unwrap();
    let current = service.preview(link).unwrap();
    assert!(service.commit(old.ticket, 0).is_err());
    service.cancel();
    assert!(service.commit(current.ticket, 0).is_err());
    let current = service.preview(link).unwrap();
    assert!(service.commit(current.ticket, 1).is_err());
    assert_eq!(store.snapshot().nodes_revision, 0);
    let settings = root.join("settings.json");
    // A directory at the destination makes the atomic replacement fail on every platform.
    fs::create_dir(&settings).unwrap();
    assert!(service.commit(current.ticket, 0).is_err());
    assert!(store.snapshot().profile.nodes.is_empty());
    assert!(store.snapshot().profile.selected_node.is_none());
    assert_eq!(store.snapshot().nodes_revision, 0);
    fs::remove_dir(&settings).unwrap();
    service.commit(current.ticket, 0).unwrap();
    assert_eq!(store.snapshot().profile.nodes.len(), 1);
    assert!(service.commit(current.ticket, 1).is_err());
    drop(service);
    drop(store);
    fs::remove_dir_all(root).unwrap();
}
