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
