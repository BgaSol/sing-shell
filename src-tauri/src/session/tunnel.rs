use crate::{
    platform::{authorization, network},
    profile::{routing::Routing, ConnectionInput, Node},
};
use serde::Deserialize;
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc, Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
use tokio::io::AsyncReadExt;
mod cache;
mod install;
mod protocol;
mod worker;
use protocol::{exchange, read, write, Closed, Request, DISCONNECT_BUDGET, STOP_BUDGET};
pub use protocol::{Failure, Phase, Snapshot};
use worker::Worker;

struct Command {
    request: Request,
    reply: Option<mpsc::Sender<Snapshot>>,
}
struct Client {
    tx: mpsc::SyncSender<Command>,
    cancel: Arc<AtomicBool>,
    thread: JoinHandle<()>,
    disconnect: Arc<AtomicBool>,
}

pub struct Tunnel {
    root: PathBuf,
    occupation: Arc<AtomicBool>,
    state: Arc<Mutex<Snapshot>>,
    client: Mutex<Option<Client>>,
    next: AtomicU64,
    history: Arc<Mutex<Vec<crate::logs::Log>>>,
}
impl Tunnel {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            occupation: Arc::new(AtomicBool::new(false)),
            state: Arc::new(Mutex::new(Snapshot::default())),
            client: Mutex::new(None),
            next: AtomicU64::new(0),
            history: Arc::new(Mutex::new(Vec::new())),
        }
    }
    pub(crate) fn with_occupation(root: PathBuf, occupation: Arc<AtomicBool>) -> Self {
        let mut tunnel = Self::new(root);
        tunnel.occupation = occupation;
        tunnel
    }
    fn released(&self) {
        self.occupation.store(false, Ordering::Release);
    }
    pub(crate) fn occupied(&self) -> bool {
        self.occupation.load(Ordering::Acquire)
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
            session_id: state.session_id.clone(),
            running_node: state.running_node.clone(),
            running_connection_options: state.running_connection_options,
            operation_id: state.operation_id,
            operation: state.operation.clone(),
            operation_error: state.operation_error.clone(),
            activity_summary: state.activity_summary.clone(),
            activity_page: None,
            core_received: state.core_received,
        }
    }
    fn request(&self, request: Request) -> Result<(), String> {
        self.occupation.store(true, Ordering::Release);
        let mut client = self.client.lock().unwrap();
        if client.as_ref().is_some_and(|c| c.thread.is_finished()) {
            let _ = client.take().unwrap().thread.join();
        }
        if let Some(client) = client.as_ref() {
            if client.disconnect.load(Ordering::Acquire) {
                return Err("上一授权会话仍在结束，请重试停止并等待收尾".into());
            }
            {
                let mut s = self.state.lock().unwrap();
                if s.phase != Phase::Running {
                    s.phase = Phase::Preparing;
                }
            }
            client
                .tx
                .try_send(Command {
                    request,
                    reply: None,
                })
                .map_err(|_| "授权进程已断开")?;
        } else {
            let (tx, rx) = mpsc::sync_channel(8);
            tx.try_send(Command {
                request,
                reply: None,
            })
            .map_err(|e| e.to_string())?;
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
            self.history.lock().unwrap().clear();
            let history = self.history.clone();
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
                                let command = if actor_disconnect.load(Ordering::Acquire) {
                                    Command {
                                        request: Request::Quit,
                                        reply: None,
                                    }
                                } else {
                                    match rx.try_recv() {
                                        Ok(c) => c,
                                        Err(mpsc::TryRecvError::Disconnected) => Command {
                                            request: Request::Quit,
                                            reply: None,
                                        },
                                        Err(mpsc::TryRecvError::Empty) => Command {
                                            request: Request::Poll,
                                            reply: None,
                                        },
                                    }
                                };
                                let request = command.request;
                                if matches!(request, Request::Start { .. })
                                    && actor_signal.load(Ordering::Acquire)
                                {
                                    return Err("连接已取消".into());
                                }
                                if let Request::CoreBegin {
                                    version,
                                    material,
                                    operation_id,
                                } = &request
                                {
                                    let result = install::provision(
                                        channel,
                                        &root,
                                        version,
                                        material.clone(),
                                        &actor_signal,
                                        &state,
                                        *operation_id,
                                    )
                                    .await;
                                    let mut snapshot = state.lock().unwrap();
                                    snapshot.operation_id = *operation_id;
                                    snapshot.operation = None;
                                    snapshot.operation_error = result.as_ref().err().cloned();
                                    snapshot.error = result.err();
                                    if snapshot.error.is_some() {
                                        snapshot.failure = Some(Failure::Preparation);
                                        if snapshot.runtime.pid.is_none() {
                                            snapshot.phase = Phase::Failed;
                                        }
                                    }
                                    continue;
                                }
                                let quit = matches!(request, Request::Quit);
                                let snapshot = exchange(channel, &request).await?;
                                if !snapshot.logs.is_empty() {
                                    let mut logs = history.lock().unwrap();
                                    for l in &snapshot.logs {
                                        if !logs.iter().any(|old| old.sequence == l.sequence) {
                                            logs.push(l.clone());
                                        }
                                    }
                                    logs.sort_by_key(|l| l.sequence);
                                    let excess = logs.len().saturating_sub(1000);
                                    logs.drain(..excess);
                                }
                                if let Some(reply) = command.reply {
                                    let _ = reply.send(snapshot);
                                } else {
                                    *state.lock().unwrap() = snapshot;
                                }
                                if quit {
                                    confirm_closed(channel).await?;
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
                    s.phase = if s.phase == Phase::CleanupRequired
                        || s.runtime.pid.is_some()
                        || network::require_clear().is_err()
                    {
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
    pub fn prepare_core(&self, version: &str, cancel: &AtomicBool) -> Result<(), String> {
        let operation_id = self.next.fetch_add(1, Ordering::AcqRel) + 1;
        self.request(Request::CoreBegin {
            version: version.into(),
            material: install::material_for(&self.root, version),
            operation_id,
        })?;
        let deadline = Instant::now() + Duration::from_secs(150);
        loop {
            if cancel.load(Ordering::Acquire) {
                self.stop()?;
                return Err("核心安装已取消".into());
            }
            let snapshot = self.snapshot();
            if snapshot.operation_id == operation_id && snapshot.operation.is_none() {
                return snapshot
                    .operation_error
                    .or(snapshot.error)
                    .map_or(Ok(()), Err);
            }
            if snapshot.phase == Phase::CleanupRequired
                || (snapshot.phase == Phase::Failed && snapshot.operation_id != operation_id)
            {
                return Err(snapshot.error.unwrap_or("核心准备失败".into()));
            }
            if Instant::now() >= deadline {
                self.stop()?;
                return Err("核心安装超时".into());
            }
            thread::sleep(Duration::from_millis(50));
        }
    }
    pub fn start(
        &self,
        input: ConnectionInput,
        core_version: String,
        cancel: &AtomicBool,
    ) -> Result<(), String> {
        if self.snapshot().phase == Phase::CleanupRequired {
            return Err("网络资源尚未清理，请先停止连接".into());
        }
        input.validate()?;
        let operation_id = self.next.fetch_add(1, Ordering::AcqRel) + 1;
        {
            let mut s = self.state.lock().unwrap();
            s.operation = Some("queued".into());
            s.operation_id = operation_id;
            s.operation_error = None;
        }
        self.request(Request::Start {
            input,
            core_version,
            operation_id,
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
            if snapshot.operation_id == operation_id && snapshot.operation.is_none() {
                if let Some(e) = snapshot.operation_error {
                    return Err(e);
                }
                if snapshot.phase == Phase::Running && snapshot.network.ready() {
                    return Ok(());
                }
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
    fn query(&self, request: Request) -> Result<Snapshot, String> {
        let (tx, rx) = mpsc::channel();
        {
            let guard = self.client.lock().unwrap();
            let client = guard.as_ref().ok_or("当前没有活动会话")?;
            if client.disconnect.load(Ordering::Acquire) {
                return Err("会话正在结束".into());
            }
            client
                .tx
                .try_send(Command {
                    request,
                    reply: Some(tx),
                })
                .map_err(|_| "授权进程已断开")?;
        }
        rx.recv_timeout(Duration::from_secs(5))
            .map_err(|_| "活动查询超时".into())
    }
    pub fn activity(&self, query: crate::activity::Query) -> Result<crate::activity::Page, String> {
        query.validate()?;
        self.query(Request::Activity { query })?
            .activity_page
            .ok_or("活动数据不可用".into())
    }
    pub fn logs(&self, after: u64, limit: usize) -> Result<Vec<crate::logs::Log>, String> {
        if limit == 0 || limit > 100 {
            return Err("日志分页超限".into());
        }
        if self
            .client
            .lock()
            .unwrap()
            .as_ref()
            .is_none_or(|c| c.thread.is_finished())
        {
            return Ok(self
                .history
                .lock()
                .unwrap()
                .iter()
                .filter(|l| l.sequence > after)
                .take(limit)
                .cloned()
                .collect());
        }
        Ok(self.query(Request::Logs { after, limit })?.logs)
    }
    pub fn stop(&self) -> Result<(), String> {
        {
            let mut guard = self.client.lock().unwrap();
            if guard.as_ref().is_some_and(|c| c.thread.is_finished()) {
                let _ = guard.take().unwrap().thread.join();
                if self.snapshot().phase != Phase::CleanupRequired
                    && network::require_clear().is_ok()
                {
                    *self.state.lock().unwrap() = Snapshot::default();
                    self.released();
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
            self.released();
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
        if tx
            .try_send(Command {
                request: Request::Stop,
                reply: None,
            })
            .is_err()
        {
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
        network::require_clear()?;
        self.released();
        Ok(())
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
        let (tx, _rx) = mpsc::sync_channel(8);
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
        let worker = Worker::new(std::env::temp_dir(), "a".repeat(64));
        worker.state.lock().unwrap().phase = Phase::Running;
        assert_eq!(worker.snapshot().phase, Phase::Failed);
        assert!(worker.snapshot().running_node.is_none());
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
        if mode == "--network-daemon" {
            return Err("旧网络服务已停用，请从当前应用重新连接并完成系统授权".into());
        }
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
            let peer = {
                let endpoint = args.get(2).map(PathBuf::from).ok_or("缺少网络控制端点")?;
                let parent = args
                    .get(3)
                    .and_then(|s| s.to_str())
                    .and_then(|s| s.parse().ok())
                    .ok_or("缺少网络控制端身份")?;
                authorization::worker(&endpoint, parent).await?
            };
            let mut channel = peer.channel;
            let root = authorization::prepare_root()?;
            let lease = crate::instance::Lease::acquire(&root)?;
            let mut worker = Worker::new(root, peer.owner_key);
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
                let stopping = matches!(request, Request::Stop | Request::Quit);
                let activity = if let Request::Activity { query } = &request {
                    Some(worker.activity(query))
                } else {
                    None
                };
                let logs = if let Request::Logs { after, limit } = &request {
                    Some(worker.runtime.logs.page(*after, *limit))
                } else {
                    None
                };
                let outcome = match request {
                    Request::Start {
                        input,
                        core_version,
                        operation_id,
                    } => worker.start(input, core_version, operation_id),
                    Request::CoreBegin {
                        version,
                        material,
                        operation_id,
                    } => {
                        if worker.snapshot().operation.is_some() {
                            Err("已有连接操作".into())
                        } else {
                            worker.transfer.take();
                            {
                                let mut state = worker.state.lock().unwrap();
                                state.error = None;
                                state.operation_error = None;
                                state.failure = None;
                                state.operation_id = operation_id;
                                if state.runtime.pid.is_none() {
                                    state.phase = Phase::Preparing;
                                }
                            }
                            match material {
                                Some(material) => install::Transfer::begin(
                                    &worker.root,
                                    &version,
                                    material,
                                    &worker.runtime,
                                )
                                .map(|transfer| worker.transfer = transfer),
                                None => install::existing(
                                    &worker.root,
                                    &version,
                                    &worker.runtime,
                                    &AtomicBool::new(false),
                                )
                                .map(|_| ()),
                            }
                        }
                    }
                    Request::CoreChunk { offset, bytes } => worker
                        .transfer
                        .as_mut()
                        .ok_or("没有活动核心传输".to_string())
                        .and_then(|t| t.chunk(offset, &bytes)),
                    Request::CoreFinish => worker
                        .transfer
                        .take()
                        .ok_or("没有活动核心传输".to_string())
                        .and_then(|t| t.finish(&worker.root, &worker.runtime)),
                    Request::Poll | Request::Activity { .. } | Request::Logs { .. } => Ok(()),
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
                let mut response = worker.snapshot();
                if stopping {
                    let mut size = 0;
                    response.logs = worker
                        .runtime
                        .logs
                        .snapshot()
                        .into_iter()
                        .rev()
                        .take_while(|l| {
                            size += l.message.len();
                            size < 256 * 1024
                        })
                        .take(100)
                        .collect();
                    response.logs.reverse();
                }
                if let Some(page) = activity {
                    match page {
                        Ok(page) => response.activity_page = Some(page),
                        Err(e) => response.operation_error = Some(e),
                    }
                }
                if let Some(logs) = logs {
                    response.logs = logs;
                }
                if let Err(error) =
                    tokio::time::timeout(Duration::from_secs(10), write(&mut channel, &response))
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

async fn confirm_closed(channel: &mut authorization::Channel) -> Result<(), String> {
    let closed: Closed = tokio::time::timeout(Duration::from_secs(10), read(channel))
        .await
        .map_err(|_| "授权进程尚未释放资源")??;
    if !closed.closed {
        return Err("授权进程结束回应无效".into());
    }
    let mut end = [0; 1];
    if tokio::time::timeout(Duration::from_secs(10), channel.read(&mut end))
        .await
        .map_err(|_| "授权进程尚未释放控制通道")?
        .map_err(|e| e.to_string())?
        != 0
    {
        return Err("授权进程退出回应无效".into());
    }
    Ok(())
}

#[cfg(target_os = "macos")]
pub fn authorization_probe() -> Result<(), String> {
    if authorization::elevated() {
        return Err("授权探针须以普通用户运行，不能用 root 入口替代系统授权".into());
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
        confirm_closed(&mut connection.channel).await?;
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
        #[serde(default)]
        options: crate::profile::ConnectionOptions,
        core_version: String,
        core_program: Option<PathBuf>,
        material: Option<crate::core::material::Envelope>,
        control: PathBuf,
    }
    let input: Input = serde_json::from_slice(&std::fs::read(file).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    crate::private_files::directory(&input.control)?;
    let application = Arc::new(crate::app::Application::open(
        input.control.join("app"),
        "0.0.0".into(),
    )?);
    if let (Some(program), Some(envelope)) = (&input.core_program, &input.material) {
        crate::core::material::program(
            envelope,
            &input.core_version,
            program,
            &AtomicBool::new(false),
        )?;
        let reference = crate::settings::ManagedRef {
            version: input.core_version.clone(),
            target: crate::TARGET.into(),
            hash: Some(crate::core::package::hash(program)?),
            source: Some(envelope.candidate(&input.core_version)?),
        };
        let destination = crate::settings::managed_path(&application.store.root, &reference)?;
        crate::private_files::directory(destination.parent().unwrap())?;
        std::fs::copy(program, &destination).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&destination, std::fs::Permissions::from_mode(0o700))
                .map_err(|e| e.to_string())?;
        }
        application
            .store
            .core(application.store.snapshot().core_revision, |s| {
                s.managed.active = Some(reference);
                s.update_preferences.automatic_core = false;
                Ok(())
            })?;
    }
    application.session.initialize()?;
    if !application.core.snapshot().usable {
        application
            .core
            .maintain(Some(crate::core::Maintenance::Retry))?;
        let deadline = Instant::now() + Duration::from_secs(160);
        while !application.core.snapshot().usable {
            let state = application.core.snapshot();
            if state.maintenance.phase == crate::core::managed::Phase::Error {
                return Err(state
                    .maintenance
                    .error
                    .unwrap_or("fixture core preparation failed".into()));
            }
            if Instant::now() >= deadline {
                return Err("fixture core preparation timed out".into());
            }
            thread::sleep(Duration::from_millis(50));
        }
    }
    if application
        .store
        .snapshot()
        .managed
        .active
        .as_ref()
        .map(|r| r.version.as_str())
        != Some(input.core_version.as_str())
    {
        return Err("fixture requires its pinned official core version".into());
    }
    fn import_node(app: &crate::app::Application, node: &Node) -> Result<String, String> {
        let preview = app.profiles.preview(&node.spec.outbound().to_string())?;
        app.profiles
            .commit(preview.ticket, app.store.snapshot().nodes_revision)?;
        app.store
            .snapshot()
            .profile
            .nodes
            .iter()
            .find(|n| n.spec == node.spec)
            .map(|n| n.id.clone())
            .ok_or("fixture node import failed".into())
    }
    fn wait_connect(app: &Arc<crate::app::Application>, id: String) -> Result<(), String> {
        let admission_deadline = Instant::now() + Duration::from_secs(30);
        while app.gate.runtime_busy() {
            if Instant::now() >= admission_deadline {
                return Err("fixture admission timed out".into());
            }
            thread::sleep(Duration::from_millis(25));
        }
        app.session.connect(id.clone())?;
        let deadline = Instant::now() + Duration::from_secs(180);
        while app.gate.runtime_busy() {
            if Instant::now() >= deadline {
                return Err("fixture connection timed out".into());
            }
            thread::sleep(Duration::from_millis(50));
        }
        let state = app.session.snapshot();
        if state.phase != "running" || state.running_node.as_ref().map(|n| &n.id) != Some(&id) {
            return Err(state
                .runtime
                .error
                .unwrap_or("fixture connection was not confirmed".into()));
        }
        Ok(())
    }
    let id = import_node(&application, &input.node)?;
    application.store.connection_options(
        application.store.snapshot().connection_revision,
        input.options,
    )?;
    application.profiles.routing(
        input.routing.clone(),
        application.store.snapshot().routing_revision,
    )?;
    wait_connect(&application, id)?;
    let tunnel = &application.session.tunnel;
    let cancel = AtomicBool::new(false);
    crate::settings::write_json(&input.control.join("started.json"), &tunnel.snapshot())?;
    let deadline = Instant::now() + Duration::from_secs(60);
    while !input.control.join("stop").exists() {
        if Instant::now() >= deadline {
            tunnel.close()?;
            return Err("fixture controller timed out".into());
        }
        let command = input.control.join("replace.json");
        if command.exists() {
            #[derive(Deserialize)]
            #[serde(deny_unknown_fields)]
            struct Replace {
                id: u64,
                input: ConnectionInput,
            }
            let request: Replace =
                serde_json::from_slice(&std::fs::read(&command).map_err(|e| e.to_string())?)
                    .map_err(|e| e.to_string())?;
            std::fs::remove_file(command).map_err(|e| e.to_string())?;
            let result = if request.id == 1 {
                tunnel
                    .prepare_core(&request.input.core_version, &cancel)
                    .and_then(|_| {
                        tunnel.start(request.input.clone(), request.input.core_version, &cancel)
                    })
            } else {
                let id = import_node(&application, &request.input.node)?;
                application.profiles.routing(
                    request.input.routing,
                    application.store.snapshot().routing_revision,
                )?;
                application.store.connection_options(
                    application.store.snapshot().connection_revision,
                    request.input.options,
                )?;
                wait_connect(&application, id)
            };
            crate::settings::write_json(
                &input.control.join(format!("replace-{}.json", request.id)),
                &serde_json::json!({"succeeded":result.is_ok(),"error":result.err(),"snapshot":tunnel.snapshot(),"session":application.session.snapshot(),"settings":application.store.snapshot().public()}),
            )?;
        }
        let query = input.control.join("query-activity");
        if query.exists() {
            std::fs::remove_file(query).map_err(|e| e.to_string())?;
            let page = tunnel.activity(crate::activity::Query::default())?;
            crate::settings::write_json(&input.control.join("activity.json"), &page)?;
        }
        thread::sleep(Duration::from_millis(50));
    }
    application.session.stop()?;
    crate::settings::write_json(&input.control.join("stopped.json"), &tunnel.snapshot())?;
    Ok(())
}
