use crate::{
    platform::{authorization, network},
    profile::{routing::Routing, Node, Profile},
};
use serde::{Deserialize, Serialize};
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
use tokio::io::AsyncReadExt;
mod protocol;
mod worker;
use protocol::{exchange, read, write, Closed, Request, DISCONNECT_BUDGET, STOP_BUDGET};
pub use protocol::{Failure, Phase, Snapshot};
use worker::Worker;

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

struct Client {
    tx: mpsc::Sender<Request>,
    cancel: Arc<AtomicBool>,
    thread: JoinHandle<()>,
    disconnect: Arc<AtomicBool>,
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
    pub fn status(&self) -> Snapshot {
        let state = self.state.lock().unwrap();
        Snapshot {
            runtime: state.runtime.clone(),
            network: state.network.clone(),
            phase: state.phase.clone(),
            error: state.error.clone(),
            failure: state.failure,
            authorization_stage: state.authorization_stage,
            logs: vec![],
        }
    }
    fn request(&self, request: Request) -> Result<(), String> {
        let mut client = self.client.lock().unwrap();
        if client.as_ref().is_some_and(|c| c.thread.is_finished()) {
            let _ = client.take().unwrap().thread.join();
        }
        if let Some(client) = client.as_ref() {
            if client.disconnect.load(Ordering::Acquire) {
                return Err("上一授权会话仍在结束，请重试停止并等待收尾".into());
            }
            self.state.lock().unwrap().phase = Phase::Preparing;
            client.tx.send(request).map_err(|_| "授权进程已断开")?;
        } else {
            let (tx, rx) = mpsc::channel();
            tx.send(request).map_err(|e| e.to_string())?;
            let root = self.root.clone();
            let state = self.state.clone();
            let actor_cancel = Arc::new(AtomicBool::new(false));
            let actor_signal = actor_cancel.clone();
            let disconnect = Arc::new(AtomicBool::new(false));
            let actor_disconnect = disconnect.clone();
            *state.lock().unwrap() = Snapshot {
                phase: Phase::Authorizing,
                ..Default::default()
            };
            let worker = thread::spawn(move || {
                let result = (|| {
                    let runtime = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .map_err(|e| e.to_string())?;
                    runtime.block_on(async {
                        let mut connection =
                            authorization::connect(&root, &actor_signal, &|stage| {
                                state.lock().unwrap().authorization_stage = Some(stage);
                            })
                            .await?;
                        let result = async {
                            let channel = &mut connection.channel;
                            loop {
                                let request = if actor_disconnect.load(Ordering::Acquire) {
                                    Request::Quit
                                } else {
                                    match rx.try_recv() {
                                        Ok(request) => request,
                                        Err(mpsc::TryRecvError::Disconnected) => Request::Quit,
                                        Err(mpsc::TryRecvError::Empty) => Request::Poll,
                                    }
                                };
                                if matches!(request, Request::Start { .. })
                                    && actor_signal.load(Ordering::Acquire)
                                {
                                    return Err("TUN 启动已取消".into());
                                }
                                let quit = matches!(request, Request::Quit);
                                let snapshot = exchange(channel, &request).await?;
                                *state.lock().unwrap() = snapshot;
                                if quit {
                                    let closed: Closed = tokio::time::timeout(
                                        Duration::from_secs(10),
                                        read(channel),
                                    )
                                    .await
                                    .map_err(|_| "授权进程尚未释放资源")??;
                                    if !closed.closed {
                                        return Err("授权进程结束回应无效".into());
                                    }
                                    let mut end = [0; 1];
                                    if tokio::time::timeout(
                                        Duration::from_secs(10),
                                        channel.read(&mut end),
                                    )
                                    .await
                                    .map_err(|_| "授权进程尚未释放控制通道")?
                                    .map_err(|e| e.to_string())?
                                        != 0
                                    {
                                        return Err("授权进程退出回应无效".into());
                                    }
                                    break;
                                }
                                tokio::time::sleep(Duration::from_millis(250)).await;
                            }
                            Ok::<_, String>(())
                        }
                        .await;
                        if let Err(error) = &result {
                            let mut snapshot = state.lock().unwrap();
                            snapshot.phase = Phase::CleanupRequired;
                            snapshot.error =
                                Some(format!("控制通道已结束，正在等待 helper 清理：{error}"));
                        }
                        let finished = connection.finish().await;
                        if result.is_ok() && finished.is_ok() {
                            *state.lock().unwrap() = Snapshot::default();
                        }
                        result.and(finished)
                    })
                })();
                if let Err(error) = result {
                    let mut s = state.lock().unwrap();
                    s.error = Some(error.clone());
                    s.runtime.error = Some(error);
                    s.phase = if s.runtime.pid.is_some() || network::require_clear().is_err() {
                        Phase::CleanupRequired
                    } else {
                        Phase::Failed
                    };
                }
            });
            *client = Some(Client {
                tx,
                cancel: actor_cancel,
                thread: worker,
                disconnect,
            });
        }
        Ok(())
    }
    pub fn start(
        &self,
        input: Connection,
        core_version: String,
        cancel: &AtomicBool,
    ) -> Result<(), String> {
        if self.snapshot().phase == Phase::CleanupRequired {
            return Err("网络资源尚未清理，请先停止连接".into());
        }
        input.profile().validate()?;
        self.request(Request::Start {
            input,
            core_version,
        })?;
        let deadline = Instant::now() + Duration::from_secs(180);
        loop {
            let snapshot = self.snapshot();
            if cancel.load(Ordering::Acquire) {
                if let Some(client) = self.client.lock().unwrap().as_ref() {
                    client.cancel.store(true, Ordering::Release);
                    client.disconnect.store(true, Ordering::Release);
                }
                self.wait_closed(DISCONNECT_BUDGET)?;
                return Err("TUN 启动已取消".into());
            }
            if snapshot.phase == Phase::Running && snapshot.network.ready() {
                return Ok(());
            }
            if matches!(snapshot.phase.as_str(), "failed" | "cleanupRequired") {
                return Err(snapshot.error.unwrap_or("TUN 启动失败".into()));
            }
            if Instant::now() >= deadline {
                self.stop()?;
                return Err("TUN 准备超时，请重试".into());
            }
            thread::sleep(Duration::from_millis(50));
        }
    }
    pub fn stop(&self) -> Result<(), String> {
        {
            let mut guard = self.client.lock().unwrap();
            if guard.as_ref().is_some_and(|c| c.thread.is_finished()) {
                let _ = guard.take().unwrap().thread.join();
                if network::require_clear().is_ok() {
                    *self.state.lock().unwrap() = Snapshot::default();
                    return Ok(());
                }
            }
            if guard.is_none() && self.snapshot().phase != Phase::CleanupRequired {
                return Ok(());
            }
        }
        // A lost controller reconnects only to clean up; it never restarts the old connection.
        let reconnecting = self.client.lock().unwrap().is_none();
        if reconnecting {
            self.request(Request::Stop)?;
        }
        let guard = self.client.lock().unwrap();
        if guard.as_ref().is_some_and(|c| c.thread.is_finished())
            && self.snapshot().runtime.pid.is_none()
            && self.snapshot().phase != Phase::CleanupRequired
        {
            drop(guard);
            self.reap();
            *self.state.lock().unwrap() = Snapshot::default();
            return Ok(());
        }
        let Some(client) = guard.as_ref() else {
            return if self.snapshot().phase == Phase::CleanupRequired {
                Err("授权控制端已断开；请重新连接授权进程完成清理".into())
            } else {
                Ok(())
            };
        };
        let tx = client.tx.clone();
        let cancel = client.cancel.clone();
        drop(guard);
        if !reconnecting && self.snapshot().phase == Phase::Authorizing {
            cancel.store(true, Ordering::Release);
        }
        self.state.lock().unwrap().phase = Phase::Stopping;
        if tx.send(Request::Stop).is_err() {
            self.state.lock().unwrap().phase = Phase::CleanupRequired;
            return Err("授权进程已断开，请重试清理".into());
        }
        let deadline = Instant::now() + STOP_BUDGET;
        loop {
            let snapshot = self.snapshot();
            if snapshot.phase == Phase::Stopped {
                return Ok(());
            }
            if snapshot.phase == Phase::CleanupRequired {
                return Err(snapshot.error.unwrap_or("网络资源尚未清理".into()));
            }
            if self.reap() {
                return if snapshot.runtime.pid.is_none() && network::require_clear().is_ok() {
                    Ok(())
                } else {
                    Err("网络控制端已断开，仍需核对网络资源".into())
                };
            }
            if Instant::now() >= deadline {
                self.state.lock().unwrap().phase = Phase::CleanupRequired;
                return Err("TUN 尚未完成停止，请重试".into());
            }
            thread::sleep(Duration::from_millis(50));
        }
    }
    pub fn close(&self) -> Result<(), String> {
        if self.client.lock().unwrap().is_none() && self.status().phase == Phase::Stopped {
            return Ok(());
        }
        let already_closing = self
            .client
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|c| c.disconnect.load(Ordering::Acquire));
        if !already_closing {
            self.stop()?;
        }
        if let Some(client) = self.client.lock().unwrap().as_ref() {
            client.disconnect.store(true, Ordering::Release);
        }
        self.wait_closed(DISCONNECT_BUDGET)?;
        let snapshot = self.snapshot();
        if snapshot.phase == Phase::CleanupRequired {
            return Err(snapshot.error.unwrap_or("网络清理尚未完成".into()));
        }
        network::require_clear()
    }
    fn wait_closed(&self, budget: Duration) -> Result<(), String> {
        let deadline = Instant::now() + budget;
        while self.client.lock().unwrap().is_some() {
            if self.reap() {
                break;
            }
            if Instant::now() >= deadline {
                let mut state = self.state.lock().unwrap();
                state.phase = Phase::CleanupRequired;
                state.error = Some("授权进程尚未结束，请重试停止并等待清理".into());
                return Err("授权进程尚未结束".into());
            }
            thread::sleep(Duration::from_millis(50));
        }
        Ok(())
    }
    fn reap(&self) -> bool {
        let mut client = self.client.lock().unwrap();
        if client.as_ref().is_some_and(|c| c.thread.is_finished()) {
            let _ = client.take().unwrap().thread.join();
            true
        } else {
            client.is_none()
        }
    }
}
impl Drop for Tunnel {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn close_timeout_preserves_owner_and_retry_joins_the_same_thread() {
        let tunnel = Tunnel::new(std::env::temp_dir());
        let (release, wait) = mpsc::channel();
        let (tx, _rx) = mpsc::channel();
        *tunnel.client.lock().unwrap() = Some(Client {
            tx,
            cancel: Arc::new(AtomicBool::new(false)),
            disconnect: Arc::new(AtomicBool::new(true)),
            thread: thread::spawn(move || {
                wait.recv().unwrap();
            }),
        });
        assert!(tunnel.wait_closed(Duration::from_millis(10)).is_err());
        assert!(tunnel.client.lock().unwrap().is_some());
        release.send(()).unwrap();
        tunnel.wait_closed(Duration::from_secs(1)).unwrap();
        assert!(tunnel.client.lock().unwrap().is_none());
        *tunnel.state.lock().unwrap() = Snapshot::default();
    }
    #[test]
    fn reading_worker_status_does_not_rewrite_or_clean_resources() {
        let worker = Worker {
            root: std::env::temp_dir(),
            runtime: Arc::new(Default::default()),
            state: Arc::new(Mutex::new(Snapshot {
                phase: Phase::Running,
                ..Default::default()
            })),
            cancel: Arc::new(AtomicBool::new(false)),
            start: None,
        };
        assert_eq!(worker.snapshot().phase, Phase::Running);
        assert!(!worker.cancel.load(Ordering::Acquire));
        assert_eq!(worker.state.lock().unwrap().phase, Phase::Running);
    }
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
                || [
                    "HTTP_PROXY",
                    "HTTPS_PROXY",
                    "ALL_PROXY",
                    "NO_PROXY",
                    "SSL_CERT_FILE",
                    "SSL_CERT_DIR",
                    "CURL_CA_BUNDLE",
                    "REQUESTS_CA_BUNDLE",
                ]
                .contains(&name.as_str())
                || name.contains("TOKEN")
                || name.contains("SECRET")
                || name.contains("PASSWORD")
                || name.contains("PRIVATE_KEY")
            {
                std::env::remove_var(key);
            }
        }
        crate::platform::network_worker().map_err(|e| e.to_string())?;
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
            let root = authorization::prepare_root()?;
            let lease = crate::instance::Lease::acquire(&root)?;
            let mut worker = Worker {
                root,
                runtime: Arc::new(Default::default()),
                state: Arc::new(Mutex::new(Snapshot::default())),
                cancel: Arc::new(AtomicBool::new(false)),
                start: None,
            };
            worker.runtime.record_network(worker.root.clone());
            let result = loop {
                worker.observe_exit();
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
                    Request::Start {
                        input,
                        core_version,
                    } => worker.start(input, core_version),
                    Request::Poll => Ok(()),
                    Request::Stop | Request::Quit => worker.stop(),
                };
                if let Err(error) = outcome {
                    let mut state = worker.state.lock().unwrap();
                    state.error = Some(error);
                    if worker.runtime.snapshot().pid.is_none()
                        && state.phase != Phase::CleanupRequired
                    {
                        state.phase = Phase::Failed;
                    }
                }
                if let Err(error) = tokio::time::timeout(
                    Duration::from_secs(10),
                    write(&mut channel, &worker.snapshot()),
                )
                .await
                .map_err(|_| "网络控制端未读取状态".to_string())
                .and_then(|r| r)
                {
                    break Err(error);
                }
                if quit && worker.snapshot().phase == Phase::Stopped {
                    drop(lease);
                    tokio::time::timeout(
                        Duration::from_secs(10),
                        write(&mut channel, &Closed { closed: true }),
                    )
                    .await
                    .map_err(|_| "网络控制端未读取结束结果")??;
                    return Ok(());
                }
            };
            let cleaned = worker.finish();
            cleaned?;
            result
        })
    })())
}

#[cfg(target_os = "macos")]
pub fn authorization_probe() -> Result<(), String> {
    if authorization::elevated() {
        return Err("授权探针须以普通用户运行，不能用 root 入口替代 SMAppService".into());
    }
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    runtime.block_on(async {
        let mut connection =
            authorization::connect(&PathBuf::new(), &AtomicBool::new(false), &|stage| {
                println!("{}", serde_json::json!({"authorizationStage": stage}))
            })
            .await?;
        let snapshot = exchange(&mut connection.channel, &Request::Quit).await?;
        if snapshot.runtime.pid.is_some() || snapshot.phase != Phase::Stopped {
            return Err("授权探针未能确认空闲 helper 退出".into());
        }
        tokio::time::timeout(Duration::from_secs(20), connection.finish())
            .await
            .map_err(|_| "授权探针未确认 helper 退出")??;
        println!(
            "{}",
            serde_json::json!({"authenticated":true,"helperExited":true,"coreStarted":false})
        );
        Ok(())
    })
}

pub fn fixture(args: &[std::ffi::OsString]) -> Result<(), String> {
    let approved_mac = cfg!(target_os = "macos")
        && std::env::var("SING_SHELL_NETWORK_PREFLIGHT").as_deref() == Ok("1");
    if !authorization::elevated() && !approved_mac {
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
