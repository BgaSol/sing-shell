use super::{ConnectionInput, Failure, Phase, Snapshot};
use crate::{
    activity::Collector,
    platform::{dns::Dns, network},
    runtime::{RuntimeService, StartSpec},
};
use std::{
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
#[derive(Default)]
pub(super) struct Resources {
    dns: Dns,
    network: bool,
    directory: Option<PathBuf>,
}
struct Started {
    resources: Resources,
    result: Result<network::Observation, (Failure, String)>,
    actual: Option<RunningConnection>,
    error: Option<String>,
}
use super::preparation::{prepare, Prepared, RunningConnection};
pub(super) struct Worker {
    pub(super) root: PathBuf,
    pub(super) transfer: Option<super::install::Transfer>,
    owner_key: String,
    pub(super) runtime: Arc<RuntimeService>,
    pub(super) state: Arc<Mutex<Snapshot>>,
    pub(super) cancel: Arc<AtomicBool>,
    start: Option<JoinHandle<Started>>,
    resources: Resources,
    actual: Option<RunningConnection>,
    collector: Arc<Mutex<Option<Collector>>>,
    observed: Instant,
    preparation: Arc<Mutex<super::resolver::Resources>>,
}
impl Worker {
    pub(super) fn new(root: PathBuf, owner_key: String) -> Self {
        Self {
            resources: Resources {
                directory: recorded_directory(&root),
                network: root.join("network-process.json").exists(),
                ..Default::default()
            },
            root,
            transfer: None,
            owner_key,
            runtime: Arc::new(Default::default()),
            state: Arc::new(Mutex::new(Snapshot::default())),
            cancel: Arc::new(AtomicBool::new(false)),
            start: None,
            actual: None,
            collector: Arc::new(Mutex::new(None)),
            observed: Instant::now(),
            preparation: Arc::new(Mutex::new(Default::default())),
        }
    }
    pub(super) fn snapshot(&self) -> Snapshot {
        let mut s = self.state.lock().unwrap().clone();
        s.runtime = self.runtime.snapshot();
        if s.phase == Phase::Running && s.runtime.pid.is_none() {
            s.phase = if s.operation.is_some() {
                Phase::Preparing
            } else {
                Phase::Failed
            };
        }
        s.running_node = if s.phase == Phase::Running && s.runtime.pid.is_some() {
            self.actual.as_ref().map(|c| c.node.summary())
        } else {
            None
        };
        s.running_connection_options = if s.running_node.is_some() {
            self.actual.as_ref().map(|c| c.options)
        } else {
            None
        };
        s.running_bypass = s.running_node.is_some()
            && self
                .actual
                .as_ref()
                .is_some_and(|c| !c.bypass.prefixes.is_empty());
        s.core_received = self.transfer.as_ref().map(|t| t.received);
        s.logs.clear();
        s.activity_summary = self
            .collector
            .lock()
            .unwrap()
            .as_ref()
            .map(Collector::summary);
        s
    }
    pub(super) fn activity(
        &self,
        q: &crate::activity::Query,
    ) -> Result<crate::activity::Page, String> {
        match self.collector.lock().unwrap().as_ref() {
            Some(c) => c.page(q),
            None => Ok(crate::activity::Page {
                summary: crate::activity::Summary {
                    state: "unavailable".into(),
                    ..Default::default()
                },
                ..Default::default()
            }),
        }
    }
    fn collect(&mut self) -> Option<Result<network::Observation, (Failure, String)>> {
        if !self.start.as_ref().is_some_and(|t| t.is_finished()) {
            return None;
        }
        let task = self.start.take().unwrap().join();
        Some(match task {
            Ok(done) => {
                self.resources = done.resources;
                self.actual = done.actual;
                self.state.lock().unwrap().operation_error = done.error;
                done.result
            }
            Err(_) => {
                self.resources.network = true;
                Err((Failure::Preparation, "连接任务异常退出".into()))
            }
        })
    }
    pub(super) fn observe_exit(&mut self) {
        if let Some(result) = self.collect() {
            self.state.lock().unwrap().operation = None;
            match result {
                Ok(n) if !self.cancel.load(Ordering::Acquire) => {
                    let mut s = self.state.lock().unwrap();
                    s.network = n;
                    s.phase = Phase::Running;
                    s.failure = None;
                    s.error = None;
                }
                Ok(_) => self.fail(Failure::Preparation, "连接已取消".into()),
                Err((f, e)) => self.fail(f, e),
            }
        }
        if self.start.is_some() || self.state.lock().unwrap().phase != Phase::Running {
            return;
        }
        if self.runtime.snapshot().pid.is_none() {
            self.fail(
                Failure::Network,
                self.runtime
                    .snapshot()
                    .error
                    .unwrap_or_else(|| "核心已退出，请手动重新连接".into()),
            );
        } else if self.observed.elapsed() >= Duration::from_secs(2) {
            self.observed = Instant::now();
            if let Err(e) = self.resources.dns.check() {
                self.fail(Failure::Network, e);
            }
        }
    }
    fn fail(&mut self, failure: Failure, error: String) {
        let result = cleanup(
            &self.root,
            &self.runtime,
            &self.state,
            &mut self.resources,
            &self.collector,
        );
        let result = result.and_then(|_| {
            self.preparation
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .close(&self.root)
        });
        let mut s = self.state.lock().unwrap();
        s.phase = if result.is_ok() && failure != Failure::Cleanup {
            Phase::Failed
        } else {
            Phase::CleanupRequired
        };
        s.failure = Some(if result.is_ok() {
            failure
        } else {
            Failure::Cleanup
        });
        s.error = Some(match result {
            Ok(()) => error,
            Err(e) => format!("{error}；清理失败：{e}"),
        });
        s.operation = None;
    }
    pub(super) fn begin_core(
        &mut self,
        version: &str,
        material: Option<crate::core::material::Envelope>,
        operation_id: u64,
    ) -> Result<(), String> {
        if self.snapshot().operation.is_some() {
            return Err("已有连接操作".into());
        }
        self.transfer.take();
        let running = self.runtime.snapshot().pid.is_some();
        {
            let mut state = self.state.lock().unwrap();
            state.error = None;
            state.operation_error = None;
            state.failure = None;
            state.operation_id = operation_id;
            if !running {
                state.phase = Phase::Preparing;
            }
        }
        match material {
            Some(material) => {
                super::install::Transfer::begin(&self.root, version, material, &self.runtime)
                    .map(|transfer| self.transfer = transfer)
            }
            None => super::install::existing(
                &self.root,
                version,
                &self.runtime,
                &AtomicBool::new(false),
            )
            .map(|_| ()),
        }
    }
    pub(super) fn start(
        &mut self,
        input: ConnectionInput,
        version: String,
        operation_id: u64,
    ) -> Result<(), String> {
        if self.start.is_some()
            || matches!(
                self.snapshot().phase,
                Phase::CleanupRequired | Phase::Stopping
            )
        {
            return Err("已有连接操作或尚未完成清理".into());
        }
        input.validate()?;
        if input.core_version != version {
            return Err("固定核心版本不符".into());
        }
        self.cancel = Arc::new(AtomicBool::new(false));
        let cancel = self.cancel.clone();
        let runtime = self.runtime.clone();
        let state = self.state.clone();
        let root = self.root.clone();
        let owner_key = self.owner_key.clone();
        let collector = self.collector.clone();
        let old = self.actual.clone();
        let preparation = self.preparation.clone();
        let mut resources = std::mem::take(&mut self.resources);
        {
            let mut s = state.lock().unwrap();
            s.operation_id = operation_id;
            s.operation = Some("preparing".into());
            s.operation_error = None;
            if old.is_none() {
                s.phase = Phase::Preparing;
            }
        }
        self.start = Some(thread::spawn(move || {
            state.lock().unwrap().operation = Some(
                if crate::profile::bypass::Targets::collect(&input.routing)
                    .is_ok_and(|t| !t.domains.is_empty())
                {
                    "resolvingBypass"
                } else {
                    "checkingConfig"
                }
                .into(),
            );
            let prepared = prepare(&root, &owner_key, input, None, &cancel, &preparation);
            let candidate = match prepared {
                Ok(v) => v,
                Err(e) => {
                    let clean = preparation
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .close(&root);
                    if let Err(clean) = clean {
                        return Started {
                            resources,
                            result: Err((Failure::Cleanup, format!("{e}；{clean}"))),
                            actual: old,
                            error: None,
                        };
                    }
                    if old.is_some()
                        && runtime.snapshot().pid.is_some()
                        && !cancel.load(Ordering::Acquire)
                    {
                        let network = state.lock().unwrap().network.clone();
                        return Started {
                            resources,
                            result: Ok(network),
                            actual: old,
                            error: Some(e),
                        };
                    }
                    return Started {
                        resources,
                        result: Err((Failure::Preparation, e)),
                        actual: old,
                        error: None,
                    };
                }
            };
            {
                let mut s = state.lock().unwrap();
                s.operation = Some("switching".into());
                s.phase = Phase::Stopping;
            }
            if let Err(e) = cleanup(&root, &runtime, &state, &mut resources, &collector) {
                let _ = preparation
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .close(&root);
                return Started {
                    resources,
                    result: Err((Failure::Cleanup, e)),
                    actual: None,
                    error: None,
                };
            }
            let requested = candidate.running.clone();
            let first = launch(
                &root,
                candidate,
                &preparation,
                &runtime,
                &state,
                &cancel,
                &mut resources,
                &collector,
            );
            match first {
                Ok(n) => Started {
                    resources,
                    result: Ok(n),
                    actual: Some(requested),
                    error: None,
                },
                Err((failure, error)) => {
                    if let Err(clean) = cleanup(&root, &runtime, &state, &mut resources, &collector)
                        .and_then(|_| {
                            preparation
                                .lock()
                                .unwrap_or_else(|p| p.into_inner())
                                .close(&root)
                        })
                    {
                        return Started {
                            resources,
                            result: Err((Failure::Cleanup, format!("{error}；{clean}"))),
                            actual: None,
                            error: None,
                        };
                    }
                    if !cancel.load(Ordering::Acquire) {
                        if let Some(previous) = old {
                            let restored = prepare(
                                &root,
                                &owner_key,
                                previous.input.clone(),
                                Some(previous.bypass.clone()),
                                &cancel,
                                &preparation,
                            )
                            .map_err(|e| (Failure::Preparation, e))
                            .and_then(|p| {
                                launch(
                                    &root,
                                    p,
                                    &preparation,
                                    &runtime,
                                    &state,
                                    &cancel,
                                    &mut resources,
                                    &collector,
                                )
                            });
                            return match restored {
                                Ok(n) => Started {
                                    resources,
                                    result: Ok(n),
                                    actual: Some(previous),
                                    error: Some(format!("切换失败，已恢复上一连接：{error}")),
                                },
                                Err((f, e)) => Started {
                                    resources,
                                    result: Err((
                                        f,
                                        format!("切换失败：{error}；上一连接恢复失败：{e}"),
                                    )),
                                    actual: None,
                                    error: None,
                                },
                            };
                        }
                    }
                    Started {
                        resources,
                        result: Err((failure, error)),
                        actual: None,
                        error: None,
                    }
                }
            }
        }));
        Ok(())
    }
    pub(super) fn stop(&mut self) -> Result<(), String> {
        self.transfer.take();
        self.cancel.store(true, Ordering::Release);
        self.state.lock().unwrap().phase = Phase::Stopping;
        let deadline = Instant::now() + Duration::from_secs(30);
        while self.start.as_ref().is_some_and(|t| !t.is_finished()) {
            if Instant::now() >= deadline {
                let mut s = self.state.lock().unwrap();
                s.phase = Phase::CleanupRequired;
                s.failure = Some(Failure::Cleanup);
                return Err("准备任务尚未退出，请重试停止".into());
            }
            thread::sleep(Duration::from_millis(25));
        }
        self.collect();
        let result = cleanup(
            &self.root,
            &self.runtime,
            &self.state,
            &mut self.resources,
            &self.collector,
        );
        let result = result.and_then(|_| {
            self.preparation
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .close(&self.root)
        });
        match &result {
            Ok(()) => {
                self.actual = None;
                *self.state.lock().unwrap() = Snapshot::default();
            }
            Err(e) => {
                let mut s = self.state.lock().unwrap();
                s.phase = Phase::CleanupRequired;
                s.failure = Some(Failure::Cleanup);
                s.error = Some(e.clone());
            }
        }
        result
    }
    pub(super) fn finish(&mut self) -> Result<(), String> {
        self.cancel.store(true, Ordering::Release);
        while self.start.as_ref().is_some_and(|t| !t.is_finished()) {
            thread::sleep(Duration::from_millis(25));
        }
        self.stop()
    }
}
fn launch(
    root: &Path,
    prepared: Prepared,
    preparation: &Mutex<super::resolver::Resources>,
    runtime: &RuntimeService,
    state: &Mutex<Snapshot>,
    cancel: &Arc<AtomicBool>,
    resources: &mut Resources,
    collector: &Mutex<Option<Collector>>,
) -> Result<network::Observation, (Failure, String)> {
    resources.directory = Some(prepared.directory);
    // Old readers have joined before launch; stored log lines are already redacted.
    let mut secrets = prepared.running.input.node.spec.secrets();
    if let Some(context) = &prepared.context {
        secrets.push(context.secret.clone());
        secrets.push(format!("127.0.0.1:{}", context.port));
    }
    runtime.logs.redact(secrets);
    let mut failure = Failure::Cleanup;
    let result = (|| {
        crate::core::package::cancelled(cancel)?;
        resources.network = true;
        network::recover(root)?;
        network::require_clear()?;
        {
            let mut state = state.lock().unwrap();
            state.phase = Phase::Starting;
            state.session_id = Some(prepared.session_id.clone());
        }
        crate::settings::write_json(&root.join("runtime/current.json"), &prepared.session_id)?;
        preparation
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .release(root)?;
        failure = Failure::CoreStart;
        runtime.start(
            &StartSpec {
                executable: prepared.executable,
                config: prepared.config,
                version: Some(prepared.running.input.core_version.clone()),
            },
            cancel.clone(),
        )?;
        failure = Failure::Network;
        let deadline = Instant::now() + Duration::from_secs(12);
        let observation = loop {
            crate::core::package::cancelled(cancel)?;
            if runtime.snapshot().pid.is_none() {
                return Err("核心已退出，TUN 未建立".into());
            }
            let n = network::observe_expected(&prepared.running.bypass.prefixes)?;
            if n.ready() {
                break n;
            }
            if Instant::now() >= deadline {
                return Err("未能确认 TUN 双栈和路由覆盖".into());
            }
            thread::sleep(Duration::from_millis(100));
        };
        resources.dns.install()?;
        crate::core::package::cancelled(cancel)?;
        if runtime.snapshot().pid.is_none() {
            return Err("DNS 接管期间核心已退出".into());
        }
        crate::private_files::directory(&root.join("rules"))?;
        crate::settings::write_json(
            &root.join("rules/pinned.json"),
            &prepared.running.input.versions,
        )?;
        if let Some(context) = prepared.context {
            *collector.lock().unwrap() =
                Some(Collector::start(context, &prepared.running.input.routing));
        }
        Ok(observation)
    })();
    result.map_err(|e| (failure, e))
}
fn cleanup(
    root: &Path,
    runtime: &RuntimeService,
    state: &Mutex<Snapshot>,
    resources: &mut Resources,
    collector: &Mutex<Option<Collector>>,
) -> Result<(), String> {
    collector.lock().unwrap().take();
    let mut errors = Vec::new();
    if let Err(e) = resources.dns.close() {
        errors.push(e);
    }
    if let Err(e) = runtime.stop() {
        errors.push(e);
    }
    if resources.network && runtime.snapshot().pid.is_none() {
        let result = (|| {
            network::recover(root)?;
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                let n = network::observe()?;
                state.lock().unwrap().network = n.clone();
                if !n.interface {
                    break;
                }
                if Instant::now() >= deadline {
                    return Err("TUN 接口未释放，不能启动第二个会话".into());
                }
                thread::sleep(Duration::from_millis(100));
            }
            network::forget(root)?;
            Ok::<_, String>(())
        })();
        if let Err(e) = result {
            errors.push(e);
        }
    }
    if errors.is_empty() {
        if let Some(directory) = resources.directory.clone() {
            if directory.starts_with(root.join("runtime")) {
                if let Err(e) = std::fs::remove_dir_all(directory) {
                    if e.kind() != std::io::ErrorKind::NotFound {
                        return Err(e.to_string());
                    }
                }
            } else {
                return Err("运行目录超出受保护路径".into());
            }
        }
        resources.directory = None;
        match std::fs::remove_file(root.join("runtime/current.json")) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.to_string()),
        }
        crate::profile::config::remove_generated(root)?;
        resources.network = false;
        Ok(())
    } else {
        Err(errors.join("；"))
    }
}
fn recorded_directory(root: &Path) -> Option<PathBuf> {
    let bytes = std::fs::read(root.join("runtime/current.json")).ok()?;
    let id: String = serde_json::from_slice(&bytes).ok()?;
    if id.len() != 64 || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    Some(root.join("runtime").join(id))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cleanup_failure_is_retained_and_retried() {
        let root = std::env::temp_dir().join(format!(
            "sing-shell-worker-{}-{}",
            std::process::id(),
            crate::logs::now()
        ));
        std::fs::create_dir_all(root.join("runtime/generated.json")).unwrap();
        let mut worker = Worker::new(root.clone(), "a".repeat(64));
        assert!(worker.stop().is_err());
        assert_eq!(worker.snapshot().phase, Phase::CleanupRequired);
        std::fs::remove_dir(root.join("runtime/generated.json")).unwrap();
        worker.stop().unwrap();
        assert_eq!(worker.snapshot().phase, Phase::Stopped);
        std::fs::remove_dir_all(root).unwrap();
    }
}
