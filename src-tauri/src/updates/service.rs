use super::{
    application, engine,
    network::{self, Transport},
    release::{self, Candidate},
    Component,
};
use crate::{
    app::{Admission, Kind, Operation, Receipt},
    logs::now,
    runtime::RuntimeService,
    settings::{stable, write_json, Engine, SettingsStore},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Item {
    pub phase: String,
    pub candidate: Option<Candidate>,
    pub error: Option<String>,
    pub error_stage: Option<String>,
    pub downloaded_bytes: u64,
    pub total_bytes: Option<u64>,
    pub network: Option<network::Attempt>,
}
#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub items: BTreeMap<Component, Item>,
    pub last_checked_at: u64,
    pub next_check_at: u64,
    pub checking: bool,
    pub initialized: bool,
}
impl Snapshot {
    fn restore_transient(&mut self, at: u64) {
        self.checking = false;
        self.initialized = false;
        for item in self.items.values_mut() {
            if ["ready", "checking", "downloading", "preparing"].contains(&item.phase.as_str()) {
                item.phase = if item.candidate.is_some() {
                    "available"
                } else {
                    "idle"
                }
                .into();
            }
            item.network = None;
        }
        self.next_check_at = self.next_check_at.min(at + 24 * 3600 * 1000);
    }
    fn record_check(&mut self, at: u64, successes: u32, failures: u32) {
        if successes > 0 {
            self.last_checked_at = at;
        }
        self.next_check_at = at
            + if failures > 0 {
                15 * 60 * 1000
            } else {
                24 * 3600 * 1000
            };
    }
}
#[derive(Clone, Deserialize, Serialize)]
pub struct Prepared {
    pub schema_version: u32,
    pub candidate: Candidate,
    pub from_version: String,
    pub engine_hash: Option<String>,
}
pub struct UpdateService {
    pub store: Arc<SettingsStore>,
    pub runtime: Arc<RuntimeService>,
    pub gate: Arc<Admission>,
    state: Mutex<Snapshot>,
    network: Arc<network::State>,
    app_version: String,
    stop: AtomicBool,
    started: AtomicBool,
    cache_writer: Mutex<()>,
}
impl UpdateService {
    pub fn new(
        store: Arc<SettingsStore>,
        runtime: Arc<RuntimeService>,
        gate: Arc<Admission>,
        version: String,
    ) -> Arc<Self> {
        Arc::new(Self {
            store,
            runtime,
            gate,
            state: Mutex::new(Default::default()),
            network: Arc::new(Default::default()),
            app_version: version,
            stop: AtomicBool::new(false),
            started: AtomicBool::new(false),
            cache_writer: Mutex::new(()),
        })
    }
    pub fn snapshot(&self) -> Snapshot {
        self.state.lock().unwrap().clone()
    }
    fn item(&self, c: Component, update: impl FnOnce(&mut Item)) {
        update(self.state.lock().unwrap().items.entry(c).or_default());
    }
    fn error(&self, c: Component, stage: &str, e: String) {
        self.runtime.logs.push("updates", e.clone());
        self.item(c, |v| {
            v.phase = "error".into();
            v.error = Some(e);
            v.error_stage = Some(stage.into());
        });
    }
    fn cache(&self, c: Component) -> PathBuf {
        self.store.root.join("updates").join(c.cache())
    }
    fn persist(&self) {
        let _writer = self.cache_writer.lock().unwrap();
        let mut value = self.snapshot();
        value.checking = false;
        for item in value.items.values_mut() {
            if ["checking", "downloading", "preparing"].contains(&item.phase.as_str()) {
                item.phase = "idle".into();
            }
            item.network = None;
        }
        if let Err(e) = write_json(&self.store.root.join("updates.json"), &value) {
            self.runtime.logs.push("updates", e);
        }
    }
    pub fn current(&self, c: Component) -> String {
        if c == Component::App {
            return self.app_version.clone();
        }
        match self.store.snapshot().engine {
            Some(Engine::Managed { version, .. }) => version,
            _ => "0.0.0".into(),
        }
    }
    pub fn start(self: &Arc<Self>, handle: tauri::AppHandle, automatic: bool) {
        if self.started.swap(true, Ordering::AcqRel) {
            return;
        }
        let service = self.clone();
        tauri::async_runtime::spawn_blocking(move || {
            let initialization = match service.gate.begin(Kind::Check) {
                Ok(op) => op,
                Err(_) => return,
            };
            service.restore(&handle);
            drop(initialization);
            if !automatic {
                return;
            }
            let weak = Arc::downgrade(&service);
            drop(service);
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(Duration::from_secs(10)).await;
                loop {
                    let Some(service) = weak.upgrade() else {
                        break;
                    };
                    if service.stop.load(Ordering::Acquire) {
                        break;
                    }
                    if service.store.error.is_none()
                        && service.store.snapshot().update_preferences.automatic
                        && service.snapshot().next_check_at <= now()
                        && !service.gate.closing()
                    {
                        let _ = service.check(None, true);
                    }
                    drop(service);
                    tokio::time::sleep(Duration::from_secs(60)).await;
                }
            });
        });
    }
    fn restore(&self, handle: &tauri::AppHandle) {
        if let Ok(bytes) = fs::read(self.store.root.join("updates.json")) {
            match serde_json::from_slice::<Snapshot>(&bytes) {
                Ok(mut state) => {
                    state.restore_transient(now());
                    *self.state.lock().unwrap() = state;
                }
                Err(e) => self
                    .runtime
                    .logs
                    .push("updates", format!("更新缓存无法读取：{e}")),
            }
        }
        for c in [Component::App, Component::Engine] {
            let path = self.cache(c).join("prepared.json");
            if path.exists() {
                let result = (|| {
                    let prepared: Prepared =
                        serde_json::from_slice(&fs::read(&path).map_err(|e| e.to_string())?)
                            .map_err(|e| e.to_string())?;
                    if prepared.schema_version != 1
                        || prepared.candidate.component != c
                        || prepared.candidate.target != crate::TARGET
                    {
                        return Err("缓存目标不符".into());
                    }
                    if stable(&prepared.candidate.version)? <= stable(&self.current(c))? {
                        fs::remove_file(&path).map_err(|e| e.to_string())?;
                        return Ok(());
                    }
                    if c == Component::App && prepared.from_version != self.app_version {
                        return Err("缓存不属于当前应用版本".into());
                    }
                    self.verify(handle, &prepared)?;
                    self.item(c, |v| {
                        v.phase = "ready".into();
                        v.candidate = Some(prepared.candidate);
                        v.error = None;
                    });
                    Ok(())
                })();
                if let Err(e) = result {
                    self.error(c, "download", e);
                }
            }
        }
        if !matches!(self.store.snapshot().engine, Some(Engine::Managed { .. })) {
            self.item(Component::Engine, |v| {
                if v.phase != "ready" {
                    v.phase = "external".into();
                }
            });
        }
        #[cfg(target_os = "linux")]
        if std::env::var_os("APPIMAGE").is_none() {
            self.item(Component::App, |v| v.phase = "unsupported".into());
        }
        self.state.lock().unwrap().initialized = true;
    }
    pub fn check(
        self: &Arc<Self>,
        component: Option<Component>,
        automatic: bool,
    ) -> Result<Receipt, String> {
        if automatic
            && (!self.store.snapshot().update_preferences.automatic
                || self.snapshot().next_check_at > now())
        {
            return Err("自动检查尚未到期或已关闭".into());
        }
        if !self.snapshot().initialized {
            return Err("更新服务正在初始化".into());
        }
        let op = self.gate.begin(Kind::Check)?;
        let receipt = op.receipt();
        let service = self.clone();
        tauri::async_runtime::spawn_blocking(move || {
            service.state.lock().unwrap().checking = true;
            let mut failures = 0;
            let mut successes = 0;
            for c in component
                .map(|c| vec![c])
                .unwrap_or_else(|| vec![Component::App, Component::Engine])
            {
                let item = service
                    .snapshot()
                    .items
                    .get(&c)
                    .cloned()
                    .unwrap_or_default();
                if ["ready", "unsupported"].contains(&item.phase.as_str()) {
                    continue;
                }
                if c == Component::Engine
                    && component.is_none()
                    && !matches!(
                        service.store.snapshot().engine,
                        Some(Engine::Managed { .. })
                    )
                {
                    continue;
                }
                service.item(c, |v| {
                    v.phase = "checking".into();
                    v.error = None;
                });
                let result = service
                    .transport(c, &op)
                    .and_then(|client| release::discover(&client, c, &service.current(c)));
                match result {
                    Ok(candidate) => {
                        successes += 1;
                        service.item(c, |v| {
                            v.phase = if candidate.is_some() {
                                "available"
                            } else {
                                "current"
                            }
                            .into();
                            v.candidate = candidate;
                            v.error = None;
                            v.error_stage = None;
                        });
                    }
                    Err(e) => {
                        failures += 1;
                        service.error(c, "check", e);
                    }
                }
            }
            {
                let mut state = service.state.lock().unwrap();
                state.checking = false;
                if component.is_none() {
                    state.record_check(now(), successes, failures);
                }
            }
            service.persist();
            drop(op);
        });
        Ok(receipt)
    }
    fn transport(self: &Arc<Self>, c: Component, op: &Operation) -> Result<Transport, String> {
        let weak = Arc::downgrade(self);
        let cancel = op.cancel.clone();
        Transport::new(
            self.store.snapshot().update_preferences.network,
            self.network.clone(),
        )
        .map(|client| {
            client.observed(
                Arc::new(move |attempt| {
                    if let Some(service) = weak.upgrade() {
                        service.item(c, |v| v.network = Some(attempt));
                    }
                }),
                Arc::new(move || cancel.load(Ordering::Acquire)),
            )
        })
    }
    pub fn download(
        self: &Arc<Self>,
        handle: tauri::AppHandle,
        components: Vec<Component>,
    ) -> Result<Vec<Receipt>, String> {
        if components.is_empty() || components.len() > 2 {
            return Err("请选择应用或受管核心".into());
        }
        let mut ops = Vec::new();
        for c in components {
            let item = self
                .snapshot()
                .items
                .get(&c)
                .cloned()
                .ok_or("请先检查更新")?;
            if !["available", "error"].contains(&item.phase.as_str())
                || item.error_stage.as_deref() == Some("check")
            {
                return Err("当前更新不可下载".into());
            }
            let candidate = item.candidate.ok_or("请先检查更新")?;
            ops.push((c, candidate, self.gate.begin(Kind::Download(c))?));
        }
        let receipts = ops.iter().map(|(_, _, op)| op.receipt()).collect();
        for (c, candidate, op) in ops {
            let service = self.clone();
            let handle = handle.clone();
            self.item(c, |v| {
                v.phase = "downloading".into();
                v.error = None;
                v.error_stage = None;
            });
            tauri::async_runtime::spawn_blocking(move || {
                if let Err(e) = service.prepare(&handle, candidate, &op) {
                    service.error(c, "download", e);
                }
                service.persist();
                drop(op);
            });
        }
        Ok(receipts)
    }
    fn prepare(
        self: &Arc<Self>,
        handle: &tauri::AppHandle,
        candidate: Candidate,
        op: &Operation,
    ) -> Result<(), String> {
        let c = candidate.component;
        let client = self.transport(c, op)?;
        release::recheck(&client, &candidate)?;
        let directory = self.cache(c);
        fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
        let part = directory.join("package.part");
        if part.exists() {
            fs::remove_file(&part).map_err(|e| e.to_string())?;
        }
        client.download(
            &candidate.url,
            &part,
            |count, total| {
                self.item(c, |v| {
                    v.downloaded_bytes = count;
                    v.total_bytes = total;
                })
            },
            |path, sha| {
                if sha != candidate.sha256
                    || fs::metadata(path)
                        .map_err(|e| network::Error::Local(e.to_string()))?
                        .len()
                        != candidate.size
                {
                    return Err(network::Error::remote("下载包摘要或大小不符"));
                }
                if c == Component::App {
                    application::verify(handle, &candidate, path)
                        .map_err(network::Error::remote)?;
                }
                Ok(())
            },
        )?;
        if op.cancel.load(Ordering::Acquire) || self.gate.closing() {
            let _ = fs::remove_file(part);
            return Err("操作已取消".into());
        }
        self.item(c, |v| v.phase = "preparing".into());
        let engine_hash = if c == Component::Engine {
            Some(engine::prepare(
                &self.store,
                &self.runtime,
                &candidate,
                &part,
                &op.cancel,
            )?)
        } else {
            None
        };
        if op.cancel.load(Ordering::Acquire) || self.gate.closing() {
            return Err("操作已取消".into());
        }
        fs::rename(part, directory.join("package.bin")).map_err(|e| e.to_string())?;
        let prepared = Prepared {
            schema_version: 1,
            candidate: candidate.clone(),
            from_version: self.current(c),
            engine_hash,
        };
        write_json(&directory.join("prepared.json"), &prepared)?;
        self.item(c, |v| {
            v.phase = "ready".into();
            v.candidate = Some(candidate);
            v.error = None;
            v.error_stage = None;
        });
        Ok(())
    }
    pub fn prepared(&self, c: Component) -> Result<Prepared, String> {
        let prepared: Prepared = serde_json::from_slice(
            &fs::read(self.cache(c).join("prepared.json")).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        if prepared.candidate.component != c {
            return Err("缓存组件不符".into());
        }
        Ok(prepared)
    }
    pub fn verify(&self, handle: &tauri::AppHandle, p: &Prepared) -> Result<(), String> {
        if p.schema_version != 1 || p.candidate.target != crate::TARGET {
            return Err("缓存格式或平台不符".into());
        }
        if stable(&p.candidate.version)? <= stable(&self.current(p.candidate.component))? {
            return Err("缓存版本不高于当前版本，请重新检查".into());
        }
        if p.candidate.component == Component::App && p.from_version != self.app_version {
            return Err("缓存不属于当前应用版本".into());
        }
        let path = self.cache(p.candidate.component).join("package.bin");
        if engine::hash(&path)? != p.candidate.sha256
            || fs::metadata(&path).map_err(|e| e.to_string())?.len() != p.candidate.size
        {
            return Err("准备包已损坏，请重新下载".into());
        }
        match p.candidate.component {
            Component::App => application::verify(handle, &p.candidate, &path),
            Component::Engine => engine::verify(
                &self.store,
                &self.runtime,
                &p.candidate,
                p.engine_hash.as_deref().ok_or("缺少核心摘要")?,
                &AtomicBool::new(false),
            ),
        }
    }
    pub fn apply_engine(&self, p: &Prepared) -> Result<(), String> {
        let settings = self.store.snapshot();
        let previous = match &settings.engine {
            Some(Engine::Managed { version, .. }) => Some(version.clone()),
            _ => None,
        };
        self.store.save(settings.revision, |s| {
            s.engine = Some(Engine::Managed {
                version: p.candidate.version.clone(),
                target: p.candidate.target.clone(),
            })
        })?;
        self.item(Component::Engine, |v| v.phase = "current".into());
        if let Err(e) = engine::cleanup(&self.store, &p.candidate.version, previous.as_deref()) {
            self.runtime
                .logs
                .push("updates", format!("旧核心清理失败：{e}"));
        }
        Ok(())
    }
    pub fn install(&self, handle: &tauri::AppHandle, p: &Prepared) -> Result<(), String> {
        let result = application::install(
            handle,
            &p.candidate,
            &self.cache(Component::App).join("package.bin"),
        );
        if let Err(e) = &result {
            self.error(Component::App, "install", e.clone());
        }
        result
    }
    pub fn reset_network(&self) {
        self.network.clear();
    }
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Release);
    }
    pub fn diagnose(
        &self,
        prefix: Option<&str>,
        cancel: Arc<AtomicBool>,
    ) -> Result<Vec<network::Diagnostic>, String> {
        let client = Transport::new(
            self.store.snapshot().update_preferences.network,
            self.network.clone(),
        )?
        .observed(
            Arc::new(|_| {}),
            Arc::new(move || cancel.load(Ordering::Acquire)),
        )
        .only_endpoint(prefix)?;
        let mut result = Vec::new();
        network::diagnostic(&mut result, "GitHub API", || {
            client
                .json::<serde_json::Value>(
                    &format!("https://api.github.com/repos/{}", release::REPOSITORY),
                    network::RequestKind::Api,
                    "application/vnd.github+json",
                )
                .map(|_| ())
        });
        network::diagnostic(&mut result, "发行文件", || {
            client.sample_package(&format!(
                "https://github.com/{}/releases/download/v{}/core-update.json",
                release::REPOSITORY,
                self.app_version
            ))
        });
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn schedule_uses_success_day_and_failure_retry_without_losing_last_success() {
        let mut state = Snapshot::default();
        state.record_check(1_000, 2, 0);
        assert_eq!(state.last_checked_at, 1_000);
        assert_eq!(state.next_check_at, 1_000 + 24 * 3600 * 1000);
        state.record_check(2_000, 0, 2);
        assert_eq!(state.last_checked_at, 1_000);
        assert_eq!(state.next_check_at, 2_000 + 15 * 60 * 1000);
        state.record_check(3_000, 1, 1);
        assert_eq!(state.last_checked_at, 3_000);
        assert_eq!(state.next_check_at, 3_000 + 15 * 60 * 1000);
    }
    #[test]
    fn restored_snapshot_does_not_trust_a_saved_ready_or_running_task() {
        let mut state = Snapshot {
            initialized: true,
            checking: true,
            next_check_at: u64::MAX,
            ..Default::default()
        };
        for (c, phase) in [
            (Component::App, "ready"),
            (Component::Engine, "downloading"),
        ] {
            state.items.insert(
                c,
                Item {
                    phase: phase.into(),
                    ..Default::default()
                },
            );
        }
        state.restore_transient(10);
        assert!(!state.initialized && !state.checking);
        assert_eq!(state.items[&Component::App].phase, "idle");
        assert_eq!(state.items[&Component::Engine].phase, "idle");
        assert_eq!(state.next_check_at, 10 + 24 * 3600 * 1000);
    }
}
