use super::{Connection, Failure, Phase, Snapshot};
use crate::{
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
}
struct Started {
    resources: Resources,
    result: Result<network::Observation, (Failure, String)>,
}
pub(super) struct Worker {
    pub(super) root: PathBuf,
    pub(super) runtime: Arc<RuntimeService>,
    pub(super) state: Arc<Mutex<Snapshot>>,
    pub(super) cancel: Arc<AtomicBool>,
    start: Option<JoinHandle<Started>>,
    resources: Resources,
    observed: Instant,
}
impl Worker {
    pub(super) fn new(root: PathBuf) -> Self {
        let resources = Resources {
            network: root.join("network-process.json").exists(),
            ..Default::default()
        };
        Self {
            root,
            runtime: Arc::new(Default::default()),
            state: Arc::new(Mutex::new(Snapshot::default())),
            cancel: Arc::new(AtomicBool::new(false)),
            start: None,
            resources,
            observed: Instant::now(),
        }
    }
    pub(super) fn snapshot(&self) -> Snapshot {
        let mut snapshot = self.state.lock().unwrap().clone();
        snapshot.runtime = self.runtime.snapshot();
        let mut size = 0;
        let mut logs = Vec::new();
        for log in self.runtime.logs.snapshot().into_iter().rev() {
            size += log.message.len();
            if size > 256 * 1024 {
                break;
            }
            logs.push(log);
        }
        logs.reverse();
        snapshot.logs = logs;
        snapshot
    }
    fn collect(&mut self) -> Option<Result<network::Observation, (Failure, String)>> {
        if !self.start.as_ref().is_some_and(|task| task.is_finished()) {
            return None;
        }
        let started = self.start.take().unwrap().join();
        Some(match started {
            Ok(started) => {
                self.resources = started.resources;
                started.result
            }
            Err(_) => {
                self.resources.network = true;
                Err((Failure::Preparation, "网络启动任务异常结束".into()))
            }
        })
    }
    pub(super) fn observe_exit(&mut self) {
        if let Some(result) = self.collect() {
            match result {
                Ok(network) if !self.cancel.load(Ordering::Acquire) => {
                    let mut state = self.state.lock().unwrap();
                    state.network = network;
                    state.phase = Phase::Running;
                    state.failure = None;
                }
                Ok(_) => self.fail(Failure::Preparation, "TUN 启动已取消".into()),
                Err((failure, error)) => self.fail(failure, error),
            }
        }
        if self.state.lock().unwrap().phase != Phase::Running {
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
            if let Err(error) = self.resources.dns.check() {
                self.fail(Failure::Network, error);
            }
        }
    }
    fn fail(&mut self, failure: Failure, error: String) {
        let cleanup = self.cleanup();
        let mut state = self.state.lock().unwrap();
        state.phase = if cleanup.is_ok() && failure != Failure::Cleanup {
            Phase::Failed
        } else {
            Phase::CleanupRequired
        };
        state.failure = Some(if cleanup.is_ok() {
            failure
        } else {
            Failure::Cleanup
        });
        state.error = Some(match cleanup {
            Ok(()) => error,
            Err(cleanup) => format!("{error}；清理失败：{cleanup}"),
        });
    }
    pub(super) fn start(&mut self, input: Connection, version: String) -> Result<(), String> {
        if !matches!(self.snapshot().phase, Phase::Stopped | Phase::Failed) || self.start.is_some()
        {
            return Err("已有 TUN 会话或尚未完成清理".into());
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
            let mut resources = Resources::default();
            let mut failure = Failure::Preparation;
            let result = (|| {
                runtime.logs.redact(input.node.spec.secrets());
                input.profile().validate()?;
                failure = Failure::Cleanup;
                resources.network = true;
                network::recover(&root)?;
                network::require_clear()?;
                failure = Failure::Preparation;
                crate::rulesets::prepare(&root, &input.rule_commit, &cancel)?;
                let core = trusted_core(&root, &version, &runtime, &cancel)?;
                let config = crate::profile::config::build(&input.profile(), &root)?;
                state.lock().unwrap().phase = Phase::Starting;
                failure = Failure::CoreStart;
                runtime.start(
                    &StartSpec {
                        executable: core,
                        config: config.path,
                        version: Some(version),
                    },
                    cancel.clone(),
                )?;
                failure = Failure::Network;
                let deadline = Instant::now() + Duration::from_secs(12);
                let observation = loop {
                    crate::core::package::cancelled(&cancel)?;
                    if runtime.snapshot().pid.is_none() {
                        return Err("核心已退出，TUN 未建立".into());
                    }
                    let observation = network::observe()?;
                    if observation.ready() {
                        break observation;
                    }
                    if Instant::now() >= deadline {
                        return Err("未能确认 TUN 接口、双栈地址及路由覆盖".into());
                    }
                    thread::sleep(Duration::from_millis(100));
                };
                resources.dns.install()?;
                crate::core::package::cancelled(&cancel)?;
                if runtime.snapshot().pid.is_none() {
                    return Err("核心已退出，DNS 接管未完成".into());
                }
                Ok(observation)
            })();
            Started {
                resources,
                result: result.map_err(|error| (failure, error)),
            }
        }));
        Ok(())
    }
    pub(super) fn stop(&mut self) -> Result<(), String> {
        self.cancel.store(true, Ordering::Release);
        self.state.lock().unwrap().phase = Phase::Stopping;
        let result = (|| {
            let deadline = Instant::now() + Duration::from_secs(30);
            while self.start.as_ref().is_some_and(|task| !task.is_finished()) {
                if Instant::now() >= deadline {
                    return Err("网络准备任务尚未结束，请重试停止".into());
                }
                thread::sleep(Duration::from_millis(25));
            }
            self.collect();
            self.cleanup()
        })();
        match &result {
            Ok(()) => *self.state.lock().unwrap() = Snapshot::default(),
            Err(error) => {
                let mut state = self.state.lock().unwrap();
                state.phase = Phase::CleanupRequired;
                state.failure = Some(Failure::Cleanup);
                state.error = Some(error.clone());
            }
        }
        result
    }
    pub(super) fn finish(&mut self) -> Result<(), String> {
        self.cancel.store(true, Ordering::Release);
        while self.start.as_ref().is_some_and(|task| !task.is_finished()) {
            thread::sleep(Duration::from_millis(25));
        }
        self.stop()
    }
    fn cleanup(&mut self) -> Result<(), String> {
        let mut errors = Vec::new();
        if let Err(error) = self.resources.dns.close() {
            errors.push(error);
        }
        if let Err(error) = self.runtime.stop() {
            errors.push(error);
        }
        if self.resources.network && self.runtime.snapshot().pid.is_none() {
            let result = (|| {
                network::recover(&self.root)?;
                let deadline = Instant::now() + Duration::from_secs(5);
                loop {
                    let observation = network::observe()?;
                    self.state.lock().unwrap().network = observation.clone();
                    if !observation.interface {
                        break;
                    }
                    if Instant::now() >= deadline {
                        return Err("TUN 接口仍未释放；不能启动第二套网络会话".into());
                    }
                    thread::sleep(Duration::from_millis(100));
                }
                crate::profile::config::remove_generated(&self.root)?;
                network::forget(&self.root)?;
                Ok::<_, String>(())
            })();
            if let Err(error) = result {
                errors.push(error);
            }
        }
        if errors.is_empty() {
            crate::profile::config::remove_generated(&self.root)?;
            self.resources.network = false;
            Ok(())
        } else {
            Err(errors.join("；"))
        }
    }
}
fn trusted_core(
    root: &Path,
    version: &str,
    runtime: &RuntimeService,
    cancel: &Arc<AtomicBool>,
) -> Result<PathBuf, String> {
    crate::profile::tun::require_version(version)?;
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

#[cfg(test)]
mod tests {
    use super::*;
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    fn root() -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "sing-shell-worker-{}-{}-{}",
            std::process::id(),
            crate::logs::now(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&root).unwrap();
        root
    }
    #[test]
    fn failed_start_and_cancellation_share_cleanup_without_publishing_running() {
        let root = root();
        let mut worker = Worker::new(root.clone());
        std::fs::create_dir(root.join("runtime")).unwrap();
        std::fs::write(root.join("runtime/generated.json"), "credential").unwrap();
        worker.start = Some(thread::spawn(|| Started {
            resources: Resources::default(),
            result: Err((Failure::CoreStart, "test failure".into())),
        }));
        while !worker.start.as_ref().unwrap().is_finished() {
            thread::yield_now();
        }
        worker.observe_exit();
        assert_eq!(worker.snapshot().phase, Phase::Failed);
        assert!(!root.join("runtime/generated.json").exists());
        worker.cancel.store(true, Ordering::Release);
        worker.start = Some(thread::spawn(|| Started {
            resources: Resources::default(),
            result: Ok(Default::default()),
        }));
        while !worker.start.as_ref().unwrap().is_finished() {
            thread::yield_now();
        }
        worker.observe_exit();
        assert_eq!(worker.snapshot().phase, Phase::Failed);
        assert!(worker.snapshot().error.unwrap().contains("取消"));
        std::fs::remove_dir(root.join("runtime")).unwrap();
        std::fs::remove_dir(root).unwrap();
    }
    #[test]
    fn cleanup_failure_retains_state_and_retries_the_same_resources() {
        let root = root();
        let mut worker = Worker::new(root.clone());
        std::fs::create_dir_all(root.join("runtime/generated.json")).unwrap();
        assert!(worker.stop().is_err());
        assert_eq!(worker.snapshot().phase, Phase::CleanupRequired);
        std::fs::remove_dir(root.join("runtime/generated.json")).unwrap();
        worker.stop().unwrap();
        assert_eq!(worker.snapshot().phase, Phase::Stopped);
        std::fs::remove_dir(root.join("runtime")).unwrap();
        std::fs::remove_dir(root).unwrap();
    }
}
