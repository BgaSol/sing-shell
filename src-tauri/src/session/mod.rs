use crate::{
    app::{Admission, Kind, Operation, Receipt},
    core::managed::Phase,
    core::{self, package, CoreService},
    runtime::{self, RuntimeService, StartSpec},
    settings::{self, SettingsStore, Trial},
};
pub mod tunnel;
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
    Apply,
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
    pub mode: crate::profile::Mode,
    pub session_id: Option<String>,
    #[serde(flatten)]
    pub core: core::Snapshot,
    pub restore: Restore,
    pub can_start: bool,
    pub can_stop: bool,
    pub can_import: bool,
    pub runtime: runtime::RuntimeSnapshot,
    pub network: crate::platform::network::Observation,
    pub running_node: Option<crate::profile::NodeSummary>,
    pub running_connection_options: Option<crate::profile::ConnectionOptions>,
    pub pending_changes: bool,
    pub target: Option<crate::profile::NodeSummary>,
    pub operation: Option<String>,
    pub activity: Option<crate::activity::Summary>,
    pub phase: String,
    pub authorization_stage: Option<crate::platform::authorization::Stage>,
}
pub struct SessionService {
    pub engine: Arc<CoreService>,
    pub store: Arc<SettingsStore>,
    pub runtime: Arc<RuntimeService>,
    pub gate: Arc<Admission>,
    wake: Arc<tokio::sync::Notify>,
    intent: Mutex<Intent>,
    tunnel: tunnel::Tunnel,
    applied: Mutex<Option<crate::profile::ConnectionInput>>,
    target: Mutex<Option<crate::profile::NodeSummary>>,
}
fn missing_restore_config(s: &settings::Settings) -> bool {
    s.profile.mode == crate::profile::Mode::ExternalConfig
        && s.config_path.is_none()
        && s.pending_config_path.is_none()
}
impl SessionService {
    pub fn new(engine: Arc<CoreService>, wake: Arc<tokio::sync::Notify>) -> Arc<Self> {
        Arc::new(Self {
            tunnel: tunnel::Tunnel::with_occupation(
                engine.store.root.clone(),
                engine.runtime.network_occupation(),
            ),
            applied: Mutex::new(None),
            target: Mutex::new(None),
            store: engine.store.clone(),
            runtime: engine.runtime.clone(),
            gate: engine.gate.clone(),
            engine,
            wake,
            intent: Mutex::new(Intent::default()),
        })
    }
    pub fn initialize(&self) -> Result<(), String> {
        crate::profile::config::remove_generated(&self.store.root)?;
        self.engine.initialize()?;
        self.arm_restore();
        Ok(())
    }
    pub fn snapshot(&self) -> Snapshot {
        self.snapshot_for(&self.store.snapshot())
    }
    pub fn runtime_snapshot(&self) -> runtime::RuntimeSnapshot {
        self.snapshot().runtime
    }
    pub fn logs(&self) -> Vec<crate::logs::Log> {
        if self.store.snapshot().profile.mode == crate::profile::Mode::Generated {
            self.tunnel.snapshot().logs
        } else {
            self.runtime.logs.snapshot()
        }
    }
    pub fn snapshot_for(&self, settings: &settings::Settings) -> Snapshot {
        let generated = settings.profile.mode == crate::profile::Mode::Generated;
        let tunnel = self.tunnel.status();
        let mut actual = self.runtime.snapshot();
        if generated {
            let local_error = actual.error;
            actual = tunnel.runtime.clone();
            actual.error = tunnel.error.clone().or(actual.error).or(local_error);
            actual.status = tunnel.phase.runtime_status().into();
            actual.cleanup_pending = tunnel.phase == tunnel::Phase::CleanupRequired;
        }
        let r = &actual;
        let core = self.engine.snapshot();
        let busy = self.gate.runtime_busy() || self.gate.closing() || !core.initialized;
        let restore = self.intent.lock().unwrap().restore;
        let applied = self.applied.lock().unwrap();
        let desired = settings.profile.selected().and_then(|node| {
            self.connection_for(settings, node.clone(), String::new())
                .ok()
        });
        Snapshot {
            mode: settings.profile.mode,
            session_id: tunnel.session_id.clone(),
            target: self.target.lock().unwrap().clone(),
            operation: tunnel.operation.clone(),
            activity: tunnel.activity_summary.clone(),
            can_start: !busy
                && r.pid.is_none()
                && !r.cleanup_pending
                && self.store.error().is_none()
                && core.usable
                && if generated {
                    settings.profile.selected_node.is_some()
                } else {
                    settings.config_path.is_some()
                },
            can_stop: !self.gate.closing()
                && (self.tunnel.occupied()
                    || r.pid.is_some()
                    || r.cleanup_pending
                    || r.status == "starting"
                    || self.gate.runtime_busy()
                    || matches!(restore, Restore::Waiting | Restore::Starting)),
            can_import: !busy
                && r.pid.is_none()
                && !r.cleanup_pending
                && self.runtime.stopped().is_ok()
                && self.store.error().is_none(),
            core,
            restore,
            runtime: r.clone(),
            network: tunnel.network,
            running_node: if generated {
                tunnel.running_node.clone()
            } else {
                None
            },
            running_connection_options: if generated {
                tunnel.running_connection_options
            } else {
                None
            },
            pending_changes: r.pid.is_some()
                && desired
                    .as_ref()
                    .map(crate::profile::ConnectionInput::fingerprint)
                    != applied
                        .as_ref()
                        .map(crate::profile::ConnectionInput::fingerprint),
            phase: if generated {
                tunnel.phase.as_str().into()
            } else {
                r.status.clone()
            },
            authorization_stage: if generated {
                tunnel.authorization_stage
            } else {
                None
            },
        }
    }
    pub fn select_core(&self, path: &Path) -> Result<(), String> {
        self.invalidate_restore();
        self.ensure_stopped()?;
        self.engine.select(path)
    }
    pub fn reset_settings(&self) -> Result<std::path::PathBuf, String> {
        let op = self.gate.begin(Kind::Selection)?;
        let _files = self.gate.begin(Kind::CoreFetch)?;
        self.ensure_stopped()?;
        let backup = self.store.backup_reset()?;
        self.invalidate_restore();
        self.runtime.clear_error();
        self.engine.reset_owned(&op);
        self.wake.notify_one();
        Ok(backup)
    }
    pub fn import_config(&self, path: &Path) -> Result<(), String> {
        self.invalidate_restore();
        self.ensure_stopped()?;
        self.engine.import(path)
    }
    pub fn use_managed(self: &Arc<Self>) -> Result<Receipt, String> {
        self.invalidate_restore();
        self.ensure_stopped()?;
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
        self.tunnel.close()?;
        self.applied.lock().unwrap().take();
        self.runtime.stop()?;
        crate::profile::config::remove_generated(&self.store.root)
    }
    pub fn ensure_stopped(&self) -> Result<(), String> {
        let s = self.runtime_snapshot();
        if s.pid.is_some()
            || s.cleanup_pending
            || ["starting", "stopping"].contains(&s.status.as_str())
        {
            Err("请先停止连接并完成网络清理".into())
        } else {
            Ok(())
        }
    }
    pub fn mode(
        &self,
        mode: crate::profile::Mode,
        expected: u64,
    ) -> Result<crate::profile::service::Snapshot, String> {
        let _op = self.gate.begin(Kind::Selection)?;
        self.ensure_stopped()?;
        self.invalidate_restore();
        self.store.profile(expected, |p| {
            p.mode = mode;
            Ok(())
        })?;
        self.store.resume(false)?;
        let s = self.store.snapshot();
        Ok(crate::profile::service::Snapshot {
            revision: s.nodes_revision,
            routing_revision: s.routing_revision,
            profile: s.profile.public(),
        })
    }
    pub fn action(self: &Arc<Self>, action: Action) -> Result<Receipt, String> {
        let op = self.gate.begin(if matches!(action, Action::Stop) {
            Kind::Stop
        } else {
            Kind::Runtime
        })?;
        if matches!(action, Action::Start | Action::Stop | Action::Apply) {
            self.invalidate_restore();
        }
        let receipt = op.receipt();
        let core = self.clone();
        tauri::async_runtime::spawn_blocking(move || {
            let result = match action {
                Action::Start => core.start_owned(&op),
                Action::Stop => core.stop_owned(&op),
                Action::Check => core.check_owned(&op),
                Action::Apply => core.apply_owned(&op),
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
        if self.store.snapshot().profile.mode == crate::profile::Mode::Generated {
            return self.start_tunnel_owned(op, None);
        }
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
    fn connection_for(
        &self,
        settings: &settings::Settings,
        node: crate::profile::Node,
        version: String,
    ) -> Result<crate::profile::ConnectionInput, String> {
        crate::profile::ConnectionInput::from_profile(
            &settings.profile,
            node,
            crate::rulesets::selected(&self.store.root, &settings.profile.routing)?,
            version,
        )
    }
    fn desired_connection(&self) -> Result<crate::profile::ConnectionInput, String> {
        let s = self.store.snapshot();
        self.connection_for(
            &s,
            s.profile
                .selected()
                .ok_or("请先导入并选择一个节点")?
                .clone(),
            s.managed
                .active
                .as_ref()
                .ok_or("核心尚未就绪")?
                .version
                .clone(),
        )
    }
    fn check_owned(&self, op: &Operation) -> Result<(), String> {
        let s = self.store.snapshot();
        if s.profile.mode == crate::profile::Mode::Generated {
            let config = crate::profile::config::build(
                &s.profile,
                &self.store.root,
                &s.managed.active.as_ref().ok_or("核心尚未就绪")?.version,
            )?;
            self.runtime.validate(
                &s.selected_path(&self.store.root)?,
                &config.path,
                &op.cancel,
            )
        } else {
            self.engine.check_local(&op.cancel)
        }
    }
    pub fn connect(self: &Arc<Self>, node_id: String) -> Result<Receipt, String> {
        self.invalidate_restore();
        let op = self.gate.begin(Kind::Runtime)?;
        let receipt = op.receipt();
        let session = self.clone();
        {
            let settings = self.store.snapshot();
            if settings.profile.mode != crate::profile::Mode::Generated {
                return Err("请先停止并切换到节点模式".into());
            }
            let node = settings
                .profile
                .nodes
                .iter()
                .find(|n| n.id == node_id)
                .ok_or("节点不存在")?;
            *self.target.lock().unwrap() = Some(node.summary());
        }
        self.runtime.events.push(
            "connection",
            "started",
            format!(
                "连接节点：{}",
                self.target.lock().unwrap().as_ref().unwrap().name
            ),
        );
        tauri::async_runtime::spawn_blocking(move || {
            let result = (|| {
                let switching = session.tunnel.status().runtime.pid.is_some();
                if !switching {
                    session.ensure_stopped()?;
                    if let Err(e) = session.engine.adopt_owned(&op) {
                        session.engine.managed.error(e);
                    }
                }
                let settings = session.store.snapshot();
                let node = settings
                    .profile
                    .nodes
                    .iter()
                    .find(|n| n.id == node_id)
                    .ok_or("节点不存在")?
                    .clone();
                let version = if switching {
                    session
                        .applied
                        .lock()
                        .unwrap()
                        .as_ref()
                        .ok_or("当前连接信息缺失")?
                        .core_version
                        .clone()
                } else {
                    settings
                        .managed
                        .active
                        .as_ref()
                        .ok_or("请等待官方核心就绪")?
                        .version
                        .clone()
                };
                let input = session.connection_for(&settings, node, version)?;
                session.execute_connection(&op, input, !switching)
            })();
            if let Err(e) = result {
                session.runtime.report(e);
            }
            session.target.lock().unwrap().take();
            drop(op);
            session.wake.notify_one();
        });
        Ok(receipt)
    }
    fn start_tunnel_owned(
        &self,
        op: &Operation,
        input: Option<crate::profile::ConnectionInput>,
    ) -> Result<(), String> {
        self.ensure_stopped()?;
        let explicit = input.is_some();
        if !explicit {
            if let Err(e) = self.engine.adopt_owned(op) {
                self.engine.managed.error(e);
            }
        }
        let input = match input {
            Some(input) => input,
            None => self.desired_connection()?,
        };
        self.execute_connection(op, input, !explicit)
    }
    fn execute_connection(
        &self,
        op: &Operation,
        mut input: crate::profile::ConnectionInput,
        allow_core_rollback: bool,
    ) -> Result<(), String> {
        if self.tunnel.status().phase == tunnel::Phase::CleanupRequired {
            return Err("请先停止并完成网络清理".into());
        }
        let epoch = self.intent.lock().unwrap().epoch;
        let saved = self.store.snapshot();
        if !saved.managed() {
            return Err("受控 TUN 仅使用官方受管核心".into());
        }
        input.validate()?;
        let reference = [
            saved.managed.active.as_ref(),
            saved.managed.previous.as_ref(),
        ]
        .into_iter()
        .flatten()
        .find(|r| r.version == input.core_version)
        .ok_or("固定核心材料缺失，请手动连接")?;
        if let Err(error) = self.engine.verify(reference, &op.cancel) {
            if allow_core_rollback
                && saved.managed.trial.is_some()
                && !op.cancel.load(Ordering::Acquire)
                && !self.gate.closing()
                && self.engine.rollback(&error, &op.cancel)?
            {
                input.core_version = self
                    .store
                    .snapshot()
                    .managed
                    .active
                    .as_ref()
                    .ok_or("没有上一核心")?
                    .version
                    .clone();
                return self
                    .execute_connection(op, input, false)
                    .map_err(|previous| format!("核心验证失败：{error}；上一版本：{previous}"));
            }
            return Err(error);
        }
        self.tunnel.prepare_core(&input.core_version, &op.cancel)?;
        let expected = if saved.managed.trial.is_some() && allow_core_rollback {
            self.store
                .core(saved.core_revision, |s| {
                    s.managed.trial = Some(Trial::Starting);
                    Ok(())
                })?
                .core_revision
        } else {
            saved.core_revision
        };
        self.runtime.clear_error();
        let result = self
            .tunnel
            .start(input.clone(), input.core_version.clone(), &op.cancel);
        if let Err(error) = result {
            if saved.managed.trial.is_some()
                && allow_core_rollback
                && self.tunnel.status().failure != Some(tunnel::Failure::CoreStart)
            {
                self.store.core(expected, |s| {
                    s.managed.trial = Some(Trial::AwaitingStart);
                    Ok(())
                })?;
            }
            if allow_core_rollback
                && saved.managed.trial.is_some()
                && self.tunnel.status().failure == Some(tunnel::Failure::CoreStart)
                && !op.cancel.load(Ordering::Acquire)
                && !self.gate.closing()
            {
                self.tunnel.stop()?;
                if self.engine.rollback(&error, &op.cancel)? {
                    let previous = self.store.snapshot();
                    input.core_version = previous
                        .managed
                        .active
                        .as_ref()
                        .ok_or("没有可回退核心")?
                        .version
                        .clone();
                    return self
                        .execute_connection(op, input, false)
                        .map_err(|e| format!("新核心失败：{error}；上一核心失败：{e}"));
                }
            }
            return Err(error);
        }
        let intent = self.intent.lock().unwrap();
        if intent.epoch != epoch || op.cancel.load(Ordering::Acquire) || self.gate.closing() {
            drop(intent);
            self.tunnel.close()?;
            return Err("连接已取消".into());
        }
        let result = self.store.confirm_connection(expected, Some(input.clone()));
        drop(intent);
        if let Err(e) = result {
            let cleanup = self.tunnel.close();
            return Err(format!("启动确认保存失败：{e}；停止结果：{cleanup:?}"));
        }
        self.runtime.events.push(
            "connection",
            "succeeded",
            format!(
                "{} 已连接，TUN 双栈、路由和临时 DNS 已确认",
                input.node.name
            ),
        );
        *self.applied.lock().unwrap() = Some(input);
        Ok(())
    }
    fn apply_owned(&self, op: &Operation) -> Result<(), String> {
        if self.store.snapshot().profile.mode != crate::profile::Mode::Generated {
            return Err("原始配置请停止后重新启动".into());
        }
        let mut candidate = self.desired_connection()?;
        if let Some(actual) = self.applied.lock().unwrap().as_ref() {
            candidate.core_version = actual.core_version.clone();
        }
        self.execute_connection(op, candidate, false)
    }
    pub fn activity(&self, query: crate::activity::Query) -> Result<crate::activity::Page, String> {
        query.validate()?;
        if self.store.snapshot().profile.mode != crate::profile::Mode::Generated
            || !self.tunnel.occupied()
        {
            return Ok(crate::activity::Page {
                summary: crate::activity::Summary {
                    state: "unavailable".into(),
                    ..Default::default()
                },
                ..Default::default()
            });
        }
        self.tunnel.activity(query)
    }
    pub fn read_logs(&self, after: u64, limit: usize) -> Result<Vec<crate::logs::Log>, String> {
        if limit == 0 || limit > 100 {
            return Err("日志分页超限".into());
        }
        if self.store.snapshot().profile.mode == crate::profile::Mode::Generated {
            let logs = self.tunnel.logs(after, limit)?;
            if logs.is_empty() && !self.tunnel.occupied() {
                Ok(self.runtime.logs.page(after, limit))
            } else {
                Ok(logs)
            }
        } else {
            Ok(self.runtime.logs.page(after, limit))
        }
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
        self.runtime
            .events
            .push("connection", "started", "正在断开并释放网络");
        let saved = self.store.resume(false);
        self.gate.cancel_runtime();
        // When cancelling an in-flight start, its owner must release the runtime ticket first.
        if self.gate.runtime_other(op.id) {
            self.gate.wait_runtime(op.id)?;
        }
        let stopped = self.tunnel.close().and_then(|_| {
            self.applied.lock().unwrap().take();
            self.runtime.stop()?;
            crate::profile::config::remove_generated(&self.store.root)
        });
        if let Err(e) = saved {
            return Err(format!(
                "恢复状态保存失败，下次启动仍可能恢复：{e}；实际停止结果：{stopped:?}"
            ));
        }
        if stopped.is_ok() {
            self.runtime
                .events
                .push("connection", "succeeded", "连接已断开，网络资源已释放");
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
            if self.engine.restore_safe.load(Ordering::Acquire) && self.store.error().is_none() {
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
        } else if s.profile.mode == crate::profile::Mode::Generated
            && (!crate::platform::authorization::can_resume()
                || s.startup.last_connection.is_none())
        {
            Some("TUN 自动恢复需要手动确认系统授权，请点击启动；不会在后台反复请求权限".into())
        } else if missing_restore_config(&s) {
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
            let s = core.store.snapshot();
            let result = if s.profile.mode == crate::profile::Mode::Generated {
                core.start_tunnel_owned(&op, s.startup.last_connection)
            } else {
                core.start_owned(&op)
            };
            if let Err(e) = result {
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
impl Drop for SessionService {
    fn drop(&mut self) {
        let _ = self.stop_internal();
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn node_restore_does_not_require_an_external_configuration() {
        let mut settings = crate::settings::Settings::default();
        assert!(settings.profile.mode == crate::profile::Mode::Generated);
        assert!(!super::missing_restore_config(&settings));
        settings.profile.mode = crate::profile::Mode::ExternalConfig;
        assert!(super::missing_restore_config(&settings));
        settings.config_path = Some(std::path::PathBuf::from("config.json"));
        assert!(!super::missing_restore_config(&settings));
    }
}
