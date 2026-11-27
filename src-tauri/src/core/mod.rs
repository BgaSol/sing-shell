mod adoption;
pub use adoption::Adoption;
pub mod managed;
pub mod material;
pub mod package;
use crate::{
    app::{Admission, Kind, Operation, Receipt},
    runtime::{self, RuntimeService},
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
    pub(crate) usable: AtomicBool,
    notice: Mutex<Option<String>>,
    config_error: Mutex<Option<String>>,
    takeover: Mutex<Option<u64>>,
    fetch_origin: Mutex<Option<Origin>>,
    pub(crate) restore_safe: AtomicBool,
    #[cfg(feature = "test-fixtures")]
    pub adoption_hook: Mutex<Option<Arc<dyn Fn(&str) + Send + Sync>>>,
}
impl CoreService {
    fn config_for(&self, s: &settings::Settings) -> Result<Option<std::path::PathBuf>, String> {
        if s.profile.mode == crate::profile::Mode::Generated {
            if s.profile.selected().is_none() {
                Ok(None)
            } else {
                self.runtime
                    .logs
                    .redact(s.profile.selected().unwrap().spec.secrets());
                // First-install checks may precede adopting the verified candidate.
                let version = s
                    .managed
                    .active
                    .as_ref()
                    .map(|r| r.version.as_str())
                    .unwrap_or("1.14.4");
                crate::profile::config::build(&s.profile, &self.store.root, version)
                    .map(|c| Some(c.path))
            }
        } else {
            Ok(s.config_path.clone())
        }
    }
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
            restore_safe: AtomicBool::new(true),
            #[cfg(feature = "test-fixtures")]
            adoption_hook: Mutex::new(None),
        })
    }
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }
    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            maintenance: self.managed.snapshot(),
            initialized: self.initialized.load(Ordering::Acquire),
            usable: self.usable.load(Ordering::Acquire),
            notice: self.notice.lock().unwrap().clone(),
            config_error: self.config_error.lock().unwrap().clone(),
            can_retry: self.initialized.load(Ordering::Acquire)
                && self.store.error().is_none()
                && !self.gate.closing()
                && !self.gate.core_fetching()
                && !self.gate.runtime_busy(),
        }
    }
    pub(crate) fn notify(&self, message: impl Into<String>) {
        let message = message.into();
        self.runtime.logs.push("core", message.clone());
        *self.notice.lock().unwrap() = Some(message);
    }
    pub fn initialize(&self) -> Result<(), String> {
        let _op = self.gate.begin(Kind::Selection)?;
        if self.initialized.load(Ordering::Acquire) {
            return Ok(());
        }
        if let Some(error) = &self.store.error() {
            self.initialized.store(true, Ordering::Release);
            return Err(error.clone());
        }
        let result = self.recover_local(&_op.cancel);
        self.initialized.store(true, Ordering::Release);
        result
    }
    fn recover_local(&self, cancel: &AtomicBool) -> Result<(), String> {
        let mut s = self.store.snapshot();
        if let Err(e) = self
            .managed
            .initialize(self.generation.load(Ordering::Acquire), &cancel)
        {
            self.managed.fail(managed::FailureKind::Cache, e);
        }

        if s.managed() && s.managed.trial == Some(Trial::Starting) {
            self.restore_safe.store(false, Ordering::Release);
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
                        self.restore_safe.store(false, Ordering::Release);
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
        if self.store.snapshot().managed()
            && !self.usable.load(Ordering::Acquire)
            && self.managed.ready().is_none()
        {
            self.managed.update(|v| v.next_check_at = 0);
        }
        Ok(())
    }
    pub(crate) fn reset_owned(&self, op: &Operation) {
        self.generation.fetch_add(1, Ordering::AcqRel);
        *self.takeover.lock().unwrap() = None;
        *self.fetch_origin.lock().unwrap() = None;
        *self.notice.lock().unwrap() = None;
        *self.config_error.lock().unwrap() = None;
        self.usable.store(false, Ordering::Release);
        self.restore_safe.store(false, Ordering::Release);
        self.managed.suppressed.store(false, Ordering::Release);
        if let Err(error) = self.recover_local(&op.cancel) {
            self.managed.error(error);
        }
        self.initialized.store(true, Ordering::Release);
    }
    pub fn verify(&self, reference: &ManagedRef, cancel: &AtomicBool) -> Result<(), String> {
        package::verify(&self.store.root, &self.runtime, reference, cancel)?;
        if self.store.snapshot().profile.mode == crate::profile::Mode::Generated {
            let envelope = reference
                .source
                .as_ref()
                .map(material::for_candidate)
                .transpose()?
                .or_else(|| material::bundled(&reference.version))
                .ok_or("当前版本缺少离线安装材料，请重试准备核心")?;
            material::program(
                &envelope,
                &reference.version,
                &settings::managed_path(&self.store.root, reference)?,
                cancel,
            )?;
        }
        Ok(())
    }
    fn compatible(&self, reference: &ManagedRef, cancel: &AtomicBool) -> Result<(), String> {
        self.verify(reference, cancel)?;
        if let Some(config) = self.config_for(&self.store.snapshot())? {
            self.runtime.validate(
                &settings::managed_path(&self.store.root, reference)?,
                &config,
                cancel,
            )?;
        }
        Ok(())
    }
    pub(crate) fn selected(&self, cancel: &AtomicBool) -> Result<std::path::PathBuf, String> {
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
                    v.profile.mode = crate::profile::Mode::ExternalConfig;
                    v.nodes_revision += 1;
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
                    v.profile.mode = crate::profile::Mode::ExternalConfig;
                    v.nodes_revision += 1;
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
    pub(crate) fn validate_pending(&self, cancel: &AtomicBool) -> Result<(), String> {
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
    pub(crate) fn check_local(&self, cancel: &AtomicBool) -> Result<(), String> {
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
    pub(crate) fn rollback(&self, error: &str, cancel: &AtomicBool) -> Result<bool, String> {
        let s = self.store.snapshot();
        let Some(previous) = s.managed.previous.clone() else {
            return Ok(false);
        };
        self.verify(&previous, cancel)?;
        if let Some(config) = self.config_for(&s)?.as_ref() {
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
        self.managed.consume_ready();
        self.usable.store(true, Ordering::Release);
        Ok(true)
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
        let files = self.gate.begin(Kind::CoreFetch)?;
        self.runtime.stopped()?;
        let gen = self.generation.fetch_add(1, Ordering::AcqRel) + 1;
        *self.takeover.lock().unwrap() = Some(gen);
        if let Some(mut p) = self.managed.ready() {
            p.generation = gen;
            p.origin = Origin::Manual;
            self.managed.save_prepared(p)?;
        }
        self.managed.suppressed.store(false, Ordering::Release);
        let receipt = op.receipt();
        let s = self.store.snapshot();
        if let Some(reference) = &s.managed.active {
            if self.verify(reference, &op.cancel).is_ok() {
                if let Some(config) = self.config_for(&s)?.as_ref() {
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
        drop(files);
        drop(op);
        self.wake.notify_one();
        Ok(receipt)
    }
    pub fn maintain(self: &Arc<Self>, manual: Option<Maintenance>) -> Result<Receipt, String> {
        if !self.initialized.load(Ordering::Acquire) {
            return Err("核心服务正在初始化".into());
        }
        if let Some(error) = self.store.error() {
            return Err(error);
        }
        let s = self.store.snapshot();
        let takeover = *self.takeover.lock().unwrap();
        if !s.managed() && takeover.is_none() {
            return Err("外部核心由用户手动管理".into());
        }
        let checking = manual == Some(Maintenance::Check);
        if self.managed.suppressed.load(Ordering::Acquire) && manual.is_none() {
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
        if manual.is_some() && !checking {
            self.managed.suppressed.store(false, Ordering::Release);
        }
        let receipt = op.receipt();
        let generation = self.generation();
        let service = self.clone();
        tauri::async_runtime::spawn_blocking(move || {
            if checking {
                service.check_updates(&op, required);
                return;
            }
            *service.fetch_origin.lock().unwrap() =
                Some(if manual.is_some() || takeover.is_some() {
                    Origin::Manual
                } else if required {
                    Origin::Required
                } else {
                    Origin::Automatic
                });
            let result = (|| -> Result<(), String> {
                if manual.is_some() {
                    service.managed.update(|s| s.phase = Phase::Preparing);
                    if service.managed.ready().is_none()
                        && service.managed.cache().join("prepared.json").exists()
                    {
                        if let Err(e) = service.managed.initialize(generation, &op.cancel) {
                            package::cancelled(&op.cancel)?;
                            service.managed.clear_ready()?;
                            service.managed.warn(format!("无效核心缓存将重新准备：{e}"));
                        }
                    }
                    if let Some(mut p) = service.managed.ready() {
                        service
                            .managed
                            .update(|s| s.candidate = p.reference.source.clone());
                        if let Err(e) = service.managed.restore_program(&mut p, &op.cancel) {
                            package::cancelled(&op.cancel)?;
                            service.managed.clear_ready()?;
                            service
                                .managed
                                .warn(format!("核心准备材料不可用，将重新获取：{e}"));
                        }
                        package::cancelled(&op.cancel)?;
                        if generation != service.generation() {
                            return Err("核心来源已改变".into());
                        }
                        if service.managed.ready().is_some() {
                            service.managed.save_prepared(p)?;
                        }
                    }
                    service.managed.resume(generation)?;
                }
                package::cancelled(&op.cancel)?;
                if generation != service.generation() {
                    return Err("核心来源已改变".into());
                }
                if service.managed.ready().is_none() {
                    service.fetch(&op, manual, required, generation);
                }
                Ok(())
            })();
            if let Err(error) = result {
                if op.cancel.load(Ordering::Acquire) {
                    service.managed.update(|s| s.phase = Phase::Paused);
                } else {
                    service.managed.suppressed.store(true, Ordering::Release);
                    service.managed.fail(managed::FailureKind::Local, error);
                }
            }
            *service.fetch_origin.lock().unwrap() = None;
            drop(op);
            if let Err(e) = service.try_adopt() {
                service.managed.error(e);
            }
            service.wake.notify_one();
        });
        Ok(receipt)
    }
    fn discover_candidate(
        &self,
        t: &crate::updates::network::Transport,
        required: bool,
    ) -> Result<Option<release::Candidate>, FetchError> {
        let s = self.store.snapshot();
        if required {
            if let Some(r) = s.managed.active.as_ref() {
                let c = match r.source.as_ref() {
                    Some(source) if !source.signature.is_empty() => {
                        material::for_candidate(source).and_then(|e| e.candidate(&r.version))
                    }
                    _ => release::engine_version(t, &r.version),
                }
                .map_err(FetchError::Network)?;
                if r.source.as_ref().is_some_and(|old| !same_asset(old, &c)) {
                    return Err(FetchError::Conflict(
                        "已选版本的官方资产发生变化，请明确处理".into(),
                    ));
                }
                return Ok(Some(c));
            }
            if s.profile.mode == crate::profile::Mode::Generated {
                return release::engine_version(t, material::BASELINE)
                    .map(Some)
                    .map_err(FetchError::Network);
            }
        }
        release::discover(
            t,
            Component::Engine,
            s.managed
                .active
                .as_ref()
                .map(|r| r.version.as_str())
                .unwrap_or("0.0.0"),
        )
        .map_err(FetchError::Network)
    }
    fn check_updates(self: &Arc<Self>, op: &Operation, required: bool) {
        let before = self.managed.snapshot();
        self.managed.update(|s| s.phase = Phase::Checking);
        let result = self
            .managed
            .transport(op)
            .map_err(FetchError::Network)
            .and_then(|t| self.discover_candidate(&t, required));
        self.managed.checked(crate::logs::now(), result.is_err());
        self.managed.update(|s| {
            s.phase = if before.error.is_some() {
                before.phase
            } else if self.managed.ready().is_some() {
                Phase::Ready
            } else {
                Phase::Idle
            }
        });
        match result {
            Ok(candidate) => self.managed.update(|s| s.candidate = candidate),
            Err(_) if op.cancel.load(Ordering::Acquire) => {
                self.managed.update(|s| s.phase = Phase::Paused)
            }
            Err(e) if before.error.is_some() => {
                self.managed.warn(format!("检查核心失败：{}", e.message()))
            }
            Err(e) => {
                let kind = if matches!(e, FetchError::Conflict(_)) {
                    managed::FailureKind::Conflict
                } else {
                    managed::FailureKind::Network
                };
                self.managed.fail(kind, e.message());
            }
        }
        self.managed.persist();
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
            let candidate = if manual == Some(Maintenance::Retry) && s.managed.active.is_none() {
                if let Some(cached) = self.managed.snapshot().candidate {
                    release::validate_engine(&cached).map_err(FetchError::Package)?;
                    let fresh = material::for_candidate(&cached)
                        .and_then(|e| e.candidate(&cached.version))
                        .or_else(|_| release::engine_version(&t, &cached.version))
                        .map_err(FetchError::Network)?;
                    if !same_asset(&cached, &fresh) {
                        return Err(FetchError::Conflict(
                            "准备版本的官方资产发生变化，请明确处理".into(),
                        ));
                    }
                    Some(fresh)
                } else {
                    self.discover_candidate(&t, required)?
                }
            } else {
                self.discover_candidate(&t, required)?
            };
            let Some(c) = candidate else {
                self.managed.update(|v| {
                    v.candidate = None;
                    v.phase = Phase::Idle;
                });
                return Ok(());
            };
            chosen = Some(c.clone());
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
            let archive = self.managed.cache().join("package.bin");
            if package::hash_cancel(&archive, &op.cancel).is_ok_and(|h| h == c.sha256)
                && fs::metadata(&archive).is_ok_and(|m| m.len() == c.size)
            {
                self.managed
                    .stage_archive(&c, &archive, origin, generation, &op.cancel)?;
            } else {
                self.managed
                    .prepare_with(&t, c, origin, generation, &op.cancel)?;
            }
            Ok(())
        })();
        self.managed.checked(
            crate::logs::now(),
            matches!(result, Err(FetchError::Network(_))),
        );
        match result {
            Ok(()) => {}
            Err(FetchError::Cancelled) => self.managed.update(|s| s.phase = Phase::Paused),
            Err(FetchError::Network(e)) => self.managed.fail(managed::FailureKind::Network, e),
            Err(FetchError::Local(e)) => {
                self.managed.suppressed.store(true, Ordering::Release);
                self.managed.fail(managed::FailureKind::Local, e);
            }
            Err(FetchError::Conflict(e)) => {
                self.managed.suppressed.store(true, Ordering::Release);
                self.managed.fail(managed::FailureKind::Conflict, e);
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
                self.managed.fail(managed::FailureKind::Package, e);
            }
        }
        self.managed.persist();
    }
    pub fn tick(self: &Arc<Self>) {
        if !self.initialized.load(Ordering::Acquire) || self.gate.closing() {
            return;
        }
        if self.managed.suppressed.load(Ordering::Acquire) {
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
