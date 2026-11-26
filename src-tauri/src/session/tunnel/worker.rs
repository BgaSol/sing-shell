use super::{Connection, Failure, Phase, Snapshot};
use crate::{
    platform::network,
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

pub(super) struct Worker {
    pub(super) root: PathBuf,
    pub(super) runtime: Arc<RuntimeService>,
    pub(super) state: Arc<Mutex<Snapshot>>,
    pub(super) cancel: Arc<AtomicBool>,
    pub(super) start: Option<JoinHandle<()>>,
}
impl Worker {
    pub(super) fn snapshot(&self) -> Snapshot {
        let mut snapshot = self.state.lock().unwrap().clone();
        snapshot.runtime = self.runtime.snapshot();
        let logs = self.runtime.logs.snapshot();
        let mut size = 0;
        let mut bounded = Vec::new();
        for log in logs.into_iter().rev() {
            size += log.message.len();
            if size > 256 * 1024 {
                break;
            }
            bounded.push(log);
        }
        bounded.reverse();
        snapshot.logs = bounded;
        snapshot
    }
    pub(super) fn observe_exit(&mut self) {
        if self.state.lock().unwrap().phase == Phase::Running
            && self.runtime.snapshot().pid.is_none()
        {
            let error = self.runtime.snapshot().error;
            let result = self.stop();
            if result.is_ok() {
                let mut state = self.state.lock().unwrap();
                state.phase = Phase::Failed;
                state.error = error.or_else(|| Some("核心已退出，请手动重新连接".into()));
            }
        }
    }
    pub(super) fn start(&mut self, input: Connection, version: String) -> Result<(), String> {
        if !matches!(self.snapshot().phase.as_str(), "stopped" | "failed") {
            return Err("已有 TUN 会话或尚未完成清理".into());
        }
        if self.start.as_ref().is_some_and(|t| !t.is_finished()) {
            return Err("网络操作尚未收尾".into());
        }
        if let Some(t) = self.start.take() {
            let _ = t.join();
        }
        self.cancel = Arc::new(AtomicBool::new(false));
        let cancel = self.cancel.clone();
        let state = self.state.clone();
        let runtime = self.runtime.clone();
        let root = self.root.clone();
        *state.lock().unwrap() = Snapshot {
            phase: Phase::Preparing,
            ..Default::default()
        };
        self.start = Some(thread::spawn(move || {
            let result = (|| {
                runtime.logs.redact(input.node.spec.secrets());
                input.profile().validate()?;
                network::recover(&root)?;
                crate::rulesets::prepare(&root, &input.rule_commit, &cancel)?;
                let core = trusted_core(&root, &version, &runtime, &cancel)?;
                crate::platform::network::require_clear()?;
                let config = crate::profile::config::build(&input.profile(), &root)?;
                state.lock().unwrap().phase = Phase::Starting;
                state.lock().unwrap().failure = Some(Failure::CoreStart);
                runtime.start(
                    &StartSpec {
                        executable: core,
                        config: config.path,
                        version: Some(version),
                    },
                    cancel.clone(),
                )?;
                state.lock().unwrap().failure = Some(Failure::Network);
                let deadline = Instant::now() + Duration::from_secs(12);
                loop {
                    crate::core::package::cancelled(&cancel)?;
                    let observation = network::observe()?;
                    if observation.ready() {
                        let mut s = state.lock().unwrap();
                        s.network = observation;
                        s.phase = Phase::Running;
                        s.failure = None;
                        break;
                    }
                    if runtime.snapshot().pid.is_none() {
                        return Err("核心已退出，TUN 未建立".into());
                    }
                    if Instant::now() >= deadline {
                        return Err(format!(
                            "未能确认 TUN 状态：接口={}，IPv4={}，IPv6={}，路由={}",
                            observation.interface,
                            observation.ipv4,
                            observation.ipv6,
                            observation.routes
                        ));
                    }
                    thread::sleep(Duration::from_millis(100));
                }
                Ok::<_, String>(())
            })();
            if let Err(error) = result {
                let stopped = runtime.stop();
                let clear = network::require_clear();
                let mut s = state.lock().unwrap();
                s.phase = if stopped.is_ok() && clear.is_ok() {
                    Phase::Failed
                } else {
                    Phase::CleanupRequired
                };
                s.error = Some(error);
                if s.phase == Phase::CleanupRequired {
                    s.failure = Some(Failure::Cleanup);
                } else if s.failure.is_none() {
                    s.failure = Some(Failure::Preparation);
                }
                s.network = network::observe().unwrap_or_default();
            }
        }));
        Ok(())
    }
    pub(super) fn stop(&mut self) -> Result<(), String> {
        let result = self.stop_inner();
        if let Err(error) = &result {
            let mut state = self.state.lock().unwrap();
            state.phase = Phase::CleanupRequired;
            state.failure = Some(Failure::Cleanup);
            state.error = Some(error.clone());
        }
        result
    }
    fn stop_inner(&mut self) -> Result<(), String> {
        self.cancel.store(true, Ordering::Release);
        let deadline = Instant::now() + Duration::from_secs(30);
        while self.start.as_ref().is_some_and(|t| !t.is_finished()) {
            if Instant::now() >= deadline {
                return Err("网络准备任务尚未结束，请重试停止".into());
            }
            thread::sleep(Duration::from_millis(25));
        }
        if let Some(thread) = self.start.take() {
            let _ = thread.join();
        }
        self.state.lock().unwrap().phase = Phase::Stopping;
        self.runtime.stop()?;
        network::recover(&self.root)?;
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let observation = network::observe()?;
            if !observation.interface {
                network::forget(&self.root)?;
                crate::profile::config::remove_generated(&self.root)?;
                *self.state.lock().unwrap() = Snapshot::default();
                return Ok(());
            }
            if Instant::now() >= deadline {
                let mut s = self.state.lock().unwrap();
                s.phase = Phase::CleanupRequired;
                s.network = observation;
                return Err("TUN 接口仍未释放；不能启动第二套网络会话".into());
            }
            thread::sleep(Duration::from_millis(100));
        }
    }
}
fn trusted_core(
    root: &Path,
    version: &str,
    runtime: &RuntimeService,
    cancel: &Arc<AtomicBool>,
) -> Result<PathBuf, String> {
    let parsed = crate::settings::stable(version)?;
    if parsed < semver::Version::new(1, 14, 4) || parsed >= semver::Version::new(1, 15, 0) {
        return Err(
            "受控 TUN 当前需要 sing-box 1.14.4 至 1.14.x，未验证的新版本不会以管理员权限执行"
                .into(),
        );
    }
    let record = root.join(format!("trusted-{version}.json"));
    if let Ok(bytes) = std::fs::read(&record) {
        let reference: crate::settings::ManagedRef =
            serde_json::from_slice(&bytes).map_err(|_| "受保护核心记录损坏")?;
        if reference.version != version || reference.source.is_none() {
            return Err("受保护核心来源无效".into());
        }
        crate::core::package::verify(root, runtime, &reference, cancel)?;
        return crate::settings::managed_path(root, &reference);
    }
    let mut preferences = crate::updates::network::Preferences::default();
    preferences.fallback_enabled = false;
    let client =
        crate::updates::network::Transport::new(preferences, Arc::new(Default::default()))?
            .observed(Arc::new(|_| {}), {
                let cancel = cancel.clone();
                Arc::new(move || cancel.load(Ordering::Acquire))
            });
    let candidate = crate::updates::release::engine_version(&client, version)?;
    let archive = root.join(format!("official-{version}.archive"));
    client
        .download_result(
            &candidate.url,
            &archive,
            |_, _| {},
            |path, hash| {
                if hash != candidate.sha256
                    || std::fs::metadata(path)
                        .map_err(|e| crate::updates::network::Error::Local(e.to_string()))?
                        .len()
                        != candidate.size
                {
                    return Err(crate::updates::network::Error::remote(
                        "官方核心摘要或大小不符",
                    ));
                }
                Ok(())
            },
        )
        .map_err(|e| e.message())?;
    let reference = crate::core::package::prepare(root, runtime, &candidate, &archive, cancel)?;
    let staged = crate::core::package::staged_path(root, &reference)?;
    let installed = crate::settings::managed_path(root, &reference)?;
    std::fs::rename(staged, &installed).map_err(|e| e.to_string())?;
    crate::settings::write_json(&record, &reference)?;
    let _ = std::fs::remove_file(archive);
    Ok(installed)
}
