#![cfg(feature = "test-fixtures")]
use sing_shell::{
    profile::{self, ConnectionInput, ConnectionOptions},
    runtime::{RuntimeService, StartSpec},
};
use std::{
    fs,
    io::{Read, Write},
    net::{IpAddr, Ipv4Addr, TcpListener, TcpStream, UdpSocket},
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
    time::Duration,
};

fn port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}
fn dns(port: u16, kind: u16) -> Vec<IpAddr> {
    let mut message = vec![0, 1, 1, 0, 0, 1, 0, 0, 0, 0, 0, 0];
    for name in ["probe", "invalid"] {
        message.push(name.len() as u8);
        message.extend(name.as_bytes());
    }
    message.push(0);
    message.extend(kind.to_be_bytes());
    message.extend(1u16.to_be_bytes());
    let question_end = message.len();
    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(4)))
        .unwrap();
    socket.send_to(&message, ("127.0.0.1", port)).unwrap();
    let mut bytes = [0; 4096];
    let size = socket.recv(&mut bytes).unwrap();
    let bytes = &bytes[..size];
    assert_eq!(bytes[3] & 15, 0, "DNS must return success");
    let count = u16::from_be_bytes([bytes[6], bytes[7]]);
    let mut offset = question_end;
    let mut addresses = Vec::new();
    for _ in 0..count {
        while bytes[offset] != 0 {
            if bytes[offset] & 192 == 192 {
                offset += 1;
                break;
            }
            offset += bytes[offset] as usize + 1;
        }
        offset += 1;
        let kind = u16::from_be_bytes([bytes[offset], bytes[offset + 1]]);
        let length = u16::from_be_bytes([bytes[offset + 8], bytes[offset + 9]]) as usize;
        offset += 10;
        if kind == 1 {
            addresses.push(IpAddr::V4(Ipv4Addr::new(
                bytes[offset],
                bytes[offset + 1],
                bytes[offset + 2],
                bytes[offset + 3],
            )));
        }
        offset += length;
    }
    addresses
}
fn socks(port: u16, target: IpAddr) -> TcpStream {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    stream
        .set_write_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    stream.write_all(&[5, 1, 0]).unwrap();
    let mut greeting = [0; 2];
    stream.read_exact(&mut greeting).unwrap();
    assert_eq!(greeting, [5, 0]);
    let mut command = vec![5, 1, 0];
    match target {
        IpAddr::V4(ip) => {
            command.push(1);
            command.extend(ip.octets());
        }
        IpAddr::V6(ip) => {
            command.push(4);
            command.extend(ip.octets());
        }
    }
    command.extend(443u16.to_be_bytes());
    stream.write_all(&command).unwrap();
    let mut response = [0; 4];
    stream.read_exact(&mut response).unwrap();
    assert_eq!(response[1], 0);
    let mut bound = vec![0; if response[3] == 4 { 18 } else { 6 }];
    stream.read_exact(&mut bound).unwrap();
    stream
}
fn tls(stream: TcpStream, config: Arc<rustls::ClientConfig>) -> std::io::Result<()> {
    let client =
        rustls::ClientConnection::new(config, "probe.invalid".try_into().unwrap()).unwrap();
    let mut stream = rustls::StreamOwned::new(client, stream);
    stream.write_all(b"x")?;
    let mut response = [0; 2];
    stream.read_exact(&mut response)?;
    assert_eq!(response, *b"ok");
    Ok(())
}

#[test]
#[ignore = "requires official sing-box; loopback DNS/Fake-IP/TLS/IPv6-node/cache integration without system networking"]
fn official_core_restores_domains_uses_real_ipv4_and_keeps_maps_across_reconnect() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    // The test runner may inherit a pseudoconsole with no visible window.
    #[cfg(windows)]
    unsafe {
        windows_sys::Win32::System::Console::FreeConsole();
    }
    sing_shell::prepare_network_test_processes().unwrap();
    let core = PathBuf::from(std::env::var_os("SING_SHELL_TEST_CORE").unwrap());
    let root = std::env::temp_dir().join(format!(
        "sing-shell-fakeip-{}-{}",
        std::process::id(),
        sing_shell::logs::now()
    ));
    fs::create_dir(&root).unwrap();
    let certificate = rustls::pki_types::CertificateDer::from(
        include_bytes!("fixtures/fakeip-cert.der").to_vec(),
    );
    let key = rustls::pki_types::PrivatePkcs8KeyDer::from(
        include_bytes!("fixtures/fakeip-key.der").to_vec(),
    );
    let tls_config = Arc::new(
        rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(vec![certificate.clone()], key.into())
            .unwrap(),
    );
    let mut roots = rustls::RootCertStore::empty();
    roots.add(certificate).unwrap();
    let client_tls = Arc::new(
        rustls::ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth(),
    );
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let tls_port = listener.local_addr().unwrap().port();
    listener.set_nonblocking(true).unwrap();
    let stopped = Arc::new(AtomicBool::new(false));
    let stop = stopped.clone();
    let echo = thread::spawn(move || {
        while !stop.load(Ordering::Acquire) {
            if let Ok((stream, _)) = listener.accept() {
                let config = tls_config.clone();
                thread::spawn(move || {
                    stream.set_nonblocking(false).unwrap();
                    stream
                        .set_read_timeout(Some(Duration::from_secs(5)))
                        .unwrap();
                    let connection = rustls::ServerConnection::new(config).unwrap();
                    let mut tls = rustls::StreamOwned::new(connection, stream);
                    let mut byte = [0];
                    let received = tls.read_exact(&mut byte);
                    if received.is_ok() {
                        let _ = tls.write_all(b"ok");
                        tls.conn.send_close_notify();
                        let _ = tls.flush();
                    } else {
                        eprintln!("fixture TLS read: {received:?}");
                    }
                });
            } else {
                thread::sleep(Duration::from_millis(10));
            }
        }
    });
    let bad = TcpListener::bind("127.0.0.1:0").unwrap();
    let bad_port = bad.local_addr().unwrap().port();
    bad.set_nonblocking(true).unwrap();
    let stop = stopped.clone();
    let failed_tls = thread::spawn(move || {
        while !stop.load(Ordering::Acquire) {
            if let Ok((stream, _)) = bad.accept() {
                drop(stream);
            } else {
                thread::sleep(Duration::from_millis(10));
            }
        }
    });
    let node_port = port();
    tls(
        TcpStream::connect(("127.0.0.1", tls_port)).unwrap(),
        client_tls.clone(),
    )
    .expect("direct fixture TLS");
    let mixed_port = port();
    let dns_port = port();
    let server = RuntimeService::default();
    let client = RuntimeService::default();
    let cancel = Arc::new(AtomicBool::new(false));
    let run = |runtime: &RuntimeService, file: &str, value: &serde_json::Value| {
        let config = root.join(file);
        fs::write(&config, value.to_string()).unwrap();
        runtime
            .start(
                &StartSpec {
                    executable: core.clone(),
                    config,
                    version: Some("1.14.4".into()),
                },
                cancel.clone(),
            )
            .unwrap();
    };
    run(
        &server,
        "server.json",
        &serde_json::json!({"inbounds":[{"type":"shadowsocks","listen":"::1","listen_port":node_port,"method":"aes-128-gcm","password":"fixture-password"}],"outbounds":[{"type":"direct","tag":"real"}],"route":{"rules":[{"ip_cidr":["203.0.113.42/32"],"action":"route","outbound":"real","override_address":"127.0.0.1","override_port":tls_port},{"ip_cidr":["2001:db8::42/128"],"action":"route","outbound":"real","override_address":"127.0.0.1","override_port":bad_port},{"action":"reject"}]}}),
    );
    let node = profile::import::preview(
        &format!("ss://YWVzLTEyOC1nY206Zml4dHVyZS1wYXNzd29yZA@node.invalid:{node_port}"),
        &[],
    )
    .unwrap()
    .nodes
    .remove(0);
    let mut input = ConnectionInput {
        node,
        routing: serde_json::from_value(serde_json::json!({"defaultExit":"proxy","policies":[]}))
            .unwrap(),
        versions: vec![],
        core_version: "1.14.4".into(),
        options: ConnectionOptions {
            ipv4_compatibility: false,
        },
    };
    let bindings = profile::config::Bindings {
        rules: Default::default(),
        cache: root.join("cache.db"),
        activity: None,
    };
    let config = |input: &ConnectionInput| {
        let mut value = profile::config::compile(input, &bindings).unwrap();
        value["inbounds"] = serde_json::json!([{"type":"mixed","listen":"127.0.0.1","listen_port":mixed_port},{"type":"direct","tag":"dns-probe","listen":"127.0.0.1","listen_port":dns_port}]);
        value["route"]["rules"].as_array_mut().unwrap().insert(
            0,
            serde_json::json!({"inbound":["dns-probe"],"action":"hijack-dns"}),
        );
        for tag in ["local", "proxy-dns"] {
            let index = if tag == "local" { 0 } else { 1 };
            value["dns"]["servers"][index] = serde_json::json!({"type":"hosts","tag":tag,"predefined":{"probe.invalid":["203.0.113.42","2001:db8::42"],"node.invalid":["::1"]}});
        }
        value
    };
    run(&client, "client.json", &config(&input));
    let addresses = dns(dns_port, 1);
    assert_eq!(addresses.len(), 1);
    let fake = addresses[0];
    assert!(matches!(fake,IpAddr::V4(ip) if profile::config::fakeip(ip)));
    assert!(dns(dns_port, 28).is_empty());
    // Dual-stack resolution may select the deliberately broken IPv6 exit.
    let initial_attempt = tls(socks(mixed_port, fake), client_tls.clone());
    if let Err(error) = initial_attempt {
        eprintln!("dual-stack selected the failing fixture exit: {error}");
    }
    assert!(
        tls(
            socks(mixed_port, "2001:db8::42".parse().unwrap()),
            client_tls.clone()
        )
        .is_err(),
        "IPv6 fixture accepts TCP then fails TLS"
    );
    input.options.ipv4_compatibility = true;
    let candidate = root.join("candidate.json");
    fs::write(&candidate, config(&input).to_string()).unwrap();
    RuntimeService::default()
        .validate(&core, &candidate, &cancel)
        .unwrap();
    client.stop().unwrap();
    run(&client, "client.json", &config(&input));
    tls(socks(mixed_port, fake), client_tls.clone()).unwrap_or_else(|e| {
        panic!(
            "IPv4-compatible TLS failed: {e}; client={:?}; server={:?}",
            client
                .logs
                .snapshot()
                .iter()
                .map(|l| &l.message)
                .collect::<Vec<_>>(),
            server
                .logs
                .snapshot()
                .iter()
                .map(|l| &l.message)
                .collect::<Vec<_>>()
        )
    });
    input.routing=serde_json::from_value(serde_json::json!({"defaultExit":"direct","policies":[{"id":"real","name":"真实 IP","enabled":true,"exit":"proxy","match":{"kind":"custom","domains":[],"ipCidrs":["203.0.113.42/32"]}}]})).unwrap();
    client.stop().unwrap();
    run(&client, "client.json", &config(&input));
    tls(socks(mixed_port, fake), client_tls).unwrap();
    assert_eq!(
        dns(dns_port, 1),
        vec!["203.0.113.42".parse::<IpAddr>().unwrap()]
    );
    client.stop().unwrap();
    server.stop().unwrap();
    stopped.store(true, Ordering::Release);
    echo.join().unwrap();
    failed_tls.join().unwrap();
    fs::remove_dir_all(root).unwrap();
}
