pub mod managed;
pub mod package;
use crate::{
    app::{Admission, Kind, Operation, Receipt},
    runtime::{self, RuntimeService, StartSpec},
    settings::{self, Blocked, CoreSource, FailureStage, ManagedRef, SettingsStore, Trial},
    updates::{release, Component},
};
use managed::{FetchError, ManagedCore, Origin, Phase, Prepared};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::Path,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
};

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Action {
    Start,
    Stop,
    Check,
}
#[derive(Clone, Copy, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum Maintenance {
    Check,
    Update,
    Retry,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub maintenance: managed::Snapshot,
    pub initialized: bool,
    pub usable: bool,
    pub notice: Option<String>,
    pub config_error: Option<String>,
    pub can_start: bool,
    pub can_stop: bool,
    pub can_import: bool,
    pub can_retry: bool,
}
pub struct CoreService {
    pub store: Arc<SettingsStore>,
    pub runtime: Arc<RuntimeService>,
    pub gate: Arc<Admission>,
    pub managed: Arc<ManagedCore>,
    wake: Arc<tokio::sync::Notify>,
    generation: AtomicU64,
    initialized: AtomicBool,
    usable: AtomicBool,
    notice: Mutex<Option<String>>,
    config_error: Mutex<Option<String>>,
    takeover: Mutex<Option<u64>>,
    fetch_origin: Mutex<Option<Origin>>,
}
impl CoreService {
    pub fn new(
        store: Arc<SettingsStore>,
        runtime: Arc<RuntimeService>,
        gate: Arc<Admission>,
        wake: Arc<tokio::sync::Notify>,
    ) -> Arc<Self> {
        Arc::new(Self {
            managed: Arc::new(ManagedCore::new(store.clone(), runtime.clone())),
            store,
            runtime,
            gate,
            wake,
            generation: AtomicU64::new(0),
            initialized: AtomicBool::new(false),
            usable: AtomicBool::new(false),
            notice: Mutex::new(None),
            config_error: Mutex::new(None),
            takeover: Mutex::new(None),
            fetch_origin: Mutex::new(None),
        })
    }
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }
    pub fn snapshot(&self) -> Snapshot {
        let settings = self.store.snapshot();
        let r = self.runtime.snapshot();
        self.snapshot_for(&settings, &r)
    }
    pub fn snapshot_for(
        &self,
        settings: &settings::Settings,
        r: &runtime::RuntimeSnapshot,
    ) -> Snapshot {
        let busy = self.gate.runtime_busy()
            || self.gate.closing()
            || !self.initialized.load(Ordering::Acquire);
        let usable = self.usable.load(Ordering::Acquire);
        Snapshot {
            maintenance: self.managed.snapshot(),
            initialized: self.initialized.load(Ordering::Acquire),
            usable,
            notice: self.notice.lock().unwrap().clone(),
            config_error: self.config_error.lock().unwrap().clone(),
            can_start: !busy
                && r.pid.is_none()
                && !r.cleanup_pending
                && self.store.error.is_none()
                && usable
                && settings.config_path.is_some(),
            can_stop: !self.gate.closing()
                && (r.pid.is_some()
                    || r.cleanup_pending
                    || r.status == "starting"
                    || self.gate.runtime_busy()),
            can_import: !busy
                && r.pid.is_none()
                && !r.cleanup_pending
                && self.store.error.is_none(),
            can_retry: !self.gate.closing() && !self.gate.core_fetching(),
        }
    }
    fn notify(&self, message: impl Into<String>) {
        let message = message.into();
        self.runtime.logs.push("core", message.clone());
        *self.notice.lock().unwrap() = Some(message);
    }
    pub fn initialize(&self) -> Result<(), String> {
        let _op = self.gate.begin(Kind::Selection)?;
        if self.initialized.load(Ordering::Acquire) {
            return Ok(());
        }
        if let Some(error) = &self.store.error {
            self.initialized.store(true, Ordering::Release);
            return Err(error.clone());
        }
        let cancel = _op.cancel.clone();
        let mut s = self.store.snapshot();
        if s.managed() && s.managed.trial == Some(Trial::Starting) {
            if let Some(previous) = s
                .managed
                .previous
                .clone()
                .filter(|r| self.compatible(r, &cancel).is_ok())
            {
                let failed = s.managed.active.clone();
                self.store.core(s.core_revision, |v| {
                    v.managed.active = Some(previous);
                    v.managed.previous = None;
                    v.managed.trial = None;
                    if let Some(c) = failed.and_then(|r| r.source) {
                        v.managed.blocked = Some(Blocked {
                            candidate: c,
                            stage: FailureStage::Startup,
                            error: "上次启动结果未确认".into(),
                        });
                    }
                    Ok(())
                })?;
                self.notify("上次核心启动未完成，已恢复上一版本；请手动启动");
                s = self.store.snapshot();
            } else {
                self.managed.suppressed.store(true, Ordering::Release);
                self.notify("上次核心启动未完成且没有可回退版本，请明确重试");
            }
        }
        if s.managed() {
            if let Some(mut active) = s.managed.active.clone() {
                let result = (|| {
                    self.verify(&active, &cancel)?;
                    if active.hash.is_none() {
                        active.hash = Some(package::hash(&settings::managed_path(
                            &self.store.root,
                            &active,
                        )?)?);
                        self.store.core(s.core_revision, |v| {
                            v.managed.active = Some(active);
                            Ok(())
                        })?;
                    }
                    Ok::<_, String>(())
                })();
                self.usable.store(result.is_ok(), Ordering::Release);
                if let Err(e) = result {
                    if s.managed.previous.is_some() && self.rollback(&e, &cancel).unwrap_or(false) {
                        self.notify("当前核心文件不可用，已恢复上一版本；请手动启动");
                    } else {
                        self.managed.error(e);
                    }
                }
            }
        } else {
            self.usable.store(
                s.selected_path(&self.store.root).is_ok_and(|p| p.is_file()),
                Ordering::Release,
            );
        }
        if let Err(e) = self
            .managed
            .initialize(self.generation.load(Ordering::Acquire), &cancel)
        {
            self.managed.error(e);
        }
        if self.store.snapshot().managed()
            && !self.usable.load(Ordering::Acquire)
            && self.managed.ready().is_none()
        {
            self.managed.update(|v| v.next_check_at = 0);
        }
        self.initialized.store(true, Ordering::Release);
        Ok(())
    }
    pub fn verify(&self, reference: &ManagedRef, cancel: &AtomicBool) -> Result<(), String> {
        package::verify(&self.store.root, &self.runtime, reference, cancel)
    }
    fn compatible(&self, reference: &ManagedRef, cancel: &AtomicBool) -> Result<(), String> {
        self.verify(reference, cancel)?;
        if let Some(config) = self.store.snapshot().config_path {
            self.runtime.validate(
                &settings::managed_path(&self.store.root, reference)?,
                &config,
                cancel,
            )?;
        }
        Ok(())
    }
    fn selected(&self, cancel: &AtomicBool) -> Result<std::path::PathBuf, String> {
        let s = self.store.snapshot();
        let p = s.selected_path(&self.store.root)?;
        if s.managed() {
            self.verify(s.managed.active.as_ref().unwrap(), cancel)?;
        }
        runtime::file(&p)
    }
    pub fn select(&self, path: &Path) -> Result<(), String> {
        let op = self.gate.begin(Kind::Selection)?;
        self.runtime.stopped()?;
        let s = self.store.snapshot();
        let path = runtime::file(path)?;
        self.runtime.probe(&path, &op.cancel)?;
        package::cancelled(&op.cancel)?;
        self.store.core(s.core_revision, |v| {
            v.core_source = CoreSource::External { path };
            v.managed.trial = None;
            Ok(())
        })?;
        self.generation.fetch_add(1, Ordering::AcqRel);
        *self.takeover.lock().unwrap() = None;
        self.gate.cancel_core();
        self.usable.store(true, Ordering::Release);
        self.runtime.clear_error();
        self.notify("已采用本机核心，由用户手动管理");
        Ok(())
    }
    pub fn import(&self, path: &Path) -> Result<(), String> {
        let op = self.gate.begin(Kind::Selection)?;
        self.runtime.stopped()?;
        let s = self.store.snapshot();
        let path = runtime::file(path)?;
        fs::File::open(&path).map_err(|e| e.to_string())?;
        match self.selected(&op.cancel) {
            Ok(core) => {
                self.runtime.validate(&core, &path, &op.cancel)?;
                self.store.core(s.core_revision, |v| {
                    v.config_path = Some(path);
                    v.pending_config_path = None;
                    if v.managed
                        .blocked
                        .as_ref()
                        .is_some_and(|b| b.stage == FailureStage::Compatibility)
                    {
                        v.managed.blocked = None;
                    }
                    Ok(())
                })?;
                *self.config_error.lock().unwrap() = None;
            }
            Err(_) if s.managed() => {
                self.store.core(s.core_revision, |v| {
                    v.pending_config_path = Some(path);
                    Ok(())
                })?;
                self.notify("配置路径已保存，等待核心校验");
            }
            Err(e) => return Err(e),
        }
        self.runtime.clear_error();
        drop(op);
        self.wake.notify_one();
        Ok(())
    }
    fn validate_pending(&self, cancel: &AtomicBool) -> Result<(), String> {
        let s = self.store.snapshot();
        let Some(config) = s.pending_config_path.clone() else {
            return Ok(());
        };
        let core = self.selected(cancel)?;
        let result = self.runtime.validate(&core, &config, cancel);
        if let Err(e) = &result {
            *self.config_error.lock().unwrap() = Some(e.clone());
            return result;
        }
        self.store.core(s.core_revision, |v| {
            v.config_path = Some(config);
            v.pending_config_path = None;
            Ok(())
        })?;
        *self.config_error.lock().unwrap() = None;
        Ok(())
    }
    pub fn check(&self) -> Result<(), String> {
        let op = self.gate.begin(Kind::Selection)?;
        self.check_local(&op.cancel)
    }
    fn check_local(&self, cancel: &AtomicBool) -> Result<(), String> {
        self.validate_pending(cancel)?;
        let s = self.store.snapshot();
        let core = self.selected(cancel)?;
        self.runtime.validate(
            &core,
            s.config_path.as_deref().ok_or("请先导入配置")?,
            cancel,
        )?;
        self.runtime.clear_error();
        self.runtime.logs.push("shell", "配置校验通过");
        *self.config_error.lock().unwrap() = None;
        Ok(())
    }
    pub fn action(self: &Arc<Self>, action: Action) -> Result<Receipt, String> {
        if matches!(action, Action::Stop) {
            self.gate.cancel_runtime();
        }
        let op = if matches!(action, Action::Stop) && self.gate.runtime_busy() {
            self.gate.begin(Kind::Diagnostic)?
        } else {
            self.gate.begin(Kind::Runtime)?
        };
        let receipt = op.receipt();
        let core = self.clone();
        tauri::async_runtime::spawn_blocking(move || {
            let result = match action {
                Action::Start => core.start_owned(&op),
                Action::Stop => core.stop_owned(&op),
                Action::Check => core.check_local(&op.cancel),
            };
            if let Err(e) = result {
                core.runtime.report(e);
            }
            drop(op);
            core.wake.notify_one();
        });
        Ok(receipt)
    }
    pub fn start(&self) -> Result<(), String> {
        let op = self.gate.begin(Kind::Runtime)?;
        self.start_owned(&op)
    }
    fn start_owned(&self, op: &Operation) -> Result<(), String> {
        if self.runtime.snapshot().pid.is_some() {
            return Err("sing-box 已在运行".into());
        }
        self.runtime.stopped()?;
        self.runtime.clear_error();
        if let Err(e) = self.adopt_owned(op) {
            self.managed.error(e);
        }
        self.validate_pending(&op.cancel)?;
        let s = self.store.snapshot();
        let config = s.config_path.clone().ok_or("请先导入有效配置")?;
        let spec = StartSpec {
            executable: s.selected_path(&self.store.root)?,
            config: config.clone(),
            version: if s.managed() {
                s.managed.active.as_ref().map(|r| r.version.clone())
            } else {
                None
            },
        };
        let trial = s.managed() && s.managed.trial.is_some();
        if trial {
            self.store.core(s.core_revision, |v| {
                v.managed.trial = Some(Trial::Starting);
                Ok(())
            })?;
        }
        let result = (|| {
            if let Err(e) = self.selected(&op.cancel) {
                self.usable.store(false, Ordering::Release);
                return Err(e);
            }
            self.runtime.start(&spec, op.cancel.clone())
        })();
        if result.is_ok() {
            self.usable.store(true, Ordering::Release);
            if trial {
                let s = self.store.snapshot();
                if let Err(e) = self.store.core(s.core_revision, |v| {
                    v.managed.trial = None;
                    Ok(())
                }) {
                    let stop = self.runtime.stop();
                    return Err(format!("启动确认保存失败：{e}；停止结果：{stop:?}"));
                }
            }
            return Ok(());
        }
        let error = result.unwrap_err();
        if !trial {
            if s.managed()
                && !self.usable.load(Ordering::Acquire)
                && !op.cancel.load(Ordering::Acquire)
                && self.rollback(&error, &op.cancel)?
            {
                self.notify(format!("当前核心文件不可用，已回退：{error}"));
                let previous = self.store.snapshot();
                return self
                    .runtime
                    .start(
                        &StartSpec {
                            executable: previous.selected_path(&self.store.root)?,
                            config,
                            version: previous.managed.active.map(|r| r.version),
                        },
                        op.cancel.clone(),
                    )
                    .map_err(|e| format!("当前版本失败：{error}；上一版本未启动：{e}"));
            }
            return Err(error);
        }
        self.runtime.stopped()?;
        if op.cancel.load(Ordering::Acquire) || self.gate.closing() {
            let s = self.store.snapshot();
            self.store.core(s.core_revision, |v| {
                v.managed.trial = Some(Trial::AwaitingStart);
                Ok(())
            })?;
            return Err(error);
        }
        if self.rollback(&error, &op.cancel)? {
            self.notify(format!("新版启动失败，已回退上一版本：{error}"));
            let previous = self.store.snapshot();
            let spec = StartSpec {
                executable: previous.selected_path(&self.store.root)?,
                config,
                version: previous.managed.active.map(|r| r.version),
            };
            return self
                .runtime
                .start(&spec, op.cancel.clone())
                .map_err(|e| format!("新版失败：{error}；上一版本也未启动：{e}"));
        }
        self.managed.suppressed.store(true, Ordering::Release);
        Err(error)
    }
    fn rollback(&self, error: &str, cancel: &AtomicBool) -> Result<bool, String> {
        let s = self.store.snapshot();
        let Some(previous) = s.managed.previous.clone() else {
            return Ok(false);
        };
        self.verify(&previous, cancel)?;
        if let Some(config) = &s.config_path {
            self.runtime.validate(
                &settings::managed_path(&self.store.root, &previous)?,
                config,
                cancel,
            )?;
        }
        package::cancelled(cancel)?;
        let failed = s.managed.active.as_ref().and_then(|r| r.source.clone());
        self.store.core(s.core_revision, |v| {
            v.managed.active = Some(previous);
            v.managed.previous = None;
            v.managed.trial = None;
            if let Some(candidate) = failed {
                v.managed.blocked = Some(Blocked {
                    candidate,
                    stage: FailureStage::Startup,
                    error: error.into(),
                });
            }
            Ok(())
        })?;
        self.managed.clear_ready();
        self.usable.store(true, Ordering::Release);
        Ok(true)
    }
    pub fn stop(&self) -> Result<(), String> {
        let op = self.gate.begin(Kind::Runtime)?;
        self.stop_owned(&op)
    }
    fn stop_owned(&self, op: &Operation) -> Result<(), String> {
        self.gate.cancel_runtime();
        // When cancelling an in-flight start, its owner must release the runtime ticket first.
        if self.gate.runtime_other(op.id) {
            self.gate.wait_runtime(op.id)?;
        }
        self.runtime.stop()?;
        Ok(())
    }
    pub fn cancel(&self) {
        self.managed.suppressed.store(true, Ordering::Release);
        self.gate.cancel_core();
    }
    pub fn preferences_changed(&self) {
        self.managed.network.clear();
        let s = self.store.snapshot();
        if !s.update_preferences.automatic_core
            && *self.fetch_origin.lock().unwrap() == Some(Origin::Automatic)
        {
            self.gate.cancel_core();
        }
        self.wake.notify_one();
    }
    pub fn use_managed(self: &Arc<Self>) -> Result<Receipt, String> {
        let op = self.gate.begin(Kind::Selection)?;
        self.runtime.stopped()?;
        let gen = self.generation.fetch_add(1, Ordering::AcqRel) + 1;
        self.gate.cancel_core();
        *self.takeover.lock().unwrap() = Some(gen);
        if let Some(mut p) = self.managed.ready() {
            p.generation = gen;
            p.origin = Origin::Manual;
            settings::write_json(&self.managed.cache().join("prepared.json"), &p)?;
            *self.managed.prepared.lock().unwrap() = Some(p);
        }
        self.managed.suppressed.store(false, Ordering::Release);
        let receipt = op.receipt();
        let s = self.store.snapshot();
        if let Some(reference) = &s.managed.active {
            if self.verify(reference, &op.cancel).is_ok() {
                if let Some(config) = &s.config_path {
                    self.runtime.validate(
                        &settings::managed_path(&self.store.root, reference)?,
                        config,
                        &op.cancel,
                    )?;
                }
                self.store.core(s.core_revision, |v| {
                    v.core_source = CoreSource::Managed;
                    v.managed.trial = None;
                    Ok(())
                })?;
                *self.takeover.lock().unwrap() = None;
                self.usable.store(true, Ordering::Release);
            }
        }
        drop(op);
        self.wake.notify_one();
        Ok(receipt)
    }
    pub fn maintain(self: &Arc<Self>, manual: Option<Maintenance>) -> Result<Receipt, String> {
        if !self.initialized.load(Ordering::Acquire) {
            return Err("核心服务正在初始化".into());
        }
        if self.store.error.is_some() {
            return Err(self.store.error.clone().unwrap());
        }
        let s = self.store.snapshot();
        let takeover = *self.takeover.lock().unwrap();
        if !s.managed() && takeover.is_none() {
            return Err("外部核心由用户手动管理".into());
        }
        let gen = self.generation.load(Ordering::Acquire);
        if manual.is_some() {
            self.managed.suppressed.store(false, Ordering::Release);
        }
        if self.managed.suppressed.load(Ordering::Acquire) {
            return Err("自动维护已暂停，请明确重试".into());
        }
        let required = takeover.is_some()
            || s.managed.active.is_none()
            || !self.usable.load(Ordering::Acquire);
        if manual.is_none()
            && !required
            && (!s.update_preferences.automatic_core || s.managed.trial.is_some())
        {
            return Err("无需自动维护".into());
        }
        if manual.is_none() && self.managed.snapshot().next_check_at > crate::logs::now() {
            return Err("核心维护未到期".into());
        }
        let op = self.gate.begin(Kind::CoreFetch)?;
        let receipt = op.receipt();
        if let Some(mut prepared) = self
            .managed
            .ready()
            .filter(|_| manual.is_some() || s.managed.blocked.is_none())
        {
            if manual.is_some() {
                prepared.origin = Origin::Manual;
                prepared.generation = gen;
                settings::write_json(&self.managed.cache().join("prepared.json"), &prepared)?;
                *self.managed.prepared.lock().unwrap() = Some(prepared);
            }
            drop(op);
            let service = self.clone();
            tauri::async_runtime::spawn_blocking(move || {
                if let Err(e) = service.try_adopt() {
                    service.managed.error(e);
                }
            });
            return Ok(receipt);
        }
        *self.fetch_origin.lock().unwrap() = Some(if manual.is_some() || takeover.is_some() {
            Origin::Manual
        } else if required {
            Origin::Required
        } else {
            Origin::Automatic
        });
        let service = self.clone();
        tauri::async_runtime::spawn_blocking(move || {
            service.fetch(&op, manual, required, gen);
            *service.fetch_origin.lock().unwrap() = None;
            drop(op);
            if let Err(e) = service.try_adopt() {
                service.managed.error(e);
            }
            service.wake.notify_one();
        });
        Ok(receipt)
    }
    fn fetch(&self, op: &Operation, manual: Option<Maintenance>, required: bool, generation: u64) {
        self.managed.update(|v| {
            v.phase = Phase::Checking;
            v.error = None;
        });
        let mut chosen = None;
        let result = (|| {
            let s = self.store.snapshot();
            if required {
                if let Some(c) = s.managed.active.as_ref().and_then(|r| r.source.clone()) {
                    let archive = self.managed.cache().join("package.bin");
                    if package::hash_cancel(&archive, &op.cancel).is_ok_and(|h| h == c.sha256)
                        && fs::metadata(&archive).is_ok_and(|m| m.len() == c.size)
                    {
                        chosen = Some(c.clone());
                        self.managed.stage_archive(
                            &c,
                            &archive,
                            Origin::Required,
                            generation,
                            &op.cancel,
                        )?;
                        return Ok(());
                    }
                }
            }
            let t = self.managed.transport(op).map_err(FetchError::Network)?;
            let candidate = if required && s.managed.active.is_some() {
                let r = s.managed.active.as_ref().unwrap();
                let c = release::engine_version(&t, &r.version).map_err(FetchError::Network)?;
                if r.source.as_ref().is_some_and(|old| !same_asset(old, &c)) {
                    return Err(FetchError::Package(
                        "已选版本的官方资产发生变化，请明确处理".into(),
                    ));
                }
                Some(c)
            } else {
                release::discover(
                    &t,
                    Component::Engine,
                    s.managed
                        .active
                        .as_ref()
                        .map(|r| r.version.as_str())
                        .unwrap_or("0.0.0"),
                )
                .map_err(FetchError::Network)?
            };
            let Some(c) = candidate else {
                self.managed.update(|v| {
                    v.candidate = None;
                    v.phase = Phase::Idle;
                });
                return Ok(());
            };
            chosen = Some(c.clone());
            if manual == Some(Maintenance::Check) {
                self.managed.update(|v| {
                    v.candidate = Some(c);
                    v.phase = Phase::Idle;
                });
                return Ok(());
            }
            if manual.is_none()
                && s.managed
                    .blocked
                    .as_ref()
                    .is_some_and(|b| b.candidate.version == c.version)
            {
                return Err(FetchError::Package(s.managed.blocked.unwrap().error));
            }
            release::recheck(&t, &c).map_err(FetchError::Network)?;
            let origin = if manual.is_some() || self.takeover.lock().unwrap().is_some() {
                Origin::Manual
            } else if required {
                Origin::Required
            } else {
                Origin::Automatic
            };
            self.managed
                .prepare_with(&t, c, origin, generation, &op.cancel)?;
            Ok(())
        })();
        self.managed.checked(
            crate::logs::now(),
            matches!(result, Err(FetchError::Network(_))),
        );
        match result {
            Ok(()) => {}
            Err(FetchError::Cancelled) => self.managed.update(|s| s.phase = Phase::Paused),
            Err(FetchError::Network(e)) => self.managed.error(e),
            Err(FetchError::Local(e)) => {
                self.managed.suppressed.store(true, Ordering::Release);
                self.managed.error(e);
            }
            Err(FetchError::Package(e)) => {
                if let Some(candidate) = chosen {
                    let s = self.store.snapshot();
                    if let Ok(_change) = self.gate.begin(Kind::Selection) {
                        if generation == self.generation.load(Ordering::Acquire) {
                            let _ = self.store.core(s.core_revision, |s| {
                                s.managed.blocked = Some(Blocked {
                                    candidate,
                                    stage: FailureStage::Package,
                                    error: e.clone(),
                                });
                                Ok(())
                            });
                        }
                    }
                }
                self.managed.error(e);
            }
        }
        self.managed.persist();
    }
    pub fn tick(self: &Arc<Self>) {
        if !self.initialized.load(Ordering::Acquire) || self.gate.closing() {
            return;
        }
        if self.managed.ready().is_some() {
            if self.store.snapshot().managed.blocked.is_some()
                && self.managed.snapshot().next_check_at <= crate::logs::now()
            {
                let _ = self.maintain(None);
                return;
            }
            let core = self.clone();
            tauri::async_runtime::spawn_blocking(move || {
                if let Err(e) = core.try_adopt() {
                    core.managed.error(e);
                }
            });
            return;
        }
        let _ = self.maintain(None);
    }
    pub fn try_adopt(&self) -> Result<(), String> {
        if self.gate.core_fetching() || self.gate.runtime_busy() || self.gate.closing() {
            return Ok(());
        }
        let op = self.gate.begin(Kind::Selection)?;
        self.adopt_owned(&op)
    }
    fn adopt_owned(&self, op: &Operation) -> Result<(), String> {
        let Ok(_files) = self.gate.begin(Kind::CoreFetch) else {
            return Ok(());
        };
        let Some(p) = self.managed.ready() else {
            return Ok(());
        };
        let s = self.store.snapshot();
        if p.generation != self.generation.load(Ordering::Acquire) || self.gate.closing() {
            return Ok(());
        }
        let takeover = *self.takeover.lock().unwrap() == Some(p.generation);
        if !s.managed() && !takeover {
            return Ok(());
        }
        if self.managed.suppressed.load(Ordering::Acquire) {
            return Ok(());
        }
        if p.origin == Origin::Automatic && !s.update_preferences.automatic_core {
            self.managed.update(|v| v.phase = Phase::Paused);
            return Ok(());
        }
        if s.managed
            .blocked
            .as_ref()
            .is_some_and(|b| same_asset(&b.candidate, p.reference.source.as_ref().unwrap()))
            && p.origin != Origin::Manual
        {
            return Ok(());
        }
        if self.runtime.stopped().is_err() {
            return Ok(());
        }
        if let Some(active) = &s.managed.active {
            if active.version == p.reference.version && self.verify(active, &op.cancel).is_ok() {
                self.managed.clear_ready();
                return Ok(());
            }
            if s.managed.trial.is_some() && self.usable.load(Ordering::Acquire) {
                return Ok(());
            }
            if settings::stable(&p.reference.version)? < settings::stable(&active.version)? {
                self.managed.clear_ready();
                return Ok(());
            }
        }
        let installed = settings::managed_path(&self.store.root, &p.reference)?;
        let staged = package::staged_path(&self.store.root, &p.reference)?;
        let program = if staged.exists() { &staged } else { &installed };
        package::owned(&self.store.root, program)?;
        package::verify_path(&self.runtime, &p.reference, program, &op.cancel)?;
        let config = s.config_path.clone();
        let mut config_hash = None;
        if s.managed.active.is_some() || !s.managed() {
            if let Some(config) = &config {
                let hash = package::hash_cancel(config, &op.cancel)?;
                if let Err(error) = self.runtime.validate(program, config, &op.cancel) {
                    self.block(&p, FailureStage::Compatibility, error.clone())?;
                    return Err(format!("新版暂不兼容当前配置，保留原核心：{error}"));
                }
                if package::hash_cancel(config, &op.cancel)? != hash {
                    return Err("配置文件已改变，请重新校验".into());
                }
                config_hash = Some(hash);
            }
        }
        package::cancelled(&op.cancel)?;
        let previous = s
            .managed
            .active
            .clone()
            .filter(|r| self.verify(r, &op.cancel).is_ok());
        // Only a stopped, invalid same-version file may be replaced by authenticated staging.
        if staged.exists() {
            if installed.exists() {
                let old = s.managed.active.as_ref().ok_or("已有版本目录内容不一致")?;
                if old.version != p.reference.version || self.verify(old, &op.cancel).is_ok() {
                    if package::hash(&installed)? != p.reference.hash.clone().unwrap() {
                        return Err("已有版本目录内容不一致".into());
                    }
                    fs::remove_file(&staged).map_err(|e| e.to_string())?;
                } else {
                    fs::rename(&staged, &installed).map_err(|e| e.to_string())?;
                }
            } else {
                fs::rename(&staged, &installed).map_err(|e| e.to_string())?;
            }
        }
        if let (Some(config), Some(hash)) = (&config, config_hash) {
            if package::hash_cancel(config, &op.cancel)? != hash {
                return Err("配置文件已改变，请重新校验".into());
            }
        }
        self.store.core(s.core_revision, |v| {
            if self.gate.closing()
                || op.cancel.load(Ordering::Acquire)
                || p.generation != self.generation.load(Ordering::Acquire)
            {
                return Err("采用已取消".into());
            }
            if !v.managed() && !takeover {
                return Err("核心来源已改变".into());
            }
            if p.origin == Origin::Automatic && !v.update_preferences.automatic_core {
                return Err("自动采用已关闭".into());
            }
            v.core_source = CoreSource::Managed;
            v.managed.previous = previous;
            v.managed.active = Some(p.reference.clone());
            v.managed.trial = Some(Trial::AwaitingStart);
            v.managed.blocked = None;
            Ok(())
        })?;
        *self.takeover.lock().unwrap() = None;
        self.usable.store(true, Ordering::Release);
        self.managed.clear_ready();
        self.notify(format!("sing-box {} 已就绪", p.reference.version));
        if let Err(e) = self.validate_pending(&op.cancel) {
            *self.config_error.lock().unwrap() = Some(e);
        }
        self.cleanup(&op.cancel);
        Ok(())
    }
    fn block(&self, p: &Prepared, stage: FailureStage, error: String) -> Result<(), String> {
        let s = self.store.snapshot();
        self.store.core(s.core_revision, |v| {
            v.managed.blocked = Some(Blocked {
                candidate: p.reference.source.clone().ok_or("缺少候选来源")?,
                stage,
                error,
            });
            Ok(())
        })?;
        Ok(())
    }
    fn cleanup(&self, cancel: &AtomicBool) {
        let s = self.store.snapshot();
        let mut keep: Vec<_> = [s.managed.active, s.managed.previous]
            .into_iter()
            .flatten()
            .map(|r| r.version)
            .collect();
        if let Some(p) = self.managed.ready() {
            keep.push(p.reference.version);
        }
        if let Err(e) = package::cleanup(&self.store.root, &keep, cancel) {
            self.runtime
                .logs
                .push("core-update", format!("旧核心清理失败：{e}"));
        }
    }
}
pub fn same_asset(a: &release::Candidate, b: &release::Candidate) -> bool {
    a.version == b.version
        && a.target == b.target
        && a.asset_id == b.asset_id
        && a.url == b.url
        && a.sha256 == b.sha256
        && a.size == b.size
}
