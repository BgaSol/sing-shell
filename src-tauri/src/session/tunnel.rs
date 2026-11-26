use crate::{
    logs::Log,
    platform::{authorization, network},
    profile::{routing::Routing, Node, Profile},
    runtime::{RuntimeService, RuntimeSnapshot, StartSpec},
};
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Connection {
    pub node: Node,
    pub routing: Routing,
    #[serde(default = "crate::rulesets::bundled_commit")]
    pub rule_commit: String,
}
impl Connection {
    pub fn profile(&self) -> Profile {
        Profile {
            mode: crate::profile::Mode::Generated,
            nodes: vec![self.node.clone()],
            selected_node: Some(self.node.id.clone()),
            routing: self.routing.clone(),
            rule_commit: self.rule_commit.clone(),
        }
    }
    pub fn fingerprint(&self) -> String {
        crate::rulesets::digest(
            &serde_json::to_vec(&(&self.node.spec, &self.routing, &self.rule_commit))
                .expect("connection serialization"),
        )
    }
}
#[derive(Deserialize, Serialize)]
#[serde(tag = "command", rename_all = "camelCase", deny_unknown_fields)]
enum Request {
    Hello {
        version: u32,
    },
    Start {
        input: Connection,
        core_version: String,
    },
    Poll,
    Stop,
    Quit,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub runtime: RuntimeSnapshot,
    pub network: network::Observation,
    pub phase: String,
    pub error: Option<String>,
    pub logs: Vec<Log>,
    pub failure: Option<Failure>,
}
#[derive(Clone, Copy, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Failure {
    Preparation,
    CoreStart,
    Network,
    Cleanup,
}
impl Default for Snapshot {
    fn default() -> Self {
        Self {
            runtime: Default::default(),
            network: Default::default(),
            phase: "stopped".into(),
            error: None,
            logs: vec![],
            failure: None,
        }
    }
}
async fn write(channel: &mut authorization::Channel, value: &impl Serialize) -> Result<(), String> {
    let bytes = serde_json::to_vec(value).map_err(|_| "网络控制消息无法编码")?;
    if bytes.len() > 2 * 1024 * 1024 {
        return Err("网络控制消息超限".into());
    }
    channel
        .write_u32(bytes.len() as u32)
        .await
        .map_err(|e| e.to_string())?;
    channel.write_all(&bytes).await.map_err(|e| e.to_string())?;
    channel.flush().await.map_err(|e| e.to_string())
}
async fn read<T: serde::de::DeserializeOwned>(
    channel: &mut authorization::Channel,
) -> Result<T, String> {
    let count = channel.read_u32().await.map_err(|e| e.to_string())? as usize;
    if count > 2 * 1024 * 1024 {
        return Err("网络控制消息超限".into());
    }
    let mut bytes = vec![0; count];
    channel
        .read_exact(&mut bytes)
        .await
        .map_err(|e| e.to_string())?;
    serde_json::from_slice(&bytes).map_err(|_| "网络控制消息格式无效".into())
}
async fn exchange(
    channel: &mut authorization::Channel,
    request: &Request,
) -> Result<Snapshot, String> {
    tokio::time::timeout(Duration::from_secs(20), async {
        write(channel, request).await?;
        read(channel).await
    })
    .await
    .map_err(|_| "授权进程未回应，请重试停止")?
}

struct Client {
    tx: mpsc::Sender<Request>,
    cancel: Arc<AtomicBool>,
    thread: JoinHandle<()>,
}
pub struct Tunnel {
    root: PathBuf,
    state: Arc<Mutex<Snapshot>>,
    client: Mutex<Option<Client>>,
}
impl Tunnel {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            state: Arc::new(Mutex::new(Snapshot::default())),
            client: Mutex::new(None),
        }
    }
    pub fn snapshot(&self) -> Snapshot {
        self.state.lock().unwrap().clone()
    }
    pub fn start(
        &self,
        input: Connection,
        core_version: String,
        cancel: &AtomicBool,
    ) -> Result<(), String> {
        input.profile().validate()?;
        let request = Request::Start {
            input,
            core_version,
        };
        let mut client = self.client.lock().unwrap();
        if client.as_ref().is_some_and(|c| c.thread.is_finished()) {
            client.take();
        }
        if let Some(client) = client.as_ref() {
            *self.state.lock().unwrap() = Snapshot {
                phase: "preparing".into(),
                ..Default::default()
            };
            client.tx.send(request).map_err(|_| "授权进程已断开")?;
        } else {
            let (tx, rx) = mpsc::channel();
            tx.send(request).map_err(|e| e.to_string())?;
            let root = self.root.clone();
            let state = self.state.clone();
            let actor_cancel = Arc::new(AtomicBool::new(false));
            let actor_signal = actor_cancel.clone();
            *state.lock().unwrap() = Snapshot {
                phase: "authorizing".into(),
                ..Default::default()
            };
            let worker = thread::spawn(move || {
                let result = (|| {
                    let runtime = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .map_err(|e| e.to_string())?;
                    runtime.block_on(async {
                        let mut channel = authorization::connect(&root, &actor_signal).await?;
                        let hello = exchange(&mut channel, &Request::Hello { version: 1 }).await?;
                        *state.lock().unwrap() = hello;
                        loop {
                            let request = match rx.try_recv() {
                                Ok(request) => request,
                                Err(mpsc::TryRecvError::Disconnected) => Request::Quit,
                                Err(mpsc::TryRecvError::Empty) => Request::Poll,
                            };
                            let quit = matches!(request, Request::Quit);
                            let snapshot = exchange(&mut channel, &request).await?;
                            *state.lock().unwrap() = snapshot;
                            if quit {
                                break;
                            }
                            tokio::time::sleep(Duration::from_millis(250)).await;
                        }
                        Ok::<_, String>(())
                    })
                })();
                if let Err(error) = result {
                    let mut s = state.lock().unwrap();
                    s.error = Some(error.clone());
                    s.runtime.error = Some(error);
                    s.phase = if s.runtime.pid.is_some() {
                        "cleanupRequired"
                    } else {
                        "failed"
                    }
                    .into();
                }
            });
            *client = Some(Client {
                tx,
                cancel: actor_cancel,
                thread: worker,
            });
        }
        drop(client);
        let deadline = Instant::now() + Duration::from_secs(180);
        loop {
            let snapshot = self.snapshot();
            if snapshot.phase == "running" && snapshot.network.ready() {
                return Ok(());
            }
            if matches!(snapshot.phase.as_str(), "failed" | "cleanupRequired") {
                return Err(snapshot.error.unwrap_or("TUN 启动失败".into()));
            }
            if cancel.load(Ordering::Acquire) {
                self.stop()?;
                return Err("TUN 启动已取消".into());
            }
            if Instant::now() >= deadline {
                self.stop()?;
                return Err("TUN 准备超时，请重试".into());
            }
            thread::sleep(Duration::from_millis(50));
        }
    }
    pub fn stop(&self) -> Result<(), String> {
        let mut guard = self.client.lock().unwrap();
        if guard.as_ref().is_some_and(|c| c.thread.is_finished())
            && self.snapshot().runtime.pid.is_none()
            && self.snapshot().phase != "cleanupRequired"
        {
            guard.take();
            *self.state.lock().unwrap() = Snapshot::default();
            return Ok(());
        }
        let Some(client) = guard.as_ref() else {
            return if self.snapshot().phase == "cleanupRequired" {
                Err("授权控制端已断开；请重新连接授权进程完成清理".into())
            } else {
                Ok(())
            };
        };
        if self.snapshot().phase == "authorizing" {
            client.cancel.store(true, Ordering::Release);
        }
        self.state.lock().unwrap().phase = "stopping".into();
        client
            .tx
            .send(Request::Stop)
            .map_err(|_| "授权进程已断开，请重试清理")?;
        let deadline = Instant::now() + Duration::from_secs(35);
        loop {
            let snapshot = self.snapshot();
            if snapshot.phase == "stopped" {
                return Ok(());
            }
            if snapshot.phase == "cleanupRequired" {
                return Err(snapshot.error.unwrap_or("网络资源尚未清理".into()));
            }
            if client.thread.is_finished() {
                guard.take();
                return if snapshot.runtime.pid.is_none() {
                    Ok(())
                } else {
                    Err("网络控制端已断开，仍需核对网络资源".into())
                };
            }
            if Instant::now() >= deadline {
                return Err("TUN 尚未完成停止，请重试".into());
            }
            thread::sleep(Duration::from_millis(50));
        }
    }
    pub fn close(&self) -> Result<(), String> {
        self.stop()?;
        if let Some(client) = self.client.lock().unwrap().take() {
            client.cancel.store(true, Ordering::Release);
            let _ = client.tx.send(Request::Quit);
            let deadline = Instant::now() + Duration::from_secs(25);
            while !client.thread.is_finished() {
                if Instant::now() >= deadline {
                    return Err("授权进程尚未结束".into());
                }
                thread::sleep(Duration::from_millis(50));
            }
            let _ = client.thread.join();
        }
        Ok(())
    }
}
impl Drop for Tunnel {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

struct Worker {
    root: PathBuf,
    runtime: Arc<RuntimeService>,
    state: Arc<Mutex<Snapshot>>,
    cancel: Arc<AtomicBool>,
    start: Option<JoinHandle<()>>,
}
impl Worker {
    fn snapshot(&self) -> Snapshot {
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
        if snapshot.phase == "running" && snapshot.runtime.pid.is_none() {
            snapshot.phase = "cleanupRequired".into();
            snapshot.error = snapshot.runtime.error.clone();
            *self.state.lock().unwrap() = snapshot.clone();
        }
        snapshot
    }
    fn start(&mut self, input: Connection, version: String) -> Result<(), String> {
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
            phase: "preparing".into(),
            ..Default::default()
        };
        self.start = Some(thread::spawn(move || {
            let result = (|| {
                runtime.logs.redact(input.node.spec.secrets());
                input.profile().validate()?;
                crate::rulesets::prepare(&root, &input.rule_commit, &cancel)?;
                let core = trusted_core(&root, &version, &runtime, &cancel)?;
                crate::platform::network::require_clear()?;
                let config = crate::profile::config::build(&input.profile(), &root)?;
                state.lock().unwrap().phase = "starting".into();
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
                        s.phase = "running".into();
                        s.failure = None;
                        break;
                    }
                    if runtime.snapshot().pid.is_none() {
                        return Err("核心已退出，TUN 未建立".into());
                    }
                    if Instant::now() >= deadline {
                        return Err("未能确认 TUN 接口和 IPv4/IPv6 路由".into());
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
                    "failed"
                } else {
                    "cleanupRequired"
                }
                .into();
                s.error = Some(error);
                if s.phase == "cleanupRequired" {
                    s.failure = Some(Failure::Cleanup);
                } else if s.failure.is_none() {
                    s.failure = Some(Failure::Preparation);
                }
                s.network = network::observe().unwrap_or_default();
            }
        }));
        Ok(())
    }
    fn stop(&mut self) -> Result<(), String> {
        self.cancel.store(true, Ordering::Release);
        if let Some(thread) = self.start.take() {
            let _ = thread.join();
        }
        self.state.lock().unwrap().phase = "stopping".into();
        self.runtime.stop()?;
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let observation = network::observe()?;
            if !observation.interface {
                *self.state.lock().unwrap() = Snapshot::default();
                return Ok(());
            }
            if Instant::now() >= deadline {
                let mut s = self.state.lock().unwrap();
                s.phase = "cleanupRequired".into();
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

pub fn dispatch(args: &[std::ffi::OsString]) -> Option<Result<(), String>> {
    let mode = args.get(1)?.to_str()?;
    if mode != "--network-worker" && mode != "--network-daemon" {
        return None;
    }
    Some((|| {
        if !authorization::elevated() {
            return Err("网络 helper 需要系统授权".into());
        }
        crate::private_files::protect_worker_writes();
        for (key, _) in std::env::vars_os() {
            let name = key.to_string_lossy().to_ascii_uppercase();
            if name.starts_with("DYLD_")
                || name.starts_with("LD_")
                || name.contains("TOKEN")
                || name.contains("SECRET")
                || name.contains("PASSWORD")
                || name.contains("PRIVATE_KEY")
            {
                std::env::remove_var(key);
            }
        }
        crate::platform::network_worker().map_err(|e| e.to_string())?;
        let root = authorization::prepare_root()?;
        let _lease = crate::instance::Lease::acquire(&root)?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| e.to_string())?;
        runtime.block_on(async {
            let mut channel = if mode == "--network-worker" {
                let endpoint = args.get(2).map(PathBuf::from).ok_or("缺少网络控制端点")?;
                let parent = args
                    .get(3)
                    .and_then(|s| s.to_str())
                    .and_then(|s| s.parse().ok())
                    .ok_or("缺少网络控制端身份")?;
                authorization::worker(&endpoint, parent).await?
            } else {
                #[cfg(target_os = "macos")]
                {
                    authorization::daemon().await?
                }
                #[cfg(not(target_os = "macos"))]
                {
                    return Err("当前系统不支持网络 daemon 入口".into());
                }
            };
            let mut worker = Worker {
                root,
                runtime: Arc::new(Default::default()),
                state: Arc::new(Mutex::new(Snapshot::default())),
                cancel: Arc::new(AtomicBool::new(false)),
                start: None,
            };
            let mut greeted = false;
            let result = loop {
                let request = match tokio::time::timeout(
                    Duration::from_secs(10),
                    read::<Request>(&mut channel),
                )
                .await
                {
                    Ok(Ok(request)) => request,
                    Ok(Err(error)) => break Err(error),
                    Err(_) => break Err("网络控制端已失去联系".into()),
                };
                let quit = matches!(request, Request::Quit);
                let outcome = match request {
                    Request::Hello { version: 1 } if !greeted => {
                        greeted = true;
                        Ok(())
                    }
                    _ if !greeted => Err("网络控制协议版本不符".into()),
                    Request::Start {
                        input,
                        core_version,
                    } => worker.start(input, core_version),
                    Request::Poll => {
                        if worker.snapshot().phase == "cleanupRequired" {
                            worker.stop()
                        } else {
                            Ok(())
                        }
                    }
                    Request::Stop | Request::Quit => worker.stop(),
                    Request::Hello { .. } => Err("网络控制协议重复初始化".into()),
                };
                if let Err(error) = outcome {
                    let mut state = worker.state.lock().unwrap();
                    state.error = Some(error);
                    if worker.runtime.snapshot().pid.is_none() && state.phase != "cleanupRequired" {
                        state.phase = "failed".into();
                    }
                }
                if let Err(error) = write(&mut channel, &worker.snapshot()).await {
                    break Err(error);
                }
                if quit {
                    break Ok(());
                }
            };
            let cleaned = worker.stop();
            cleaned?;
            result
        })
    })())
}

#[cfg(feature = "test-fixtures")]
pub fn fixture(args: &[std::ffi::OsString]) -> Result<(), String> {
    if !authorization::elevated() {
        return Err("network fixture needs a disposable privileged runner".into());
    }
    let file = args
        .get(1)
        .map(PathBuf::from)
        .ok_or("fixture input file missing")?;
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    struct Input {
        node: Node,
        routing: Routing,
        core_version: String,
        control: PathBuf,
    }
    let input: Input = serde_json::from_slice(&std::fs::read(file).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    crate::private_files::directory(&input.control)?;
    let tunnel = Tunnel::new(input.control.clone());
    let cancel = AtomicBool::new(false);
    tunnel.start(
        Connection {
            node: input.node,
            routing: input.routing,
            rule_commit: crate::rulesets::bundled_commit(),
        },
        input.core_version,
        &cancel,
    )?;
    crate::settings::write_json(&input.control.join("started.json"), &tunnel.snapshot())?;
    let deadline = Instant::now() + Duration::from_secs(60);
    while !input.control.join("stop").exists() {
        if Instant::now() >= deadline {
            tunnel.close()?;
            return Err("fixture controller timed out".into());
        }
        thread::sleep(Duration::from_millis(50));
    }
    tunnel.close()?;
    crate::settings::write_json(&input.control.join("stopped.json"), &tunnel.snapshot())?;
    Ok(())
}
