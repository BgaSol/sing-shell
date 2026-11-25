#![cfg(feature = "test-fixtures")]
mod support;
use sing_shell::{
    core::{
        managed::{Origin, Phase},
        package,
    },
    settings::{CoreSource, Trial},
    updates::{
        release::{self, Candidate},
        Component,
    },
};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use support::TestCore;
static SEQUENCE: AtomicU64 = AtomicU64::new(0);
struct Directory(PathBuf);
impl Directory {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!(
            "sing-shell-automation-{}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&p).unwrap();
        Self(p)
    }
    fn config(&self, value: serde_json::Value) -> PathBuf {
        let p = self.0.join("配置 with spaces.json");
        fs::write(&p, value.to_string()).unwrap();
        p
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn wait(mut f: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(12);
    while !f() {
        assert!(Instant::now() < deadline, "timed out");
        thread::sleep(Duration::from_millis(20));
    }
}
fn archive(d: &Directory, version: &str) -> (PathBuf, Candidate) {
    let name = release::engine_asset(version, sing_shell::TARGET).unwrap();
    let path = d.0.join(&name);
    let bytes = fs::read(env!("CARGO_BIN_EXE_core-fixture")).unwrap();
    if cfg!(windows) {
        let mut zip = zip::ZipWriter::new(fs::File::create(&path).unwrap());
        zip.start_file(
            format!("sing-box-{version}/sing-box.exe"),
            zip::write::SimpleFileOptions::default(),
        )
        .unwrap();
        zip.write_all(&bytes).unwrap();
        zip.finish().unwrap();
    } else {
        let gzip = flate2::write::GzEncoder::new(
            fs::File::create(&path).unwrap(),
            flate2::Compression::fast(),
        );
        let mut tar = tar::Builder::new(gzip);
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(0o700);
        header.set_cksum();
        tar.append_data(
            &mut header,
            format!("sing-box-{version}/sing-box"),
            &bytes[..],
        )
        .unwrap();
        tar.into_inner().unwrap().finish().unwrap();
    }
    let c = Candidate {
        component: Component::Engine,
        version: version.into(),
        target: sing_shell::TARGET.into(),
        url: format!("https://github.com/SagerNet/sing-box/releases/download/v{version}/{name}"),
        signature: String::new(),
        sha256: package::hash(&path).unwrap(),
        size: fs::metadata(&path).unwrap().len(),
        notes: String::new(),
        release_url: format!("https://github.com/SagerNet/sing-box/releases/tag/v{version}"),
        asset_name: name,
        asset_id: version.bytes().map(u64::from).sum(),
        tag: format!("v{version}"),
    };
    (path, c)
}
fn prepare(core: &TestCore, d: &Directory, version: &str, origin: Origin) {
    let (path, c) = archive(d, version);
    core.core
        .managed
        .stage_archive(
            &c,
            &path,
            origin,
            core.core.generation(),
            &AtomicBool::new(false),
        )
        .unwrap();
}
fn install(core: &TestCore, d: &Directory, version: &str) {
    prepare(core, d, version, Origin::Required);
    core.core.try_adopt().unwrap();
    assert_eq!(
        core.store.snapshot().managed.active.unwrap().version,
        version
    );
}
fn baseline(core: &TestCore, d: &Directory) {
    install(core, d, "1.14.3");
    let config = d.config(serde_json::json!({}));
    core.import_config(&config).unwrap();
    core.start().unwrap();
    core.stop().unwrap();
}
#[test]
fn first_install_promotes_pending_configuration_without_starting() {
    let d = Directory::new();
    let core = TestCore::open(d.0.join("data")).unwrap();
    let config = d.config(serde_json::json!({}));
    core.import_config(&config).unwrap();
    assert_eq!(
        core.store.snapshot().pending_config_path.as_ref(),
        Some(&config.canonicalize().unwrap())
    );
    assert!(core.store.snapshot().config_path.is_none());
    install(&core, &d, "1.14.3");
    assert!(core.store.snapshot().pending_config_path.is_none());
    assert!(core.store.snapshot().config_path.is_some());
    assert!(core.runtime.snapshot().pid.is_none());
    assert_eq!(
        core.store.snapshot().managed.trial,
        Some(Trial::AwaitingStart)
    );
    core.start().unwrap();
    assert!(core.store.snapshot().managed.trial.is_none());
    core.stop().unwrap();
}
#[test]
fn invalid_pending_config_does_not_reject_or_redownload_valid_core() {
    let d = Directory::new();
    let core = TestCore::open(d.0.join("data")).unwrap();
    let config = d.config(serde_json::json!({"invalid":true}));
    core.import_config(&config).unwrap();
    install(&core, &d, "1.14.3");
    assert!(core.core.snapshot().config_error.is_some());
    assert!(core.core.snapshot().usable);
    assert!(core.store.snapshot().config_path.is_none());
    assert!(core.core.managed.ready().is_none());
    assert!(core.start().is_err());
    fs::write(&config, "{}").unwrap();
    core.core.check().unwrap();
    assert!(core.store.snapshot().pending_config_path.is_none());
}
#[test]
fn running_process_and_listener_survive_preparation_and_switch_only_after_stop() {
    use std::net::{TcpListener, TcpStream};
    let d = Directory::new();
    let core = TestCore::open(d.0.join("data")).unwrap();
    baseline(&core, &d);
    let port = TcpListener::bind(("127.0.0.1", 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let config = d.config(serde_json::json!({"port":port}));
    core.import_config(&config).unwrap();
    core.start().unwrap();
    let pid = core.runtime.snapshot().pid;
    let _connection = TcpStream::connect(("127.0.0.1", port)).unwrap();
    prepare(&core, &d, "1.14.4", Origin::Automatic);
    core.core.try_adopt().unwrap();
    assert_eq!(core.runtime.snapshot().pid, pid);
    assert_eq!(
        core.store.snapshot().managed.active.unwrap().version,
        "1.14.3"
    );
    core.stop().unwrap();
    core.core.try_adopt().unwrap();
    assert_eq!(
        core.store.snapshot().managed.active.unwrap().version,
        "1.14.4"
    );
    assert!(core.runtime.snapshot().pid.is_none());
    core.start().unwrap();
    assert_eq!(core.runtime.snapshot().version.as_deref(), Some("1.14.4"));
    core.stop().unwrap();
}
#[test]
fn automatic_ready_respects_disabled_switch_but_explicit_retry_can_apply() {
    use sing_shell::core::Maintenance;
    let d = Directory::new();
    let core = TestCore::open(d.0.join("data")).unwrap();
    baseline(&core, &d);
    let revision = core.store.snapshot().preferences_revision;
    let mut prefs = core.store.snapshot().update_preferences;
    prefs.automatic_core = false;
    core.store.preferences(revision, prefs).unwrap();
    prepare(&core, &d, "1.14.4", Origin::Automatic);
    core.core.try_adopt().unwrap();
    assert_eq!(
        core.store.snapshot().managed.active.unwrap().version,
        "1.14.3"
    );
    assert_eq!(core.core.managed.snapshot().phase, Phase::Paused);
    core.core.maintain(Some(Maintenance::Retry)).unwrap();
    wait(|| {
        core.store
            .snapshot()
            .managed
            .active
            .as_ref()
            .unwrap()
            .version
            == "1.14.4"
    });
}
#[test]
fn incompatible_prepared_version_preserves_previous_and_can_retry_locally() {
    use sing_shell::core::Maintenance;
    let d = Directory::new();
    let core = TestCore::open(d.0.join("data")).unwrap();
    baseline(&core, &d);
    let config = d.config(serde_json::json!({"incompatibleVersion":"1.14.4"}));
    core.import_config(&config).unwrap();
    prepare(&core, &d, "1.14.4", Origin::Automatic);
    assert!(core.core.try_adopt().is_err());
    assert_eq!(
        core.store.snapshot().managed.active.unwrap().version,
        "1.14.3"
    );
    let blocked = core.store.snapshot().managed.blocked.unwrap();
    assert_eq!(
        blocked.stage,
        sing_shell::settings::FailureStage::Compatibility
    );
    fs::write(config, "{}").unwrap();
    core.core.maintain(Some(Maintenance::Retry)).unwrap();
    wait(|| {
        core.store
            .snapshot()
            .managed
            .active
            .as_ref()
            .unwrap()
            .version
            == "1.14.4"
    });
}
#[test]
fn early_new_version_failure_rolls_back_once_and_runs_previous() {
    let d = Directory::new();
    let core = TestCore::open(d.0.join("data")).unwrap();
    baseline(&core, &d);
    let config = d.config(serde_json::json!({"failVersion":"1.14.4"}));
    core.import_config(&config).unwrap();
    install(&core, &d, "1.14.4");
    core.start().unwrap();
    assert_eq!(core.runtime.snapshot().version.as_deref(), Some("1.14.3"));
    let s = core.store.snapshot();
    assert_eq!(s.managed.active.unwrap().version, "1.14.3");
    assert!(s.managed.previous.is_none());
    assert!(s.managed.trial.is_none());
    assert_eq!(s.managed.blocked.unwrap().candidate.version, "1.14.4");
    core.stop().unwrap();
}
#[test]
fn later_exit_does_not_rollback_or_restart() {
    let d = Directory::new();
    let core = TestCore::open(d.0.join("data")).unwrap();
    baseline(&core, &d);
    let config = d.config(serde_json::json!({"exitAfterMs":2600}));
    core.import_config(&config).unwrap();
    install(&core, &d, "1.14.4");
    core.start().unwrap();
    wait(|| core.runtime.snapshot().pid.is_none());
    assert_eq!(
        core.store.snapshot().managed.active.unwrap().version,
        "1.14.4"
    );
    assert!(core.store.snapshot().managed.blocked.is_none());
}
#[test]
fn external_selection_invalidates_prepared_automatic_takeover() {
    let d = Directory::new();
    let core = TestCore::open(d.0.join("data")).unwrap();
    prepare(&core, &d, "1.14.4", Origin::Automatic);
    let external = Path::new(env!("CARGO_BIN_EXE_core-fixture"));
    let before = fs::read(external).unwrap();
    core.select_core(external).unwrap();
    core.core.try_adopt().unwrap();
    assert!(matches!(
        core.store.snapshot().core_source,
        CoreSource::External { .. }
    ));
    assert_eq!(fs::read(external).unwrap(), before);
}
#[test]
fn settings_domains_do_not_overwrite_each_other_and_reject_stale_preferences() {
    let d = Directory::new();
    let core = TestCore::open(d.0.join("data")).unwrap();
    let before = core.store.snapshot();
    install(&core, &d, "1.14.3");
    let mut p = before.update_preferences;
    p.automatic_core = false;
    core.store
        .preferences(before.preferences_revision, p.clone())
        .unwrap();
    assert_eq!(
        core.store.snapshot().managed.active.unwrap().version,
        "1.14.3"
    );
    assert!(core
        .store
        .preferences(before.preferences_revision, p)
        .is_err());
}
#[test]
fn incomplete_trial_recovers_previous_without_starting_or_reviving_ready_cache() {
    let d = Directory::new();
    let data = d.0.join("data");
    let core = TestCore::open(data.clone()).unwrap();
    baseline(&core, &d);
    install(&core, &d, "1.14.4");
    let s = core.store.snapshot();
    core.store
        .core(s.core_revision, |v| {
            v.managed.trial = Some(Trial::Starting);
            Ok(())
        })
        .unwrap();
    drop(core);
    let restored = TestCore::open(data).unwrap();
    assert_eq!(
        restored.store.snapshot().managed.active.unwrap().version,
        "1.14.3"
    );
    assert!(restored.runtime.snapshot().pid.is_none());
    assert!(restored.store.snapshot().managed.blocked.is_some());
}
#[test]
fn migrated_v1_managed_has_legacy_evidence_and_respects_disabled_preference() {
    let d = Directory::new();
    let version = "1.14.4";
    let path =
        d.0.join("engines/sing-box")
            .join(version)
            .join(sing_shell::TARGET)
            .join(if cfg!(windows) {
                "sing-box.exe"
            } else {
                "sing-box"
            });
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::copy(env!("CARGO_BIN_EXE_core-fixture"), &path).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let old = serde_json::json!({"schemaVersion":1,"revision":8,"engine":{"mode":"managed","version":version,"target":sing_shell::TARGET},"configPath":null,"updatePreferences":{"automatic":false,"network":{"revision":0,"fallbackEnabled":true,"disabledBuiltins":[],"customMirrors":[]}}});
    fs::write(d.0.join("settings.json"), old.to_string()).unwrap();
    let core = TestCore::open(d.0.clone()).unwrap();
    let s = core.store.snapshot();
    assert_eq!(s.schema_version, 2);
    assert!(!s.update_preferences.automatic_core);
    let r = s.managed.active.unwrap();
    assert!(r.hash.is_some());
    assert!(r.source.is_none());
    assert!(d.0.join("settings.v1.backup.json").exists());
}
#[test]
fn cancelled_hash_and_archive_do_not_publish_ready() {
    let d = Directory::new();
    let core = TestCore::open(d.0.join("data")).unwrap();
    let (path, c) = archive(&d, "1.14.4");
    let cancel = AtomicBool::new(true);
    assert!(package::hash_cancel(&path, &cancel).is_err());
    assert!(core
        .core
        .managed
        .stage_archive(&c, &path, Origin::Required, 0, &cancel)
        .is_err());
    assert!(core.core.managed.ready().is_none());
}
#[test]
fn background_tick_without_frontend_discovers_downloads_and_adopts_official_shape() {
    use sing_shell::updates::network::{State, Transport};
    use std::net::TcpListener;
    let d = Directory::new();
    let core = TestCore::open(d.0.join("data")).unwrap();
    let (path, c) = archive(&d, "1.14.4");
    let bytes = fs::read(path).unwrap();
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    listener.set_nonblocking(true).unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let done = Arc::new(AtomicBool::new(false));
    let stopped = done.clone();
    let worker = thread::spawn(move || {
        while !stopped.load(Ordering::Acquire) {
            let Ok((mut socket, _)) = listener.accept() else {
                thread::sleep(Duration::from_millis(2));
                continue;
            };
            socket
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut request = Vec::new();
            let mut one = [0];
            while !request.ends_with(b"\r\n\r\n") {
                if socket.read(&mut one).unwrap_or(0) == 0 {
                    break;
                }
                request.push(one[0]);
            }
            let request = String::from_utf8_lossy(&request);
            let body = if request.contains("matching-refs") {
                serde_json::json!([{"ref":"refs/tags/v1.14.4"}])
                    .to_string()
                    .into_bytes()
            } else if request.contains("/releases/tags/") {
                serde_json::json!({"tag_name":"v1.14.4","draft":false,"prerelease":false,"body":"official-shape fixture","html_url":c.release_url,"assets":[{"id":c.asset_id,"name":c.asset_name,"browser_download_url":c.url,"size":c.size,"digest":format!("sha256:{}",c.sha256)}]}).to_string().into_bytes()
            } else {
                bytes.clone()
            };
            let _ = write!(
                socket,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = socket.write_all(&body);
        }
    });
    *core.core.managed.test_transport.lock().unwrap() = Some(Transport::test_server(
        &base,
        vec![],
        Arc::new(State::default()),
    ));
    let mut prefs = core.store.snapshot().update_preferences;
    prefs.automatic = false;
    prefs.automatic_core = false;
    core.store.preferences(0, prefs).unwrap();
    core.core.tick();
    wait(|| {
        core.store.snapshot().managed.active.is_some()
            || core.core.managed.snapshot().phase == Phase::Error
    });
    let state = core.core.managed.snapshot();
    assert!(
        core.store.snapshot().managed.active.is_some(),
        "{:?}",
        state.error
    );
    assert!(core.runtime.snapshot().pid.is_none());
    assert!(state.next_check_at > state.last_checked_at);
    done.store(true, Ordering::Release);
    worker.join().unwrap();
}
#[test]
fn cancellation_during_initial_start_does_not_attempt_previous() {
    use sing_shell::core::Action;
    let d = Directory::new();
    let core = TestCore::open(d.0.join("data")).unwrap();
    baseline(&core, &d);
    install(&core, &d, "1.14.4");
    core.core.action(Action::Start).unwrap();
    wait(|| core.runtime.snapshot().pid.is_some());
    core.core.action(Action::Stop).unwrap();
    wait(|| !core.core.gate.runtime_busy() && core.runtime.snapshot().pid.is_none());
    let s = core.store.snapshot();
    assert_eq!(s.managed.active.unwrap().version, "1.14.4");
    assert_eq!(s.managed.trial, Some(Trial::AwaitingStart));
    assert!(s.managed.blocked.is_none());
}
#[test]
fn cancelled_ready_is_not_applied_on_start_or_tick() {
    let d = Directory::new();
    let core = TestCore::open(d.0.join("data")).unwrap();
    baseline(&core, &d);
    prepare(&core, &d, "1.14.4", Origin::Automatic);
    core.core.cancel();
    core.core.try_adopt().unwrap();
    core.start().unwrap();
    assert_eq!(core.runtime.snapshot().version.as_deref(), Some("1.14.3"));
    core.stop().unwrap();
}
#[test]
fn tampered_ready_cache_is_not_restored_or_executed() {
    let d = Directory::new();
    let data = d.0.join("data");
    let core = TestCore::open(data.clone()).unwrap();
    prepare(&core, &d, "1.14.4", Origin::Required);
    let p = core.core.managed.ready().unwrap();
    let staging = package::staged_path(&data, &p.reference).unwrap();
    fs::write(staging, b"tampered").unwrap();
    drop(core);
    let restored = TestCore::open(data).unwrap();
    assert!(restored.core.managed.ready().is_none());
    assert_eq!(restored.core.managed.snapshot().phase, Phase::Error);
    assert!(restored.runtime.snapshot().pid.is_none());
}
#[test]
fn valid_cached_archive_repairs_same_version_offline_without_overwriting_external_files() {
    let d = Directory::new();
    let core = TestCore::open(d.0.join("data")).unwrap();
    baseline(&core, &d);
    let active = core.store.snapshot().managed.active.unwrap();
    let path = sing_shell::settings::managed_path(&core.store.root, &active).unwrap();
    fs::write(&path, b"broken").unwrap();
    assert!(core.start().is_err());
    core.core.tick();
    wait(|| {
        core.core.managed.ready().is_some() || core.core.managed.snapshot().phase == Phase::Error
    });
    wait(|| {
        package::hash(&path).is_ok_and(|h| Some(h) == active.hash)
            && !core.core.gate.core_fetching()
    });
    assert_eq!(
        core.store.snapshot().managed.active.unwrap().version,
        active.version
    );
    assert!(core.runtime.snapshot().pid.is_none());
}
#[test]
fn failed_start_confirmation_never_leaves_an_unrecorded_proxy_running() {
    let d = Directory::new();
    let core = TestCore::open(d.0.join("data")).unwrap();
    install(&core, &d, "1.14.3");
    let config = d.config(serde_json::json!({}));
    core.import_config(&config).unwrap();
    core.core.action(sing_shell::core::Action::Start).unwrap();
    wait(|| core.runtime.snapshot().pid.is_some());
    fs::create_dir(core.store.root.join("settings.json.tmp")).unwrap();
    wait(|| !core.core.gate.runtime_busy());
    assert!(core.runtime.snapshot().pid.is_none());
    assert!(core
        .runtime
        .snapshot()
        .error
        .unwrap()
        .contains("启动确认保存失败"));
}
