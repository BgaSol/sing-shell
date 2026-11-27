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
    let envelope =
        sing_shell::core::material::fixture(&c, Path::new(env!("CARGO_BIN_EXE_core-fixture")));
    (path, envelope.candidate(version).unwrap())
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
fn identical_unrecorded_program_recovers_without_overwriting_or_restarting_trial() {
    let d = Directory::new();
    let core = TestCore::open(d.0.join("data")).unwrap();
    install(&core, &d, "1.14.4");
    let reference = core.store.snapshot().managed.active.unwrap();
    let program = sing_shell::settings::managed_path(&core.store.root, &reference).unwrap();
    let modified = fs::metadata(&program).unwrap().modified().unwrap();
    core.store
        .core(core.store.snapshot().core_revision, |s| {
            s.managed = Default::default();
            Ok(())
        })
        .unwrap();
    prepare(&core, &d, "1.14.4", Origin::Required);
    core.core.try_adopt().unwrap();
    assert_eq!(
        fs::metadata(&program).unwrap().modified().unwrap(),
        modified
    );
    let revision = core.store.snapshot().core_revision;
    core.core.try_adopt().unwrap();
    assert_eq!(core.store.snapshot().core_revision, revision);
    assert_eq!(
        core.store.snapshot().managed.trial,
        Some(Trial::AwaitingStart)
    );
    assert!(core.runtime.snapshot().pid.is_none());
}
#[test]
fn explicit_schema_reset_recovers_preserved_prepared_material_in_the_same_process() {
    let d = Directory::new();
    let data = d.0.join("data");
    let core = TestCore::open(data.clone()).unwrap();
    install(&core, &d, "1.14.4");
    prepare(&core, &d, "1.14.4", Origin::Required);
    let mut old = serde_json::to_value(core.store.snapshot()).unwrap();
    old["schemaVersion"] = serde_json::json!(4);
    let bytes = serde_json::to_vec(&old).unwrap();
    fs::write(data.join("settings.json"), &bytes).unwrap();
    drop(core);
    let core = TestCore::open(data).unwrap();
    assert!(core.store.error().is_some());
    let backup = core.session.reset_settings().unwrap();
    assert_eq!(fs::read(backup).unwrap(), bytes);
    core.core.try_adopt().unwrap();
    assert_eq!(
        core.store.snapshot().managed.active.unwrap().version,
        "1.14.4"
    );
    assert!(!core.store.snapshot().startup.resume_core);
    assert!(core.runtime.snapshot().pid.is_none());
}
#[test]
fn generated_mode_rejects_an_unvalidated_core_before_importing_nodes() {
    let d = Directory::new();
    let c = TestCore::open(d.0.join("data")).unwrap();
    c.store
        .profile(c.store.snapshot().nodes_revision, |p| {
            p.mode = sing_shell::profile::Mode::Generated;
            Ok(())
        })
        .unwrap();
    prepare(&c, &d, "1.14.3", Origin::Required);
    assert!(c.core.try_adopt().is_err());
    assert!(c.store.snapshot().managed.active.is_none());
    assert!(c.store.snapshot().managed.blocked.is_some());
}
#[test]
fn restore_waits_for_required_core_and_successful_rollback_records_intent() {
    let d = Directory::new();
    {
        let c = TestCore::open(d.0.join("data")).unwrap();
        c.session.set_restore(true).unwrap();
        c.store.resume(true).unwrap();
        let config = d.config(serde_json::json!({}));
        c.import_config(&config).unwrap();
    }
    let c = TestCore::open(d.0.join("data")).unwrap();
    c.session.restore_tick();
    assert_eq!(
        c.session.snapshot().restore,
        sing_shell::session::Restore::Waiting
    );
    install(&c, &d, "1.14.3");
    c.session.restore_tick();
    wait(|| c.session.snapshot().restore == sing_shell::session::Restore::Finished);
    assert!(c.runtime.snapshot().pid.is_some());
    assert!(c.store.snapshot().startup.resume_core);
    c.stop().unwrap();
    let config = d.config(serde_json::json!({"failVersion":"1.14.4"}));
    c.import_config(&config).unwrap();
    install(&c, &d, "1.14.4");
    c.start().unwrap();
    assert!(c.store.snapshot().startup.resume_core);
    assert_eq!(c.store.snapshot().managed.active.unwrap().version, "1.14.3");
    c.stop().unwrap();
}
#[test]
fn unconfirmed_trial_blocks_restore_even_after_previous_version_is_recovered() {
    let d = Directory::new();
    {
        let c = TestCore::open(d.0.join("data")).unwrap();
        baseline(&c, &d);
        install(&c, &d, "1.14.4");
        let s = c.store.snapshot();
        c.store
            .core(s.core_revision, |s| {
                s.managed.trial = Some(Trial::Starting);
                Ok(())
            })
            .unwrap();
        c.session.set_restore(true).unwrap();
        c.store.resume(true).unwrap();
    }
    let c = TestCore::open(d.0.join("data")).unwrap();
    c.session.restore_tick();
    assert_eq!(
        c.session.snapshot().restore,
        sing_shell::session::Restore::Finished
    );
    assert!(c.runtime.snapshot().pid.is_none());
    assert_eq!(c.store.snapshot().managed.active.unwrap().version, "1.14.3");
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
fn unsupported_managed_schema_is_preserved_without_adoption() {
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
    assert_eq!(s.schema_version, 5);
    assert!(core.store.error().is_some());
    assert!(s.managed.active.is_none());
    assert_eq!(
        fs::read_to_string(d.0.join("settings.json")).unwrap(),
        old.to_string()
    );
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
    let d = Directory::new();
    let core = TestCore::open(d.0.join("data")).unwrap();
    let (done, worker, _) = release_server(&core, &d, "1.14.4");
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
    use sing_shell::session::Action;
    let d = Directory::new();
    let core = TestCore::open(d.0.join("data")).unwrap();
    baseline(&core, &d);
    install(&core, &d, "1.14.4");
    core.session.action(Action::Start).unwrap();
    wait(|| core.runtime.snapshot().pid.is_some());
    core.session.action(Action::Stop).unwrap();
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
    core.session.cancel_core();
    core.core.try_adopt().unwrap();
    core.start().unwrap();
    assert_eq!(core.runtime.snapshot().version.as_deref(), Some("1.14.3"));
    core.stop().unwrap();
}
#[test]
fn tampered_staging_is_rebuilt_from_verified_archive_without_execution() {
    let d = Directory::new();
    let data = d.0.join("data");
    let core = TestCore::open(data.clone()).unwrap();
    prepare(&core, &d, "1.14.4", Origin::Required);
    let p = core.core.managed.ready().unwrap();
    let staging = package::staged_path(&data, &p.reference).unwrap();
    fs::write(&staging, b"tampered").unwrap();
    drop(core);
    let restored = TestCore::open(data).unwrap();
    assert!(restored.core.managed.ready().is_some());
    assert_eq!(restored.core.managed.snapshot().phase, Phase::Ready);
    assert_eq!(package::hash(&staging).unwrap(), p.reference.hash.unwrap());
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
        let maintenance = core.core.managed.snapshot();
        assert_ne!(
            maintenance.phase,
            Phase::Error,
            "repair failed: {:?}",
            maintenance.error
        );
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
    core.session
        .action(sing_shell::session::Action::Start)
        .unwrap();
    wait(|| core.runtime.snapshot().pid.is_some());
    fs::create_dir(core.store.root.join("settings.private-tmp")).unwrap();
    wait(|| !core.core.gate.runtime_busy());
    assert!(core.runtime.snapshot().pid.is_none());
    assert!(core
        .runtime
        .snapshot()
        .error
        .unwrap()
        .contains("启动确认保存失败"));
}

#[test]
fn unrecorded_conflict_pauses_automatic_work_and_explicit_retry_repairs_only_the_program() {
    let d = Directory::new();
    let core = TestCore::open(d.0.join("data")).unwrap();
    prepare(&core, &d, "1.14.4", Origin::Required);
    let p = core.core.managed.ready().unwrap();
    let program = sing_shell::settings::managed_path(&core.store.root, &p.reference).unwrap();
    fs::write(&program, b"unrecorded bytes").unwrap();
    let other = program.with_file_name("keep.txt");
    fs::write(&other, b"keep").unwrap();
    assert!(core.core.try_adopt().is_err());
    assert_eq!(
        core.core.managed.snapshot().failure_kind,
        Some(sing_shell::core::managed::FailureKind::Conflict)
    );
    let logs = core.runtime.logs.snapshot().len();
    core.core.tick();
    assert_eq!(core.runtime.logs.snapshot().len(), logs);
    core.core
        .maintain(Some(sing_shell::core::Maintenance::Retry))
        .unwrap();
    wait(|| core.store.snapshot().managed.active.is_some() && !core.core.gate.core_fetching());
    assert_eq!(package::hash(&program).unwrap(), p.reference.hash.unwrap());
    assert_eq!(fs::read(other).unwrap(), b"keep");
    assert!(core.runtime.snapshot().pid.is_none());
}

#[test]
fn published_program_survives_failed_settings_write_and_retries_without_a_download() {
    let d = Directory::new();
    let core = TestCore::open(d.0.join("data")).unwrap();
    prepare(&core, &d, "1.14.4", Origin::Required);
    let temp = core.store.root.join("settings.private-tmp");
    let blocked = temp.clone();
    *core.core.adoption_hook.lock().unwrap() = Some(Arc::new(move |stage| {
        if stage == "after_publish" {
            fs::create_dir(&blocked).unwrap();
        }
    }));
    assert!(core.core.try_adopt().is_err());
    assert!(core.store.snapshot().managed.active.is_none());
    let p = core.core.managed.ready().unwrap();
    let installed = sing_shell::settings::managed_path(&core.store.root, &p.reference).unwrap();
    let modified = fs::metadata(&installed).unwrap().modified().unwrap();
    assert!(!package::staged_path(&core.store.root, &p.reference)
        .unwrap()
        .exists());
    assert!(core.store.snapshot().managed.blocked.is_none());
    *core.core.adoption_hook.lock().unwrap() = None;
    fs::remove_dir(temp).unwrap();
    core.core
        .maintain(Some(sing_shell::core::Maintenance::Retry))
        .unwrap();
    wait(|| core.store.snapshot().managed.active.is_some() && !core.core.gate.core_fetching());
    assert_eq!(
        fs::metadata(installed).unwrap().modified().unwrap(),
        modified
    );
    assert!(core.runtime.snapshot().pid.is_none());
}

#[test]
fn committed_selection_survives_prepared_cleanup_failure_and_restart_preserves_trial() {
    let d = Directory::new();
    let data = d.0.join("data");
    let core = TestCore::open(data.clone()).unwrap();
    prepare(&core, &d, "1.14.4", Origin::Required);
    let record = core.core.managed.cache().join("prepared.json");
    let bytes = fs::read(&record).unwrap();
    let blocked = record.clone();
    *core.core.adoption_hook.lock().unwrap() = Some(Arc::new(move |stage| {
        if stage == "after_commit" {
            fs::remove_file(&blocked).unwrap();
            fs::create_dir(&blocked).unwrap();
        }
    }));
    assert_eq!(
        core.core.try_adopt().unwrap(),
        sing_shell::core::Adoption::Applied
    );
    assert!(core.core.snapshot().usable);
    assert!(core.core.managed.snapshot().warning.is_some());
    assert!(core.core.managed.ready().is_none());
    let revision = core.store.snapshot().core_revision;
    drop(core);
    fs::remove_dir(&record).unwrap();
    fs::write(record, bytes).unwrap();
    let core = TestCore::open(data).unwrap();
    assert_eq!(
        core.core.try_adopt().unwrap(),
        sing_shell::core::Adoption::AlreadyCurrent
    );
    assert_eq!(core.store.snapshot().core_revision, revision);
    assert_eq!(
        core.store.snapshot().managed.trial,
        Some(Trial::AwaitingStart)
    );
}

#[test]
fn cancellation_before_commit_never_selects_the_candidate_and_after_commit_is_truthful() {
    for checkpoint in [
        "before_publish",
        "after_publish",
        "before_commit",
        "after_commit",
    ] {
        let d = Directory::new();
        let core = TestCore::open(d.0.join("data")).unwrap();
        prepare(&core, &d, "1.14.4", Origin::Required);
        let weak = Arc::downgrade(&core.core);
        *core.core.adoption_hook.lock().unwrap() = Some(Arc::new(move |stage| {
            if stage == checkpoint {
                weak.upgrade().unwrap().cancel();
            }
        }));
        let outcome = core.core.try_adopt().unwrap();
        if checkpoint == "after_commit" {
            assert_eq!(outcome, sing_shell::core::Adoption::Applied);
            assert!(core.store.snapshot().managed.active.is_some());
            assert!(core.core.snapshot().usable);
        } else {
            assert_eq!(outcome, sing_shell::core::Adoption::Deferred);
            assert!(core.store.snapshot().managed.active.is_none());
            assert!(core.core.managed.ready().is_some());
            *core.core.adoption_hook.lock().unwrap() = None;
            core.core
                .maintain(Some(sing_shell::core::Maintenance::Retry))
                .unwrap();
            wait(|| {
                core.store.snapshot().managed.active.is_some() && !core.core.gate.core_fetching()
            });
        }
        assert!(core.runtime.snapshot().pid.is_none());
    }
}

#[test]
fn disabling_automatic_adoption_after_publication_preserves_the_old_selection() {
    let d = Directory::new();
    let core = TestCore::open(d.0.join("data")).unwrap();
    baseline(&core, &d);
    prepare(&core, &d, "1.14.4", Origin::Automatic);
    let store = core.store.clone();
    *core.core.adoption_hook.lock().unwrap() = Some(Arc::new(move |stage| {
        if stage == "after_publish" {
            let s = store.snapshot();
            let mut prefs = s.update_preferences;
            prefs.automatic_core = false;
            store.preferences(s.preferences_revision, prefs).unwrap();
        }
    }));
    assert_eq!(
        core.core.try_adopt().unwrap(),
        sing_shell::core::Adoption::Deferred
    );
    assert_eq!(
        core.store.snapshot().managed.active.unwrap().version,
        "1.14.3"
    );
    assert!(core.core.managed.ready().is_some());
}

#[test]
fn preparation_io_failure_is_local_and_preserves_selected_core() {
    let d = Directory::new();
    let core = TestCore::open(d.0.join("data")).unwrap();
    baseline(&core, &d);
    let (archive, candidate) = archive(&d, "1.14.4");
    let reference = sing_shell::settings::ManagedRef {
        version: candidate.version.clone(),
        target: candidate.target.clone(),
        hash: None,
        source: None,
    };
    let stage = package::staged_path(&core.store.root, &reference).unwrap();
    fs::create_dir_all(&stage).unwrap();
    assert!(matches!(
        core.core.managed.stage_archive(
            &candidate,
            &archive,
            Origin::Manual,
            core.core.generation(),
            &AtomicBool::new(false)
        ),
        Err(sing_shell::core::managed::FetchError::Local(_))
    ));
    assert_eq!(
        core.store.snapshot().managed.active.unwrap().version,
        "1.14.3"
    );
    assert!(core.store.snapshot().managed.blocked.is_none());
}

#[test]
fn network_occupation_and_core_file_tasks_prevent_adoption_and_reset() {
    let d = Directory::new();
    let core = TestCore::open(d.0.join("data")).unwrap();
    prepare(&core, &d, "1.14.4", Origin::Required);
    core.runtime.network_owned(true);
    assert_eq!(
        core.core.try_adopt().unwrap(),
        sing_shell::core::Adoption::Deferred
    );
    core.runtime.network_owned(false);
    let files = core
        .core
        .gate
        .begin(sing_shell::app::Kind::CoreFetch)
        .unwrap();
    assert!(core.session.reset_settings().is_err());
    assert_eq!(
        core.core.try_adopt().unwrap(),
        sing_shell::core::Adoption::Deferred
    );
    drop(files);
    core.core.try_adopt().unwrap();
    assert!(core.core.snapshot().usable);
}

#[test]
fn a_new_candidate_cannot_overwrite_the_single_prepared_slot() {
    let d = Directory::new();
    let core = TestCore::open(d.0.join("data")).unwrap();
    prepare(&core, &d, "1.14.4", Origin::Required);
    let before = fs::read(core.core.managed.cache().join("prepared.json")).unwrap();
    let (path, candidate) = archive(&d, "1.14.5");
    assert!(core
        .core
        .managed
        .stage_archive(
            &candidate,
            &path,
            Origin::Manual,
            core.core.generation(),
            &AtomicBool::new(false)
        )
        .is_err());
    assert_eq!(
        fs::read(core.core.managed.cache().join("prepared.json")).unwrap(),
        before
    );
    core.core.try_adopt().unwrap();
    assert_eq!(
        core.store.snapshot().managed.active.unwrap().version,
        "1.14.4"
    );
}

fn release_server(
    core: &TestCore,
    d: &Directory,
    version: &str,
) -> (Arc<AtomicBool>, thread::JoinHandle<()>, Arc<AtomicU64>) {
    use sing_shell::updates::network::{State, Transport};
    use std::net::TcpListener;
    let (path, c) = archive(d, version);
    let bytes = fs::read(path).unwrap();
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    listener.set_nonblocking(true).unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let done = Arc::new(AtomicBool::new(false));
    let stopped = done.clone();
    let downloads = Arc::new(AtomicU64::new(0));
    let downloaded = downloads.clone();
    let worker = thread::spawn(move || {
        while !stopped.load(Ordering::Acquire) {
            let Ok((mut socket, _)) = listener.accept() else {
                thread::sleep(Duration::from_millis(2));
                continue;
            };
            socket.set_nonblocking(false).unwrap();
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
            let body = if request.contains("recommended.json") || request.contains("material.json")
            {
                c.signature.as_bytes().to_vec()
            } else if request.contains("matching-refs") {
                serde_json::json!([{"ref":format!("refs/tags/v{}",c.version)}])
                    .to_string()
                    .into_bytes()
            } else if request.contains("/releases/tags/") {
                serde_json::json!({"tag_name":format!("v{}",c.version),"draft":false,"prerelease":false,"body":"official-shape fixture","html_url":c.release_url,"assets":[{"id":c.asset_id,"name":c.asset_name,"browser_download_url":c.url,"size":c.size,"digest":format!("sha256:{}",c.sha256)}]}).to_string().into_bytes()
            } else {
                downloaded.fetch_add(1, Ordering::Relaxed);
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
    (done, worker, downloads)
}

#[test]
fn check_with_prepared_material_does_not_adopt_promote_origin_or_clear_failure() {
    let d = Directory::new();
    let core = TestCore::open(d.0.join("data")).unwrap();
    prepare(&core, &d, "1.14.5", Origin::Automatic);
    core.core.managed.fail(
        sing_shell::core::managed::FailureKind::Conflict,
        "preserve adoption failure".into(),
    );
    core.core.managed.suppressed.store(true, Ordering::Release);
    let (done, worker, downloads) = release_server(&core, &d, "1.14.6");
    core.core
        .maintain(Some(sing_shell::core::Maintenance::Check))
        .unwrap();
    wait(|| !core.core.gate.core_fetching());
    assert!(core.store.snapshot().managed.active.is_none());
    assert_eq!(core.core.managed.ready().unwrap().origin, Origin::Automatic);
    assert_eq!(
        core.core.managed.snapshot().error.as_deref(),
        Some("preserve adoption failure")
    );
    assert_eq!(
        core.core.managed.snapshot().candidate.unwrap().version,
        "1.14.6"
    );
    assert!(core.core.managed.suppressed.load(Ordering::Acquire));
    assert_eq!(downloads.load(Ordering::Relaxed), 0);
    done.store(true, Ordering::Release);
    worker.join().unwrap();
}

#[test]
fn retry_of_corrupt_archive_and_staging_rechecks_and_downloads_the_same_asset() {
    let d = Directory::new();
    let core = TestCore::open(d.0.join("data")).unwrap();
    prepare(&core, &d, "1.14.5", Origin::Required);
    let prepared = core.core.managed.ready().unwrap();
    let (_, newer) = archive(&d, "1.14.6");
    core.core.managed.update(|s| s.candidate = Some(newer));
    fs::write(
        package::staged_path(&core.store.root, &prepared.reference).unwrap(),
        b"broken",
    )
    .unwrap();
    fs::write(core.core.managed.cache().join("package.bin"), b"broken").unwrap();
    let (done, worker, downloads) = release_server(&core, &d, "1.14.5");
    core.core
        .maintain(Some(sing_shell::core::Maintenance::Retry))
        .unwrap();
    wait(|| {
        core.store.snapshot().managed.active.is_some()
            || (core.core.managed.snapshot().phase == Phase::Error
                && !core.core.gate.core_fetching())
    });
    assert_eq!(
        core.store
            .snapshot()
            .managed
            .active
            .as_ref()
            .map(|r| r.version.as_str()),
        Some("1.14.5"),
        "{:?}",
        core.core.managed.snapshot().error
    );
    assert_eq!(downloads.load(Ordering::Relaxed), 1);
    done.store(true, Ordering::Release);
    worker.join().unwrap();
    wait(|| !core.core.gate.core_fetching());
}

#[test]
fn an_existing_target_directory_is_never_replaced_even_by_manual_retry() {
    let d = Directory::new();
    let core = TestCore::open(d.0.join("data")).unwrap();
    prepare(&core, &d, "1.14.4", Origin::Manual);
    let prepared = core.core.managed.ready().unwrap();
    let target = sing_shell::settings::managed_path(&core.store.root, &prepared.reference).unwrap();
    fs::create_dir(&target).unwrap();
    fs::write(target.join("keep"), b"keep").unwrap();
    assert!(core.core.try_adopt().is_err());
    core.core.managed.resume(core.core.generation()).unwrap();
    assert!(core.core.try_adopt().is_err());
    assert_eq!(fs::read(target.join("keep")).unwrap(), b"keep");
    assert!(core.store.snapshot().managed.active.is_none());
}

#[cfg(unix)]
#[test]
fn existing_target_symlink_is_rejected_without_touching_the_external_program() {
    let d = Directory::new();
    let core = TestCore::open(d.0.join("data")).unwrap();
    prepare(&core, &d, "1.14.4", Origin::Manual);
    let prepared = core.core.managed.ready().unwrap();
    let target = sing_shell::settings::managed_path(&core.store.root, &prepared.reference).unwrap();
    let external = d.0.join("external");
    fs::write(&external, b"external").unwrap();
    std::os::unix::fs::symlink(&external, &target).unwrap();
    assert!(core.core.try_adopt().is_err());
    assert_eq!(fs::read(external).unwrap(), b"external");
    assert!(core.store.snapshot().managed.active.is_none());
}

#[test]
fn manual_retry_does_not_replace_a_referenced_previous_program() {
    let d = Directory::new();
    let core = TestCore::open(d.0.join("data")).unwrap();
    baseline(&core, &d);
    prepare(&core, &d, "1.14.4", Origin::Manual);
    let p = core.core.managed.ready().unwrap();
    let installed = sing_shell::settings::managed_path(&core.store.root, &p.reference).unwrap();
    fs::write(&installed, b"preserve referenced program").unwrap();
    core.store
        .core(core.store.snapshot().core_revision, |s| {
            s.managed.previous = Some(p.reference.clone());
            Ok(())
        })
        .unwrap();
    assert!(core.core.try_adopt().is_err());
    assert_eq!(fs::read(installed).unwrap(), b"preserve referenced program");
    assert_eq!(
        core.store.snapshot().managed.active.unwrap().version,
        "1.14.3"
    );
}

#[test]
fn explicit_takeover_does_not_overwrite_an_external_program_in_the_managed_target() {
    let d = Directory::new();
    let core = TestCore::open(d.0.join("data")).unwrap();
    prepare(&core, &d, "1.14.4", Origin::Required);
    let p = core.core.managed.ready().unwrap();
    let installed = sing_shell::settings::managed_path(&core.store.root, &p.reference).unwrap();
    fs::copy(env!("CARGO_BIN_EXE_core-fixture"), &installed).unwrap();
    fs::OpenOptions::new()
        .append(true)
        .open(&installed)
        .unwrap()
        .write_all(b"external variant")
        .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&installed, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let hash = package::hash(&installed).unwrap();
    core.select_core(&installed).unwrap();
    core.core.use_managed().unwrap();
    assert!(core.core.try_adopt().is_err());
    assert_eq!(package::hash(&installed).unwrap(), hash);
    assert!(matches!(
        core.store.snapshot().core_source,
        CoreSource::External { .. }
    ));
}

#[test]
fn equal_version_and_bytes_cannot_hide_changed_asset_identity() {
    let d = Directory::new();
    let core = TestCore::open(d.0.join("data")).unwrap();
    install(&core, &d, "1.14.4");
    prepare(&core, &d, "1.14.4", Origin::Manual);
    let mut p = core.core.managed.ready().unwrap();
    p.reference.source.as_mut().unwrap().asset_id += 1;
    core.core.managed.save_prepared(p).unwrap();
    let revision = core.store.snapshot().core_revision;
    assert!(core.core.try_adopt().is_err());
    assert_eq!(core.store.snapshot().core_revision, revision);
    assert_eq!(
        core.store.snapshot().managed.trial,
        Some(Trial::AwaitingStart)
    );
}

#[cfg(windows)]
#[test]
fn managed_path_rejects_windows_junctions_without_touching_the_target() {
    let d = Directory::new();
    let target = d.0.join("external");
    let link = d.0.join("junction");
    fs::create_dir(&target).unwrap();
    fs::write(target.join("keep"), b"keep").unwrap();
    let result = std::process::Command::new("powershell.exe").args(["-NoProfile","-NonInteractive","-Command","$ErrorActionPreference='Stop'; New-Item -ItemType Junction -Path $env:SING_SHELL_TEST_LINK -Target $env:SING_SHELL_TEST_TARGET | Out-Null"])
        .env("SING_SHELL_TEST_LINK",&link).env("SING_SHELL_TEST_TARGET",&target).output().unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let rejected = package::owned(&d.0, &link.join("keep")).is_err();
    fs::remove_dir(&link).unwrap();
    assert!(rejected);
    assert_eq!(fs::read(target.join("keep")).unwrap(), b"keep");
}
