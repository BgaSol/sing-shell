#![cfg(feature = "test-fixtures")]
mod support;
use sing_shell::{
    app::Kind,
    session::{Action, Restore},
    settings::SettingsStore,
};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};
use support::TestCore;
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Directory(PathBuf);
impl Directory {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "sing-shell-restore-{}-{}-{}",
            std::process::id(),
            sing_shell::logs::now(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        Self(root)
    }
    fn configured(&self, value: serde_json::Value) -> TestCore {
        let core = TestCore::open(self.0.join("data")).unwrap();
        core.select_core(Path::new(env!("CARGO_BIN_EXE_core-fixture")))
            .unwrap();
        let config = self.0.join("配置 space.json");
        fs::write(&config, value.to_string()).unwrap();
        core.import_config(&config).unwrap();
        core
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
#[test]
fn successful_run_survives_internal_cleanup_and_restores_once_without_frontend() {
    let d = Directory::new();
    {
        let c = d.configured(serde_json::json!({}));
        c.session.set_restore(true).unwrap();
        c.start().unwrap();
        assert!(c.store.snapshot().startup.resume_core);
        c.session.stop_internal().unwrap();
        assert!(c.store.snapshot().startup.resume_core);
    }
    let c = TestCore::open(d.0.join("data")).unwrap();
    assert_eq!(c.session.snapshot().restore, Restore::Waiting);
    c.session.restore_tick();
    wait(|| c.session.snapshot().restore == Restore::Finished);
    assert!(c.runtime.snapshot().pid.is_some());
    c.session.stop_internal().unwrap();
    c.session.restore_tick();
    assert!(c.runtime.snapshot().pid.is_none());
    assert!(c.store.snapshot().startup.resume_core);
    c.stop().unwrap();
    assert!(!c.store.snapshot().startup.resume_core);
}
#[test]
fn stop_cancels_start_without_competing_with_diagnostics() {
    let d = Directory::new();
    let c = d.configured(serde_json::json!({"checkDelayMs":700}));
    c.session.action(Action::Start).unwrap();
    let diagnostic = c.core.gate.begin(Kind::Diagnostic).unwrap();
    c.session.action(Action::Stop).unwrap();
    wait(|| !c.core.gate.runtime_busy());
    assert!(c.runtime.snapshot().pid.is_none());
    assert!(!c.store.snapshot().startup.resume_core);
    drop(diagnostic);
}
#[test]
fn pending_restore_is_revoked_by_stop_selection_or_disabling() {
    for cancel in 0..3 {
        let d = Directory::new();
        {
            let c = d.configured(serde_json::json!({}));
            c.session.set_restore(true).unwrap();
            c.store.resume(true).unwrap();
        }
        let c = TestCore::open(d.0.join("data")).unwrap();
        match cancel {
            0 => c.stop().unwrap(),
            1 => {
                c.session.set_restore(false).unwrap();
            }
            _ => c
                .import_config(&c.store.snapshot().config_path.unwrap())
                .unwrap(),
        }
        c.session.restore_tick();
        thread::sleep(Duration::from_millis(100));
        assert!(c.runtime.snapshot().pid.is_none());
        assert_eq!(c.session.snapshot().restore, Restore::Finished);
    }
}
#[test]
fn failed_restore_is_consumed_and_does_not_retry_changed_configuration() {
    let d = Directory::new();
    let config;
    {
        let c = d.configured(serde_json::json!({}));
        config = c.store.snapshot().config_path.unwrap();
        c.session.set_restore(true).unwrap();
        c.store.resume(true).unwrap();
    }
    fs::write(&config, "{invalid").unwrap();
    let c = TestCore::open(d.0.join("data")).unwrap();
    c.session.restore_tick();
    wait(|| c.session.snapshot().restore == Restore::Finished);
    assert!(c.runtime.snapshot().pid.is_none());
    assert!(c.runtime.snapshot().error.unwrap().contains("自动恢复失败"));
    fs::write(config, "{}").unwrap();
    c.session.restore_tick();
    thread::sleep(Duration::from_millis(100));
    assert!(c.runtime.snapshot().pid.is_none());
}
#[test]
fn migration_preserves_backup_and_independent_writes_preserve_network_draft_revision() {
    let d = Directory::new();
    let data = d.0.join("data");
    fs::create_dir(&data).unwrap();
    let old = serde_json::json!({"schemaVersion":2,"coreRevision":8,"preferencesRevision":5,"coreSource":{"mode":"external","path":"old core"},"managed":{"active":null,"previous":null,"trial":null,"blocked":null},"configPath":"old config","pendingConfigPath":null,"updatePreferences":{"automatic":false,"automaticCore":false,"network":{"revision":0,"fallbackEnabled":true,"disabledBuiltins":[],"customMirrors":[]}}});
    fs::write(data.join("settings.json"), old.to_string()).unwrap();
    {
        let store = Arc::new(SettingsStore::open(data.clone()).unwrap());
        assert!(store.error.is_none());
        let baseline = store.snapshot();
        assert_eq!(baseline.schema_version, 3);
        assert!(!baseline.startup.resume_core);
        store.restore_preference(true).unwrap();
        store.resume(true).unwrap();
        store.background_notice().unwrap();
        store.preferences(5, baseline.update_preferences).unwrap();
        assert!(store.snapshot().startup.resume_core);
        assert_eq!(store.snapshot().core_revision, 8);
    }
    let backup = fs::read(data.join("settings.v2.backup.json")).unwrap();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&backup).unwrap(),
        old
    );
    let store = SettingsStore::open(data.clone()).unwrap();
    assert!(store.snapshot().startup.restore_core_on_launch);
    assert_eq!(
        fs::read(data.join("settings.v2.backup.json")).unwrap(),
        backup
    );
}
#[test]
fn failed_intent_save_still_stops_real_process() {
    let d = Directory::new();
    let c = d.configured(serde_json::json!({}));
    c.start().unwrap();
    fs::create_dir(c.store.root.join("settings.json.tmp")).unwrap();
    assert!(c.stop().unwrap_err().contains("下次启动仍可能恢复"));
    assert!(c.runtime.snapshot().pid.is_none());
}
#[test]
fn migration_does_not_replace_unrecognized_legacy_fields() {
    for field in ["startup", "desktop"] {
        let d = Directory::new();
        let mut value = serde_json::to_value(sing_shell::settings::Settings::default()).unwrap();
        value["schemaVersion"] = serde_json::json!(2);
        value.as_object_mut().unwrap().remove("startup");
        value.as_object_mut().unwrap().remove("desktop");
        value[field] = serde_json::json!({"unrecognized":"keep this data"});
        let bytes = value.to_string();
        fs::write(d.0.join("settings.json"), &bytes).unwrap();
        let store = SettingsStore::open(d.0.clone()).unwrap();
        assert!(store.error.is_some());
        assert!(store.resume(false).is_err());
        assert_eq!(
            fs::read_to_string(d.0.join("settings.json")).unwrap(),
            bytes
        );
    }
}
#[test]
fn disabling_restore_does_not_cancel_a_manual_start() {
    let d = Directory::new();
    let c = d.configured(serde_json::json!({"checkDelayMs":500}));
    c.session.action(Action::Start).unwrap();
    c.session.set_restore(false).unwrap();
    wait(|| !c.core.gate.runtime_busy());
    assert!(c.runtime.snapshot().pid.is_some());
    c.stop().unwrap();
}
#[test]
fn cancelling_required_preparation_consumes_a_waiting_restore() {
    let d = Directory::new();
    {
        let c = TestCore::open(d.0.join("data")).unwrap();
        c.session.set_restore(true).unwrap();
        c.store.resume(true).unwrap();
    }
    let c = TestCore::open(d.0.join("data")).unwrap();
    assert_eq!(c.session.snapshot().restore, Restore::Waiting);
    c.session.cancel_core();
    c.session.restore_tick();
    assert_eq!(c.session.snapshot().restore, Restore::Finished);
    assert!(c.runtime.snapshot().pid.is_none());
}
