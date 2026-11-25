mod support;
use std::{
    fs,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    path::PathBuf,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use support::TestCore as Core;

#[test]
#[ignore = "requires an official sing-box binary in SING_SHELL_TEST_CORE"]
fn real_sing_box_proxies_loopback_traffic_and_stops() {
    let executable =
        PathBuf::from(std::env::var_os("SING_SHELL_TEST_CORE").expect("set SING_SHELL_TEST_CORE"));
    let id = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let directory =
        std::env::temp_dir().join(format!("sing-shell-real-{}-{id}", std::process::id()));
    fs::create_dir_all(&directory).unwrap();
    let core = Core::open(directory.join("data")).unwrap();
    core.select_core(&executable).unwrap();
    let echo = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let echo_port = echo.local_addr().unwrap().port();
    let proxy_port = TcpListener::bind(("127.0.0.1", 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let config = directory.join("config.json");
    fs::write(
        &config,
        serde_json::json!({
            "log":{"level":"info"},
            "inbounds":[{"type":"mixed","listen":"127.0.0.1","listen_port":proxy_port}],
            "outbounds":[{"type":"direct"}]
        })
        .to_string(),
    )
    .unwrap();
    core.import_config(&config).unwrap();
    core.start().unwrap();
    let start = Instant::now();
    let mut client = loop {
        if let Ok(stream) = TcpStream::connect(("127.0.0.1", proxy_port)) {
            break stream;
        }
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "core did not start: {:?}",
            core.snapshot().unwrap().error
        );
        thread::sleep(Duration::from_millis(20));
    };
    client
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    client.write_all(&[5, 1, 0]).unwrap();
    let mut greeting = [0; 2];
    client.read_exact(&mut greeting).unwrap();
    assert_eq!(greeting, [5, 0]);
    let [hi, lo] = echo_port.to_be_bytes();
    client
        .write_all(&[5, 1, 0, 1, 127, 0, 0, 1, hi, lo])
        .unwrap();
    let mut reply = [0; 10];
    client.read_exact(&mut reply).unwrap();
    assert_eq!(&reply[..4], &[5, 0, 0, 1]);
    echo.set_nonblocking(true).unwrap();
    let (mut server, _) = loop {
        match echo.accept() {
            Ok(connection) => break connection,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                assert!(start.elapsed() < Duration::from_secs(10));
                thread::sleep(Duration::from_millis(20));
            }
            Err(error) => panic!("{error}"),
        }
    };
    server.set_nonblocking(false).unwrap();
    server
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    client.write_all(b"ping").unwrap();
    let mut payload = [0; 4];
    server.read_exact(&mut payload).unwrap();
    assert_eq!(&payload, b"ping");
    server.write_all(b"pong").unwrap();
    client.read_exact(&mut payload).unwrap();
    assert_eq!(&payload, b"pong");
    drop(client);
    drop(server);
    core.stop().unwrap();
    assert_eq!(core.snapshot().unwrap().status, "stopped");
    let _released = TcpListener::bind(("127.0.0.1", proxy_port)).unwrap();
    drop(core);
    fs::remove_dir_all(&directory).unwrap();
}

#[test]
#[ignore = "requires official core and archive paths"]
fn managed_core_preparation_preserves_external_program_and_checks_saved_bytes() {
    use sing_shell::{
        settings::Engine,
        updates::{engine, release::Candidate, Component},
    };
    use std::sync::atomic::AtomicBool;
    let executable = PathBuf::from(std::env::var_os("SING_SHELL_TEST_CORE").unwrap());
    let archive = PathBuf::from(std::env::var_os("SING_SHELL_TEST_ARCHIVE").unwrap());
    let directory = std::env::temp_dir().join(format!(
        "sing-shell-managed-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let core = Core::open(directory.clone()).unwrap();
    let original = engine::hash(&executable).unwrap();
    core.select_core(&executable).unwrap();
    let config = directory.join("config.json");
    fs::write(&config,r#"{"inbounds":[{"type":"mixed","listen":"127.0.0.1","listen_port":0}],"outbounds":[{"type":"direct"}]}"#).unwrap();
    core.import_config(&config).unwrap();
    let candidate = Candidate {
        component: Component::Engine,
        version: "1.14.4".into(),
        target: sing_shell::TARGET.into(),
        url: "https://github.com/SagerNet/sing-box/releases/download/v1.14.4/test".into(),
        signature: String::new(),
        sha256: engine::hash(&archive).unwrap(),
        size: fs::metadata(&archive).unwrap().len(),
        notes: String::new(),
        release_url: String::new(),
        asset_name: archive.file_name().unwrap().to_string_lossy().into_owned(),
        asset_id: 1,
        tag: "v1.14.4".into(),
    };
    let digest = engine::prepare(
        &core.store,
        &core.runtime,
        &candidate,
        &archive,
        &AtomicBool::new(false),
    )
    .unwrap();
    assert!(matches!(
        core.store.snapshot().engine,
        Some(Engine::External { .. })
    ));
    engine::verify(
        &core.store,
        &core.runtime,
        &candidate,
        &digest,
        &AtomicBool::new(false),
    )
    .unwrap();
    let settings = core.store.snapshot();
    core.store
        .save(settings.revision, |s| {
            s.engine = Some(Engine::Managed {
                version: "1.14.4".into(),
                target: sing_shell::TARGET.into(),
            })
        })
        .unwrap();
    core.start().unwrap();
    core.stop().unwrap();
    let managed = core
        .store
        .engine_path(core.store.snapshot().engine.as_ref().unwrap())
        .unwrap();
    fs::write(managed, b"tampered").unwrap();
    assert!(engine::verify(
        &core.store,
        &core.runtime,
        &candidate,
        &digest,
        &AtomicBool::new(false)
    )
    .is_err());
    assert_eq!(engine::hash(&executable).unwrap(), original);
    drop(core);
    fs::remove_dir_all(directory).unwrap();
}
