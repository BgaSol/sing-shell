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
    let revision = store.snapshot().profile_revision;
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
        profile::routing::{Action, Rule, SiteRule},
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
                    stream
                        .set_read_timeout(Some(Duration::from_secs(5)))
                        .unwrap();
                    let mut bytes = [0; 4];
                    if stream.read_exact(&mut bytes).is_ok() {
                        let _ = stream.write_all(&[label]);
                    }
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
                    version: Some("1.14.4".into()),
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
        id: "n".into(),
        name: "test".into(),
        source: "fixture".into(),
        spec: spec(proxy_port),
    });
    profile.selected_node = Some("n".into());
    profile.routing.custom_rules = vec![
        Rule::Domain {
            value: "example.test".into(),
            include_subdomains: true,
            action: Action::Direct,
        },
        Rule::Domain {
            value: "www.example.test".into(),
            include_subdomains: false,
            action: Action::Proxy,
        },
        Rule::Domain {
            value: "deep.example.test".into(),
            include_subdomains: true,
            action: Action::Proxy,
        },
        Rule::Domain {
            value: "github.com".into(),
            include_subdomains: false,
            action: Action::Proxy,
        },
        Rule::Ip {
            value: "198.18.20.0/24".into(),
            action: Action::Direct,
        },
        Rule::Ip {
            value: "198.18.20.1/32".into(),
            action: Action::Proxy,
        },
    ];
    profile.routing.site_rules = vec![
        SiteRule {
            site: "github".into(),
            action: Action::Direct,
        },
        SiteRule {
            site: "google".into(),
            action: Action::Proxy,
        },
    ];
    let generated = profile::config::build(&profile, &root).unwrap();
    let mut config: serde_json::Value =
        serde_json::from_slice(&fs::read(generated.path).unwrap()).unwrap();
    // Keep generated routing and SRS unchanged; replace only ingress, DNS transport and exit probes.
    config["inbounds"] =
        serde_json::json!([{"type":"mixed","listen":"127.0.0.1","listen_port":mixed_port}]);
    let mut direct = spec(direct_port).outbound();
    direct["tag"] = serde_json::json!("direct");
    config["outbounds"][1] = direct;
    let hosts = serde_json::json!({"example.test":["198.18.20.1"],"www.example.test":["198.18.20.1"],"a.example.test":["198.18.20.1"],"a.deep.example.test":["198.18.20.2"],"github.com":["198.18.77.1"],"api.github.com":["198.18.77.1"],"google.com":["198.18.77.1"],"unmatched.test":["198.18.77.1"]});
    config["dns"]["servers"] = serde_json::json!([{"type":"hosts","tag":"local","predefined":hosts},{"type":"hosts","tag":"proxy-dns","predefined":hosts}]);
    run(&client, "client.json", &config);
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
        if let Ok(ip) = host.parse::<std::net::Ipv4Addr>() {
            address.push(1);
            address.extend(ip.octets());
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
        stream.read_exact(&mut marker).unwrap();
        assert_eq!(marker[0], expected, "wrong exit for {host}");
    };
    for (host, expected) in [
        ("127.0.0.1", b'D'),
        ("198.18.77.1", b'P'),
        ("198.18.20.2", b'D'),
        ("198.18.20.1", b'P'),
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
    client.stop().unwrap();
    config["route"]["final"] = serde_json::json!("direct");
    run(&client, "client.json", &config);
    request("unmatched.test", b'D');
    client.stop().unwrap();
    server.stop().unwrap();
    stopped.store(true, Ordering::Release);
    for echo in echoes {
        echo.join().unwrap();
    }
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn profile_revision_is_independent_and_migration_preserves_original_bytes() {
    let root = directory();
    let original=serde_json::json!({"schemaVersion":3,"coreRevision":0,"preferencesRevision":0,"coreSource":{"mode":"managed"},"managed":{"active":null,"previous":null,"trial":null,"blocked":null},"configPath":"C:/old/config.json","pendingConfigPath":null,"updatePreferences":{"automatic":false,"automaticCore":false,"network":{"revision":0,"fallbackEnabled":false,"disabledBuiltins":[],"customMirrors":[]}},"startup":{"restoreCoreOnLaunch":true,"resumeCore":true},"desktop":{"backgroundNoticeShown":true}}).to_string();
    fs::write(root.join("settings.json"), &original).unwrap();
    {
        let store = sing_shell::settings::SettingsStore::open(root.clone()).unwrap();
        assert!(store.error.is_none(), "{:?}", store.error);
        assert_eq!(store.snapshot().schema_version, 4);
        assert!(store.snapshot().profile.mode == profile::Mode::ExternalConfig);
        assert_eq!(
            fs::read(root.join("settings.v3.backup.json")).unwrap(),
            original.as_bytes()
        );
        store
            .profile(0, |p| {
                p.routing.default_action = profile::routing::Action::Direct;
                Ok(())
            })
            .unwrap();
        assert_eq!(store.snapshot().core_revision, 0);
        assert_eq!(store.snapshot().preferences_revision, 0);
        assert!(store.profile(0, |_| Ok(())).is_err());
    }
    fs::remove_dir_all(root).unwrap();
}
#[test]
#[ignore = "requires official core in SING_SHELL_TEST_CORE; validates generated TUN configuration without starting it"]
fn official_core_checks_every_supported_protocol_and_site_assets() {
    let core = std::env::var_os("SING_SHELL_TEST_CORE").unwrap();
    let samples = [
        "ss://YWVzLTEyOC1nY206cGFzcw@127.0.0.1:443#ss",
        "trojan://test@127.0.0.1:443#trojan",
        "vless://550e8400-e29b-41d4-a716-446655440000@127.0.0.1:443?security=tls#vless",
        "hysteria2://test@127.0.0.1:443#hy2",
        "tuic://550e8400-e29b-41d4-a716-446655440000:test@127.0.0.1:443#tuic",
    ];
    let root = directory();
    for sample in samples {
        let (name, spec) = profile::import::link(sample).unwrap();
        let mut profile = Profile::default();
        profile.nodes = vec![Node {
            id: "n".into(),
            name,
            source: "test".into(),
            spec,
        }];
        profile.selected_node = Some("n".into());
        profile.routing.site_rules = profile::routing::SITE_IDS
            .iter()
            .map(|site| profile::routing::SiteRule {
                site: (*site).into(),
                action: profile::routing::Action::Direct,
            })
            .collect();
        let config = profile::config::build(&profile, &root).unwrap();
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
    assert!(service.select("missing".into(), 2).is_err());
    assert_eq!(store.snapshot().profile_revision, 2);
    assert!(!store
        .snapshot()
        .public()
        .to_string()
        .contains("private-test-password"));
    assert!(!serde_json::to_string(&second)
        .unwrap()
        .contains("another-password"));
    service.select(id, 2).unwrap();
    store
        .confirm_connection(
            0,
            Some(sing_shell::session::tunnel::Connection {
                node: store.snapshot().profile.selected().unwrap().clone(),
                routing: Default::default(),
                rule_commit: sing_shell::rulesets::bundled_commit(),
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
