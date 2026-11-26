use crate::{
    app::{Admission, Kind, Operation, Receipt},
    core::managed::Phase,
    core::{self, package, CoreService},
    runtime::{self, RuntimeService, StartSpec},
    settings::{self, SettingsStore, Trial},
};
use serde::{Deserialize, Serialize};
use std::{
    path::Path,
    sync::{atomic::Ordering, Arc, Mutex},
};

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Action {
    Start,
    Stop,
    Check,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Restore {
    #[default]
    None,
    Waiting,
    Starting,
    Finished,
}
#[derive(Default)]
struct Intent {
    epoch: u64,
    restore: Restore,
    restore_operation: Option<u64>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    #[serde(flatten)]
    pub core: core::Snapshot,
    pub restore: Restore,
    pub can_start: bool,
    pub can_stop: bool,
    pub can_import: bool,
}
pub struct SessionService {
    pub engine: Arc<CoreService>,
    pub store: Arc<SettingsStore>,
    pub runtime: Arc<RuntimeService>,
    pub gate: Arc<Admission>,
    wake: Arc<tokio::sync::Notify>,
    intent: Mutex<Intent>,
}
impl SessionService {
    pub fn new(engine: Arc<CoreService>, wake: Arc<tokio::sync::Notify>) -> Arc<Self> {
        Arc::new(Self {
            store: engine.store.clone(),
            runtime: engine.runtime.clone(),
            gate: engine.gate.clone(),
            engine,
            wake,
            intent: Mutex::new(Intent::default()),
        })
    }
    pub fn initialize(&self) -> Result<(), String> {
        self.engine.initialize()?;
        self.arm_restore();
        Ok(())
    }
    pub fn snapshot(&self) -> Snapshot {
        self.snapshot_for(&self.store.snapshot(), &self.runtime.snapshot())
    }
    pub fn snapshot_for(
        &self,
        settings: &settings::Settings,
        r: &runtime::RuntimeSnapshot,
    ) -> Snapshot {
        let core = self.engine.snapshot();
        let busy = self.gate.runtime_busy() || self.gate.closing() || !core.initialized;
        let restore = self.intent.lock().unwrap().restore;
        Snapshot {
            can_start: !busy
                && r.pid.is_none()
                && !r.cleanup_pending
                && self.store.error.is_none()
                && core.usable
                && settings.config_path.is_some(),
            can_stop: !self.gate.closing()
                && (r.pid.is_some()
                    || r.cleanup_pending
                    || r.status == "starting"
                    || self.gate.runtime_busy()
                    || matches!(restore, Restore::Waiting | Restore::Starting)),
            can_import: !busy
                && r.pid.is_none()
                && !r.cleanup_pending
                && self.store.error.is_none(),
            core,
            restore,
        }
    }
    pub fn select_core(&self, path: &Path) -> Result<(), String> {
        self.invalidate_restore();
        self.engine.select(path)
    }
    pub fn import_config(&self, path: &Path) -> Result<(), String> {
        self.invalidate_restore();
        self.engine.import(path)
    }
    pub fn use_managed(self: &Arc<Self>) -> Result<Receipt, String> {
        self.invalidate_restore();
        self.engine.use_managed()
    }
    pub fn cancel_core(&self) {
        self.engine.cancel();
        let mut i = self.intent.lock().unwrap();
        if i.restore == Restore::Waiting && !self.engine.usable.load(Ordering::Acquire) {
            i.restore = Restore::Finished;
            self.runtime
                .report("自动恢复已取消：必需核心准备被取消".into());
        }
    }
    pub fn stop_internal(&self) -> Result<(), String> {
        self.invalidate_restore();
        self.runtime.stop()
    }
    pub fn action(self: &Arc<Self>, action: Action) -> Result<Receipt, String> {
        let op = self.gate.begin(if matches!(action, Action::Stop) {
            Kind::Stop
        } else {
            Kind::Runtime
        })?;
        if matches!(action, Action::Start | Action::Stop) {
            self.invalidate_restore();
        }
        let receipt = op.receipt();
        let core = self.clone();
        tauri::async_runtime::spawn_blocking(move || {
            let result = match action {
                Action::Start => core.start_owned(&op),
                Action::Stop => core.stop_owned(&op),
                Action::Check => core.engine.check_local(&op.cancel),
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
        self.invalidate_restore();
        self.start_owned(&op)
    }
    fn start_owned(&self, op: &Operation) -> Result<(), String> {
        let epoch = self.intent.lock().unwrap().epoch;
        self.start_process(op)?;
        let intent = self.intent.lock().unwrap();
        let result: Result<(), String> = (|| {
            if intent.epoch != epoch || self.gate.closing() {
                return Err("启动已取消".into());
            }
            package::cancelled(&op.cancel)?;
            self.store
                .confirm_start(self.store.snapshot().core_revision)?;
            Ok(())
        })();
        drop(intent);
        if let Err(e) = result {
            let stop = self.runtime.stop();
            if op.cancel.load(Ordering::Acquire) && stop.is_ok() {
                let s = self.store.snapshot();
                if s.managed.trial == Some(Trial::Starting) {
                    let _ = self.store.core(s.core_revision, |v| {
                        v.managed.trial = Some(Trial::AwaitingStart);
                        Ok(())
                    });
                }
            }
            return Err(format!("启动确认保存失败：{e}；停止结果：{stop:?}"));
        }
        Ok(())
    }
    fn start_process(&self, op: &Operation) -> Result<(), String> {
        if self.runtime.snapshot().pid.is_some() {
            return Err("sing-box 已在运行".into());
        }
        self.runtime.stopped()?;
        self.runtime.clear_error();
        if let Err(e) = self.engine.adopt_owned(op) {
            self.engine.managed.error(e);
        }
        self.engine.validate_pending(&op.cancel)?;
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
            if let Err(e) = self.engine.selected(&op.cancel) {
                self.engine.usable.store(false, Ordering::Release);
                return Err(e);
            }
            self.runtime.start(&spec, op.cancel.clone())
        })();
        if result.is_ok() {
            self.engine.usable.store(true, Ordering::Release);
            return Ok(());
        }
        let error = result.unwrap_err();
        if !trial {
            if s.managed()
                && !self.engine.usable.load(Ordering::Acquire)
                && !op.cancel.load(Ordering::Acquire)
                && self.engine.rollback(&error, &op.cancel)?
            {
                self.engine
                    .notify(format!("当前核心文件不可用，已回退：{error}"));
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
        if self.engine.rollback(&error, &op.cancel)? {
            self.engine
                .notify(format!("新版启动失败，已回退上一版本：{error}"));
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
        self.engine
            .managed
            .suppressed
            .store(true, Ordering::Release);
        Err(error)
    }
    pub fn stop(&self) -> Result<(), String> {
        let op = self.gate.begin(Kind::Stop)?;
        self.invalidate_restore();
        self.stop_owned(&op)
    }
    fn stop_owned(&self, op: &Operation) -> Result<(), String> {
        let saved = self.store.resume(false);
        self.gate.cancel_runtime();
        // When cancelling an in-flight start, its owner must release the runtime ticket first.
        if self.gate.runtime_other(op.id) {
            self.gate.wait_runtime(op.id)?;
        }
        let stopped = self.runtime.stop();
        if let Err(e) = saved {
            return Err(format!(
                "恢复状态保存失败，下次启动仍可能恢复：{e}；实际停止结果：{stopped:?}"
            ));
        }
        stopped
    }
    pub fn invalidate_restore(&self) {
        let mut i = self.intent.lock().unwrap();
        i.epoch += 1;
        i.restore = Restore::Finished;
        if let Some(id) = i.restore_operation {
            self.gate.cancel_operation(id);
        }
    }
    pub fn set_restore(&self, enabled: bool) -> Result<settings::Settings, String> {
        let value = self.store.restore_preference(enabled)?;
        if !enabled {
            let mut i = self.intent.lock().unwrap();
            if matches!(i.restore, Restore::Waiting | Restore::Starting) {
                i.epoch += 1;
                i.restore = Restore::Finished;
                if let Some(id) = i.restore_operation {
                    self.gate.cancel_operation(id);
                }
            }
        }
        Ok(value)
    }
    fn arm_restore(&self) {
        let s = self.store.snapshot();
        let mut i = self.intent.lock().unwrap();
        if i.epoch == 0 && s.startup.restore_core_on_launch && s.startup.resume_core {
            if self.engine.restore_safe.load(Ordering::Acquire) && self.store.error.is_none() {
                i.restore = Restore::Waiting;
            } else {
                i.restore = Restore::Finished;
                self.engine
                    .notify("自动恢复已暂停：上次启动未确认，请手动处理");
            }
        }
    }
    pub fn restore_tick(self: &Arc<Self>) {
        let mut i = self.intent.lock().unwrap();
        if i.restore != Restore::Waiting || self.gate.closing() {
            return;
        }
        let s = self.store.snapshot();
        let fail = if !s.startup.restore_core_on_launch || !s.startup.resume_core {
            Some("自动恢复已取消".into())
        } else if s.config_path.is_none() && s.pending_config_path.is_none() {
            Some("自动恢复失败：请先导入配置".into())
        } else if !s.managed() && !self.engine.usable.load(Ordering::Acquire) {
            Some("自动恢复失败：外部核心路径不可用".into())
        } else if !self.engine.usable.load(Ordering::Acquire)
            && !self.gate.core_fetching()
            && matches!(
                self.engine.managed.snapshot().phase,
                Phase::Error | Phase::Paused
            )
        {
            Some("自动恢复失败：必需核心准备未完成，请手动重试".into())
        } else {
            None
        };
        if let Some(error) = fail {
            i.restore = Restore::Finished;
            self.runtime.report(error);
            return;
        }
        if !self.engine.usable.load(Ordering::Acquire) || self.gate.runtime_busy() {
            return;
        }
        let Ok(op) = self.gate.begin(Kind::Runtime) else {
            return;
        };
        i.restore = Restore::Starting;
        i.restore_operation = Some(op.id);
        drop(i);
        let core = self.clone();
        tauri::async_runtime::spawn_blocking(move || {
            if let Err(e) = core.start_owned(&op) {
                core.runtime.report(format!("自动恢复失败：{e}"));
            }
            let mut i = core.intent.lock().unwrap();
            i.restore = Restore::Finished;
            i.restore_operation = None;
            drop(i);
            drop(op);
            core.wake.notify_one();
        });
    }
}
