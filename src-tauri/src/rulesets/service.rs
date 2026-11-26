use super::*;
use crate::{
    app::{Admission, Kind, Receipt},
    core::CoreService,
    settings::SettingsStore,
};
use std::{collections::BTreeMap, sync::Mutex};
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Preference {
    pub source_id: String,
    pub automatic: bool,
    pub interval_hours: u16,
}
impl Preference {
    pub fn new(source_id: String) -> Self {
        Self {
            source_id,
            automatic: true,
            interval_hours: 24,
        }
    }
    pub fn validate(&self) -> Result<(), String> {
        definition(&self.source_id)?;
        if ![6, 24, 168].contains(&self.interval_hours) {
            return Err("更新周期必须为 6 小时、每天或每周".into());
        }
        Ok(())
    }
}
pub fn defaults() -> Vec<Preference> {
    ["cn-domain", "cn-ip"]
        .into_iter()
        .map(|id| Preference::new(id.into()))
        .collect()
}
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub phase: String,
    pub last_attempt: u64,
    pub last_success: u64,
    pub error: Option<String>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Source {
    pub definition: Definition,
    pub version: Option<Version>,
    pub status: Status,
    pub preference: Option<Preference>,
}
pub struct Service {
    store: Arc<SettingsStore>,
    gate: Arc<Admission>,
    core: Arc<CoreService>,
    status: Mutex<BTreeMap<String, Status>>,
    active: Mutex<Option<String>>,
}
impl Service {
    pub fn new(
        store: Arc<SettingsStore>,
        gate: Arc<Admission>,
        core: Arc<CoreService>,
    ) -> Arc<Self> {
        let mut state: BTreeMap<String, Status> = fs::read(store.root.join("rules/status.json"))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        for s in state.values_mut() {
            s.phase = "idle".into();
        }
        Arc::new(Self {
            store,
            gate,
            core,
            status: Mutex::new(state),
            active: Mutex::new(None),
        })
    }
    pub fn snapshot(&self) -> Vec<Source> {
        let settings = self.store.snapshot();
        let state = self.status.lock().unwrap();
        let index: Vec<Version> = fs::read(self.store.root.join("rules/index.json"))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        CATALOG
            .iter()
            .map(|d| Source {
                definition: d.clone(),
                version: index
                    .iter()
                    .find(|v| v.source_id == d.id)
                    .cloned()
                    .or_else(|| bundled().into_iter().find(|v| v.source_id == d.id)),
                status: state.get(d.id).cloned().unwrap_or_else(|| Status {
                    phase: "idle".into(),
                    ..Default::default()
                }),
                preference: settings
                    .rule_preferences
                    .iter()
                    .find(|p| p.source_id == d.id)
                    .cloned(),
            })
            .collect()
    }
    pub fn update(self: &Arc<Self>, id: String) -> Result<Receipt, String> {
        definition(&id)?;
        if self.store.error().is_some() {
            return Err("请先处理设置文件错误".into());
        }
        if !self
            .store
            .snapshot()
            .profile
            .routing
            .source_ids()
            .contains(&id)
        {
            return Err("请先保存包含此来源的规则表".into());
        }
        let op = self.gate.begin(Kind::Rules)?;
        let receipt = op.receipt();
        *self.active.lock().unwrap() = Some(id.clone());
        {
            let mut state = self.status.lock().unwrap();
            let entry = state.entry(id.clone()).or_default();
            entry.phase = "preparing".into();
            entry.last_attempt = crate::logs::now();
            entry.error = None;
        }
        self.core.runtime.events.push(
            "rules",
            "started",
            format!("正在更新 {}", definition(&id)?.name),
        );
        self.persist();
        let service = self.clone();
        std::thread::spawn(move || {
            let result = (|| {
                let s = service.store.snapshot();
                let preferences = s.update_preferences.network.clone();
                let commit = latest_commit(&id, preferences.clone(), &op.cancel)?;
                let v = prepare(&service.store.root, &id, &commit, preferences, &op.cancel)?;
                let core = service.core.selected(&op.cancel)?;
                validate_file(&service.store.root, &v, &core, &op.cancel)?;
                crate::core::package::cancelled(&op.cancel)?;
                if !service
                    .store
                    .snapshot()
                    .profile
                    .routing
                    .source_ids()
                    .contains(&id)
                {
                    return Err("来源已移除，更新已取消".into());
                }
                let index_path = service.store.root.join("rules/index.json");
                let mut index: Vec<Version> = fs::read(&index_path)
                    .ok()
                    .and_then(|b| serde_json::from_slice(&b).ok())
                    .unwrap_or_default();
                index.retain(|old| old.source_id != id);
                index.push(v);
                crate::settings::write_json(&index_path, &index)?;
                if !service.gate.runtime_busy() && !service.gate.core_fetching() {
                    let mut keep = index;
                    if let Some(last) = service.store.snapshot().startup.last_connection {
                        keep.extend(last.versions);
                    }
                    let _ = super::prune(&service.store.root, &keep);
                }
                Ok(())
            })();
            service.core.runtime.events.push(
                "rules",
                if result.is_ok() {
                    "succeeded"
                } else {
                    "failed"
                },
                format!(
                    "{}：{}",
                    id,
                    result
                        .as_ref()
                        .err()
                        .map(String::as_str)
                        .unwrap_or("已验证并准备，下次连接采用")
                ),
            );
            {
                let mut state = service.status.lock().unwrap();
                let entry = state.entry(id).or_default();
                entry.phase = if result.is_ok() { "idle" } else { "error" }.into();
                match result {
                    Ok(()) => {
                        entry.last_success = crate::logs::now();
                        entry.error = None
                    }
                    Err(e) => entry.error = Some(e.chars().take(1024).collect()),
                }
            }
            *service.active.lock().unwrap() = None;
            service.persist();
            drop(op);
        });
        Ok(receipt)
    }
    fn persist(&self) {
        let directory = self.store.root.join("rules");
        if crate::private_files::directory(&directory).is_ok() {
            let _ = crate::settings::write_json(
                &directory.join("status.json"),
                &*self.status.lock().unwrap(),
            );
        }
    }
    pub fn reconcile(&self) {
        if self.active.lock().unwrap().as_ref().is_some_and(|id| {
            !self
                .store
                .snapshot()
                .profile
                .routing
                .source_ids()
                .contains(id)
        }) {
            self.cancel();
        }
    }
    pub fn cancel(&self) {
        self.gate.cancel_kind(|k| k == Kind::Rules);
    }
    pub fn tick(self: &Arc<Self>) {
        if self.gate.closing()
            || self.store.error().is_some()
            || self.active.lock().unwrap().is_some()
        {
            return;
        }
        let now = crate::logs::now();
        for source in self.snapshot() {
            if let Some(pref) = source.preference {
                let due = source
                    .status
                    .last_success
                    .saturating_add(pref.interval_hours as u64 * 3_600_000);
                let backoff = source.status.last_attempt.saturating_add(15 * 60_000);
                if pref.automatic && now >= due && now >= backoff {
                    let _ = self.update(pref.source_id);
                    break;
                }
            }
        }
    }
}
