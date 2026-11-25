#![cfg(feature = "test-fixtures")]

use sing_shell::core::Core;
use std::{
    fs,
    net::{TcpListener, TcpStream},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

struct Directory(PathBuf);

static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);

impl Directory {
    fn new() -> Self {
        let id = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        // Clock precision alone does not distinguish concurrent tests on macOS.
        let sequence = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "sing-shell test-{}-{id}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn config(&self, value: serde_json::Value) -> PathBuf {
        let path = self.0.join("配置 with spaces.json");
        fs::write(&path, value.to_string()).unwrap();
        path
    }
}

impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn concurrent_tests_use_distinct_directories() {
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
    let threads: Vec<_> = (0..8)
        .map(|_| {
            let barrier = barrier.clone();
            thread::spawn(move || {
                barrier.wait();
                Directory::new()
            })
        })
        .collect();
    let directories: Vec<_> = threads
        .into_iter()
        .map(|thread| thread.join().unwrap())
        .collect();
    let paths: std::collections::HashSet<_> =
        directories.iter().map(|directory| &directory.0).collect();
    assert_eq!(paths.len(), directories.len());
}

fn fixture() -> &'static Path {
    Path::new(env!("CARGO_BIN_EXE_core-fixture"))
}

fn wait(mut predicate: impl FnMut() -> bool) {
    let start = Instant::now();
    while !predicate() {
        assert!(start.elapsed() < Duration::from_secs(5), "timed out");
        thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn imports_persists_runs_and_releases_the_listener() {
    let directory = Directory::new();
    let mut core = Core::open(directory.0.join("data")).unwrap();
    core.select_core(fixture()).unwrap();
    fs::write(directory.0.join("relative.txt"), "relative data").unwrap();
    let port = TcpListener::bind(("127.0.0.1", 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let config = directory.config(serde_json::json!({"port":port,"relativeFile":"relative.txt"}));
    let original = fs::read(&config).unwrap();
    core.import_config(&config).unwrap();
    core.start().unwrap();
    wait(|| TcpStream::connect(("127.0.0.1", port)).is_ok());
    assert_eq!(core.snapshot().unwrap().status, "running");
    assert!(core.start().unwrap_err().contains("已在运行"));
    assert!(core.import_config(&config).unwrap_err().contains("先停止"));
    wait(|| {
        core.snapshot()
            .unwrap()
            .logs
            .iter()
            .any(|log| log.message == "fixture stderr")
    });
    assert!(core
        .snapshot()
        .unwrap()
        .logs
        .iter()
        .all(|log| log.message.len() <= 8192));
    core.stop().unwrap();
    assert_eq!(core.snapshot().unwrap().status, "stopped");
    let _listener = TcpListener::bind(("127.0.0.1", port)).unwrap();
    assert_eq!(fs::read(&config).unwrap(), original);
    drop(core);
    let mut restored = Core::open(directory.0.join("data")).unwrap();
    assert_eq!(
        restored.snapshot().unwrap().settings.config_path.unwrap(),
        config.canonicalize().unwrap()
    );
    assert_eq!(restored.snapshot().unwrap().status, "stopped");
}

#[test]
fn failed_import_preserves_previous_configuration_and_changed_configs_are_rechecked() {
    let directory = Directory::new();
    let mut core = Core::open(directory.0.join("data")).unwrap();
    assert!(core.start().is_err());
    assert!(core.select_core(Path::new("relative-program")).is_err());
    core.select_core(fixture()).unwrap();
    let config = directory.config(serde_json::json!({}));
    core.import_config(&config).unwrap();
    let settings = fs::read(directory.0.join("data/settings.json")).unwrap();
    fs::write(&config, r#"{"invalid":true}"#).unwrap();
    assert!(core
        .import_config(&config)
        .unwrap_err()
        .contains("校验失败"));
    assert_eq!(
        fs::read(directory.0.join("data/settings.json")).unwrap(),
        settings
    );
    assert!(core.start().is_err());
    assert!(core.snapshot().unwrap().pid.is_none());
    assert!(core
        .snapshot()
        .unwrap()
        .logs
        .iter()
        .any(|log| log.message.contains("invalid configuration")));
}

#[test]
fn unexpected_exit_is_reported_and_core_can_be_restarted() {
    let directory = Directory::new();
    let mut core = Core::open(directory.0.join("data")).unwrap();
    core.select_core(fixture()).unwrap();
    let config = directory.config(serde_json::json!({"exit":true}));
    core.import_config(&config).unwrap();
    core.start().unwrap();
    wait(|| core.snapshot().unwrap().status == "failed");
    assert!(core.snapshot().unwrap().error.unwrap().contains("意外退出"));
    fs::write(config, "{}").unwrap();
    core.start().unwrap();
    assert_eq!(core.snapshot().unwrap().status, "running");
    core.stop().unwrap();
}

#[test]
fn dropping_the_application_stops_the_core() {
    let directory = Directory::new();
    let mut core = Core::open(directory.0.join("data")).unwrap();
    core.select_core(fixture()).unwrap();
    let port = TcpListener::bind(("127.0.0.1", 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let config = directory.config(serde_json::json!({"port":port}));
    core.import_config(&config).unwrap();
    core.start().unwrap();
    wait(|| TcpStream::connect(("127.0.0.1", port)).is_ok());
    drop(core);
    let _listener = TcpListener::bind(("127.0.0.1", port)).unwrap();
}

#[test]
fn corrupted_settings_are_not_overwritten_even_after_stop() {
    let directory = Directory::new();
    fs::write(directory.0.join("settings.json"), "broken settings").unwrap();
    let mut core = Core::open(directory.0.clone()).unwrap();
    assert_eq!(core.snapshot().unwrap().status, "failed");
    core.stop().unwrap();
    assert!(core.select_core(fixture()).is_err());
    assert_eq!(
        fs::read_to_string(directory.0.join("settings.json")).unwrap(),
        "broken settings"
    );
}

#[test]
fn a_second_instance_cannot_manage_the_same_data_directory() {
    let directory = Directory::new();
    let core = Core::open(directory.0.clone()).unwrap();
    assert!(Core::open(directory.0.clone()).is_err());
    drop(core);
    assert!(Core::open(directory.0.clone()).is_ok());
}
