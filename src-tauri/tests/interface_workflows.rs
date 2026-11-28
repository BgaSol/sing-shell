#![cfg(feature = "test-fixtures")]
mod support;
use sing_shell::{
    profile::{
        change::Change,
        routing::{Action, Match, Policy},
    },
    settings::SettingsStore,
    updates::Component,
};
struct Store {
    root: std::path::PathBuf,
    store: Option<SettingsStore>,
}
static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
impl Store {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "sing-shell-interface-{}-{}-{}",
            std::process::id(),
            sing_shell::logs::now(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let store = SettingsStore::open(root.clone()).unwrap();
        Self {
            root,
            store: Some(store),
        }
    }
}
impl Drop for Store {
    fn drop(&mut self) {
        self.store.take();
        let _ = std::fs::remove_dir_all(&self.root);
    }
}
fn custom() -> Policy {
    Policy {
        id: "test-rule".into(),
        name: "".into(),
        enabled: true,
        exit: Action::Direct,
        bypass_tun: false,
        r#match: Match::Custom {
            domains: vec![],
            ip_cidrs: vec!["192.0.2.7".into()],
        },
    }
}
#[test]
fn rule_operations_are_atomic_and_undo_keeps_later_changes_and_source_preferences() {
    let c = Store::new();
    let s = c.store.as_ref().unwrap();
    let mut pref = s.snapshot().rule_preferences[0].clone();
    pref.automatic = false;
    pref.interval_hours = 168;
    s.rule_preference(pref.clone()).unwrap();
    let (_, removed) = s
        .change_routing(
            0,
            Change::Remove {
                id: "cn-domain".into(),
            },
        )
        .unwrap();
    let (saved, _) = s
        .change_routing(
            1,
            Change::Default {
                exit: Action::Direct,
            },
        )
        .unwrap();
    assert_eq!(saved.profile.routing.default_exit, Action::Direct);
    let (saved, _) = s
        .change_routing(
            2,
            Change::Restore {
                removed: removed.unwrap(),
            },
        )
        .unwrap();
    assert_eq!(saved.profile.routing.default_exit, Action::Direct);
    assert_eq!(saved.profile.routing.policies[0].id, "cn-domain");
    assert!(
        !saved
            .rule_preferences
            .iter()
            .find(|p| p.source_id == pref.source_id)
            .unwrap()
            .automatic
    );
    let before = std::fs::read(c.root.join("settings.json")).unwrap();
    assert!(s
        .change_routing(
            3,
            Change::AddSources {
                ids: vec!["google".into(), "google".into()]
            }
        )
        .is_err());
    assert_eq!(std::fs::read(c.root.join("settings.json")).unwrap(), before);
    assert_eq!(s.snapshot().routing_revision, 3);
    assert!(s
        .change_routing(
            2,
            Change::Default {
                exit: Action::Proxy
            }
        )
        .is_err());
}
#[test]
fn custom_save_normalizes_once_and_proxy_clears_bypass() {
    let c = Store::new();
    let s = c.store.as_ref().unwrap();
    let mut p = custom();
    p.bypass_tun = true;
    let (saved, _) = s.change_routing(0, Change::Add { policy: p }).unwrap();
    let p = saved.profile.routing.policies.last().unwrap();
    assert!(!p.name.is_empty());
    assert!(p.bypass_tun);
    let (saved, _) = s
        .change_routing(
            1,
            Change::Exit {
                id: "test-rule".into(),
                exit: Action::Proxy,
            },
        )
        .unwrap();
    assert!(!saved.profile.routing.policies.last().unwrap().bypass_tun);
    let before = saved.routing_revision;
    let mut bad = custom();
    bad.id = "bad".into();
    bad.r#match = Match::Custom {
        domains: vec![],
        ip_cidrs: vec![],
    };
    assert!(s
        .change_routing(before, Change::Add { policy: bad })
        .is_err());
    assert_eq!(s.snapshot().routing_revision, before);
}
#[test]
fn independent_update_toggles_do_not_overwrite_an_open_network_editor() {
    let c = Store::new();
    let s = c.store.as_ref().unwrap();
    let mut network = s.snapshot().update_preferences.network;
    s.automatic_updates(Component::App, true, false).unwrap();
    network.fallback_enabled = false;
    let saved = s.network_preferences(0, network.clone()).unwrap();
    assert!(!saved.update_preferences.automatic);
    assert!(!saved.update_preferences.network.fallback_enabled);
    assert_eq!(saved.update_preferences.network.revision, 1);
    s.automatic_updates(Component::Engine, true, false).unwrap();
    assert!(s.network_preferences(0, network).is_err());
    assert!(!s.snapshot().update_preferences.automatic_core);
}
#[test]
fn mode_and_resume_are_committed_together_and_raw_routing_is_read_only() {
    let c = Store::new();
    let s = c.store.as_ref().unwrap();
    s.resume(true).unwrap();
    s.change_mode(0, sing_shell::profile::Mode::ExternalConfig, false, None)
        .unwrap();
    let saved = s.snapshot();
    assert!(!saved.startup.resume_core);
    assert_eq!(saved.nodes_revision, 1);
    assert!(s
        .change_routing(
            0,
            Change::Default {
                exit: Action::Direct
            }
        )
        .is_err());
    assert_eq!(s.snapshot().routing_revision, 0);
}

#[test]
fn mode_switch_checks_official_core_before_stopping_and_then_releases_the_real_process() {
    let directory = Store::new();
    let core = support::TestCore::open(directory.root.join("session")).unwrap();
    let fixture = std::path::Path::new(env!("CARGO_BIN_EXE_core-fixture"));
    core.select_core(fixture).unwrap();
    let config = directory.root.join("config.json");
    std::fs::write(&config, "{}").unwrap();
    core.import_config(&config).unwrap();
    core.start().unwrap();
    let before = core.store.snapshot();
    let pid = core.runtime.snapshot().pid.unwrap();
    assert!(core
        .session
        .switch_mode(sing_shell::profile::Mode::Generated, before.nodes_revision)
        .is_err());
    assert_eq!(core.runtime.snapshot().pid, Some(pid));
    let reference = sing_shell::settings::ManagedRef {
        version: "1.14.4".into(),
        target: sing_shell::TARGET.into(),
        hash: Some(sing_shell::core::package::hash(fixture).unwrap()),
        source: None,
    };
    let program = sing_shell::settings::managed_path(&core.store.root, &reference).unwrap();
    std::fs::create_dir_all(program.parent().unwrap()).unwrap();
    std::fs::copy(fixture, &program).unwrap();
    core.store
        .core(before.core_revision, |s| {
            s.managed.active = Some(reference);
            Ok(())
        })
        .unwrap();
    let saved = core.store.snapshot();
    core.session
        .switch_mode(sing_shell::profile::Mode::Generated, saved.nodes_revision)
        .unwrap();
    assert!(core.runtime.snapshot().pid.is_none());
    let saved = core.store.snapshot();
    assert!(saved.managed());
    assert!(saved.profile.mode == sing_shell::profile::Mode::Generated);
    assert!(!saved.startup.resume_core);
    drop(core);
}
