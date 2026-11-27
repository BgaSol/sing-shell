use sing_shell::{
    profile::{
        self,
        routing::{Action, Match, Policy},
    },
    rulesets,
    settings::SettingsStore,
};
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Directory(PathBuf);
impl Directory {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!(
            "ss-rules-{}-{}-{}",
            std::process::id(),
            sing_shell::logs::now(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&p).unwrap();
        Self(p)
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
#[test]
fn routing_commit_preserves_other_source_preferences_and_rejects_stale_writes() {
    let d = Directory::new();
    let store = SettingsStore::open(d.0.clone()).unwrap();
    let mut plan = store.snapshot().profile.routing;
    store
        .rule_preference(rulesets::service::Preference {
            source_id: "cn-ip".into(),
            automatic: false,
            interval_hours: 168,
        })
        .unwrap();
    plan.policies.retain(|p| p.id != "cn-domain");
    plan.policies.push(Policy {
        id: "github".into(),
        name: "GitHub".into(),
        enabled: true,
        exit: Action::Proxy,
        r#match: Match::Source {
            source_id: "github".into(),
        },
    });
    store.routing(0, plan).unwrap();
    let s = store.snapshot();
    assert_eq!(s.rule_preferences.len(), 2);
    let ip = s
        .rule_preferences
        .iter()
        .find(|p| p.source_id == "cn-ip")
        .unwrap();
    assert!(!ip.automatic);
    assert_eq!(ip.interval_hours, 168);
    assert!(
        s.rule_preferences
            .iter()
            .find(|p| p.source_id == "github")
            .unwrap()
            .automatic
    );
    assert_eq!(s.nodes_revision, 0);
    assert_eq!(s.preferences_revision, 0);
    assert!(store
        .rule_preference(rulesets::service::Preference::new("cn-domain".into()))
        .is_err());
    assert!(store.routing(0, Default::default()).is_err());
}
#[test]
fn frozen_input_ignores_metadata_and_disabled_policies_but_tracks_priority_and_content() {
    let d = Directory::new();
    let node = profile::import::preview("trojan://private@example.com:443", &[])
        .unwrap()
        .nodes
        .remove(0);
    let routing = profile::routing::Routing::default();
    let mut input = sing_shell::profile::ConnectionInput {
        options: Default::default(),
        node,
        routing: routing.clone(),
        versions: rulesets::selected(&d.0, &routing).unwrap(),
        core_version: "1.14.4".into(),
    };
    let before = input.fingerprint();
    input.node.name = "renamed".into();
    input.node.tags = vec!["工作".into()];
    input.routing.policies[0].name = "显示名".into();
    assert_eq!(before, input.fingerprint());
    input.routing.policies.swap(0, 1);
    assert_ne!(before, input.fingerprint());
    input.routing.policies.swap(0, 1);
    input.versions[0].sha256 = "0".repeat(64);
    assert_ne!(before, input.fingerprint());
    let content_fingerprint = input.fingerprint();
    input.options.ipv4_compatibility = true;
    assert_ne!(content_fingerprint, input.fingerprint());
}

#[test]
fn connection_options_are_independent_and_legacy_schema_five_remains_readable() {
    use sing_shell::profile::ConnectionOptions;
    let d = Directory::new();
    let store = SettingsStore::open(d.0.clone()).unwrap();
    let before = store.snapshot();
    let changed = store
        .connection_options(
            0,
            ConnectionOptions {
                ipv4_compatibility: true,
            },
        )
        .unwrap();
    assert_eq!(changed.connection_revision, 1);
    assert_eq!(changed.nodes_revision, before.nodes_revision);
    assert_eq!(changed.routing_revision, before.routing_revision);
    assert_eq!(changed.preferences_revision, before.preferences_revision);
    assert!(store.connection_options(0, Default::default()).is_err());
    let mut legacy = serde_json::to_value(before).unwrap();
    legacy.as_object_mut().unwrap().remove("connectionRevision");
    legacy["profile"]
        .as_object_mut()
        .unwrap()
        .remove("connection");
    let restored: sing_shell::settings::Settings = serde_json::from_value(legacy).unwrap();
    assert_eq!(restored.connection_revision, 0);
    assert!(!restored.profile.connection.ipv4_compatibility);
}
#[test]
fn fixed_sources_reject_paths_and_corrupted_content_without_reordering_settings() {
    let d = Directory::new();
    assert!(rulesets::definition("../github").is_err());
    assert!(rulesets::validate_commit("main").is_err());
    let plan = profile::routing::Routing::default();
    let versions = rulesets::selected(&d.0, &plan).unwrap();
    let file = rulesets::materialize_at(&d.0, &versions[0]).unwrap();
    fs::write(file, b"corrupt").unwrap();
    let restored = rulesets::materialize_at(&d.0, &versions[0]).unwrap();
    assert_eq!(
        rulesets::digest(&fs::read(restored).unwrap()),
        versions[0].sha256
    );
    let mut wrong = versions[0].clone();
    wrong.sha256 = "0".repeat(64);
    assert!(rulesets::materialize_at(&d.0, &wrong).is_err());
}
#[test]
fn cache_pruning_keeps_pinned_and_bundled_versions() {
    let d = Directory::new();
    let stale = d.0.join("rules/content/github").join("1".repeat(40));
    let pinned = d.0.join("rules/content/github").join("2".repeat(40));
    fs::create_dir_all(&stale).unwrap();
    fs::create_dir_all(&pinned).unwrap();
    let mut keep = rulesets::bundled()
        .into_iter()
        .find(|v| v.source_id == "github")
        .unwrap();
    keep.commit = "2".repeat(40);
    rulesets::prune(&d.0, &[keep]).unwrap();
    assert!(!stale.exists());
    assert!(pinned.exists());
}
#[test]
#[ignore = "requires official core in SING_SHELL_TEST_CORE"]
fn official_core_must_load_every_bundled_source_and_reject_malformed_srs() {
    let d = Directory::new();
    let core = std::env::var_os("SING_SHELL_TEST_CORE").unwrap();
    let cancel = std::sync::atomic::AtomicBool::new(false);
    for v in rulesets::bundled() {
        rulesets::validate_file(&d.0, &v, &PathBuf::from(&core), &cancel).unwrap();
    }
    let bad = b"SRSinvalid";
    let v = rulesets::Version {
        source_id: "github".into(),
        commit: "3".repeat(40),
        sha256: rulesets::digest(bad),
        size: bad.len() as u64,
    };
    let dir = d.0.join("rules/content/github").join(&v.commit);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("rules.srs"), bad).unwrap();
    assert!(rulesets::validate_file(&d.0, &v, &PathBuf::from(&core), &cancel).is_err());
    assert!(!d.0.join("rules/index.json").exists());
}
