use crate::{logs::Logs, settings};
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    process::Command,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeSnapshot {
    pub status: String,
    pub pid: Option<u32>,
    pub version: Option<String>,
    pub error: Option<String>,
    pub cleanup_pending: bool,
}
impl Default for RuntimeSnapshot {
    fn default() -> Self {
        Self {
            status: "stopped".into(),
            pid: None,
            version: None,
            error: None,
            cleanup_pending: false,
        }
    }
}
pub struct StartSpec {
    pub executable: PathBuf,
    pub config: PathBuf,
    pub version: Option<String>,
}
pub struct RuntimeService {
    network_owned: Arc<AtomicBool>,
    network_root: Mutex<Option<PathBuf>>,
    state: Arc<Mutex<RuntimeSnapshot>>,
    stop: Arc<AtomicBool>,
    worker: Mutex<Option<JoinHandle<()>>>,
    pending_checks: Mutex<Vec<Process>>,
    pub logs: Logs,
    pub events: crate::activity::Events,
}
pub(crate) mod control;
pub(crate) mod process;
use process::Process;
impl Default for RuntimeService {
    fn default() -> Self {
        Self {
            network_owned: Arc::new(AtomicBool::new(false)),
            network_root: Mutex::new(None),
            state: Arc::new(Mutex::new(Default::default())),
            stop: Arc::new(AtomicBool::new(false)),
            worker: Mutex::new(None),
            pending_checks: Mutex::new(Vec::new()),
            logs: Default::default(),
            events: Default::default(),
        }
    }
}
impl RuntimeService {
    pub fn record_network(&self, root: PathBuf) {
        *self.network_root.lock().unwrap() = Some(root);
    }
    pub(crate) fn network_occupation(&self) -> Arc<AtomicBool> {
        self.network_owned.clone()
    }
    #[cfg(feature = "test-fixtures")]
    pub fn network_owned(&self, value: bool) {
        self.network_owned.store(value, Ordering::Release);
    }
    pub fn clear_error(&self) {
        let mut state = self.state.lock().unwrap();
        state.error = None;
        if state.pid.is_none() && state.status == "failed" {
            state.status = "stopped".into();
        }
    }
    pub fn report(&self, error: String) {
        self.events.push("operation", "failed", error.clone());
        self.logs.push("shell", error.clone());
        let mut state = self.state.lock().unwrap();
        state.error = Some(error);
        if state.pid.is_none() {
            state.status = "failed".into();
        }
    }
    pub fn snapshot(&self) -> RuntimeSnapshot {
        let mut state = self.state.lock().unwrap().clone();
        state.cleanup_pending = !self.pending_checks.lock().unwrap().is_empty();
        state
    }
    pub fn stopped(&self) -> Result<(), String> {
        let s = self.snapshot();
        if s.pid.is_some()
            || self.network_owned.load(Ordering::Acquire)
            || s.cleanup_pending
            || ["checking", "starting", "stopping"].contains(&s.status.as_str())
        {
            Err("请先停止 sing-box，再更换核心或配置".into())
        } else {
            Ok(())
        }
    }
    pub fn probe(&self, path: &Path, cancel: &AtomicBool) -> Result<String, String> {
        let path = file(path)?;
        let output = self.command(&path, None, "version", cancel)?;
        let version = output
            .lines()
            .find_map(|line| line.strip_prefix("sing-box version "))
            .and_then(|line| line.split_whitespace().next())
            .ok_or("不是有效的 sing-box 版本输出")?;
        settings::stable(version)?;
        Ok(version.into())
    }
    pub fn supports_activity(&self, path: &Path, cancel: &AtomicBool) -> Result<bool, String> {
        let details = self.command(path, None, "version", cancel)?;
        Ok(details
            .lines()
            .filter_map(|line| line.strip_prefix("Tags:"))
            .flat_map(|tags| tags.split(','))
            .any(|tag| tag.trim() == "with_clash_api"))
    }
    fn command(
        &self,
        core: &Path,
        config: Option<&Path>,
        action: &str,
        cancel: &AtomicBool,
    ) -> Result<String, String> {
        if !self.pending_checks.lock().unwrap().is_empty() {
            return Err("先前校验进程尚未收尾，请停止后重试".into());
        }
        let mut command = Command::new(core);
        command.arg(action);
        if let Some(config) = config {
            let config = file(config)?;
            command
                .arg("-c")
                .arg(&config)
                .current_dir(config.parent().ok_or("配置目录不存在")?);
        }
        let capture = Arc::new(Mutex::new(Vec::new()));
        let command_logs = if action == "format" {
            Logs::default()
        } else {
            self.logs.clone()
        };
        let mut process = Process::spawn(&mut command, &command_logs, Some(capture.clone()))?;
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            if cancel.load(Ordering::Acquire) {
                self.finish_check(process)?;
                return Err("操作已取消".into());
            }
            let exit = match process.child.try_wait() {
                Ok(exit) => exit,
                Err(e) => {
                    self.finish_check(process)?;
                    return Err(e.to_string());
                }
            };
            if let Some(status) = exit {
                self.finish_check(process)?;
                if !status.success() {
                    return Err(format!("sing-box 校验失败（{status}），请查看日志"));
                }
                return Ok(String::from_utf8_lossy(&capture.lock().unwrap()).into_owned());
            }
            if Instant::now() >= deadline {
                self.finish_check(process)?;
                return Err("sing-box 校验超过 15 秒".into());
            }
            thread::sleep(Duration::from_millis(20));
        }
    }
    fn finish_check(&self, mut process: Process) -> Result<(), String> {
        if let Err(error) = process.terminate() {
            self.pending_checks.lock().unwrap().push(process);
            return Err(error);
        }
        Ok(())
    }
    pub fn validate(&self, core: &Path, config: &Path, cancel: &AtomicBool) -> Result<(), String> {
        file(core)?;
        self.command(core, Some(config), "check", cancel)
            .map(|_| ())
    }
    pub fn format_config(
        &self,
        core: &Path,
        config: &Path,
        cancel: &AtomicBool,
    ) -> Result<String, String> {
        self.command(core, Some(config), "format", cancel)
    }
    pub fn start(&self, spec: &StartSpec, cancel: Arc<AtomicBool>) -> Result<(), String> {
        if self.snapshot().pid.is_some() {
            return Err("sing-box 已在运行".into());
        }
        self.stopped()?;
        self.clear_error();
        self.state.lock().unwrap().status = "starting".into();
        let prepared: Result<(Process, String), String> = (|| {
            let version = self.probe(&spec.executable, &cancel)?;
            if spec
                .version
                .as_ref()
                .is_some_and(|expected| expected != &version)
            {
                return Err("核心实际版本与选择不符".into());
            }
            self.validate(&spec.executable, &spec.config, &cancel)?;
            if cancel.load(Ordering::Acquire) {
                return Err("启动已取消".into());
            }
            let mut command = Command::new(&spec.executable);
            command
                .arg("run")
                .arg("-c")
                .arg(&spec.config)
                .current_dir(spec.config.parent().ok_or("配置目录不存在")?);
            Ok((Process::spawn(&mut command, &self.logs, None)?, version))
        })();
        let (mut process, version) = match prepared {
            Ok(p) => p,
            Err(e) => {
                let mut s = self.state.lock().unwrap();
                s.status = "failed".into();
                s.error = Some(e.clone());
                return Err(e);
            }
        };
        if let Some(root) = self.network_root.lock().unwrap().as_ref() {
            if let Err(error) =
                crate::platform::network::record(root, process.child.id(), &spec.executable)
            {
                let _ = process.terminate();
                let mut state = self.state.lock().unwrap();
                state.status = "failed".into();
                state.error = Some(error.clone());
                return Err(error);
            }
        }
        if let Some(worker) = self.worker.lock().unwrap().take() {
            let _ = worker.join();
        }
        self.stop.store(false, Ordering::Release);
        *self.state.lock().unwrap() = RuntimeSnapshot {
            status: "starting".into(),
            pid: Some(process.child.id()),
            version: Some(version),
            error: None,
            cleanup_pending: false,
        };
        let (send, receive) = std::sync::mpsc::sync_channel(1);
        let state = self.state.clone();
        let stop = self.stop.clone();
        let logs = self.logs.clone();
        *self.worker.lock().unwrap() = Some(thread::spawn(move || {
            let mut initial = Some(send);
            let observed = Instant::now() + Duration::from_secs(2);
            loop {
                let requested = stop.load(Ordering::Acquire)
                    || (initial.is_some() && cancel.load(Ordering::Acquire));
                let exit = process.child.try_wait();
                if requested || !matches!(exit, Ok(None)) {
                    match process.terminate() {
                        Ok(()) => {
                            let error = if requested {
                                None
                            } else {
                                Some(format!("sing-box 意外退出（{exit:?}）"))
                            };
                            logs.push("shell", error.as_deref().unwrap_or("sing-box 已停止"));
                            {
                                let mut s = state.lock().unwrap();
                                s.pid = None;
                                s.status =
                                    if error.is_some() { "failed" } else { "stopped" }.into();
                                s.error = error.clone();
                            }
                            if let Some(send) = initial.take() {
                                let _ = send.send(Err(error.unwrap_or("启动已取消".into())));
                            }
                            break;
                        }
                        Err(e) => {
                            state.lock().unwrap().error = Some(e);
                        }
                    }
                } else if initial.is_some() && Instant::now() >= observed {
                    state.lock().unwrap().status = "running".into();
                    logs.push("shell", "已启动 sing-box；连接情况请查看核心日志");
                    let _ = initial.take().unwrap().send(Ok(()));
                }
                thread::sleep(Duration::from_millis(20));
            }
        }));
        receive
            .recv_timeout(Duration::from_secs(9))
            .map_err(|_| "启动或资源收尾尚未完成，请停止后重试".to_string())?
    }
    pub fn stop(&self) -> Result<(), String> {
        let pending = std::mem::take(&mut *self.pending_checks.lock().unwrap());
        let mut cleanup_error = None;
        for process in pending {
            if let Err(e) = self.finish_check(process) {
                cleanup_error = Some(e);
            }
        }
        if self.snapshot().pid.is_some() {
            self.state.lock().unwrap().status = "stopping".into();
            self.stop.store(true, Ordering::Release);
            let deadline = Instant::now() + Duration::from_secs(12);
            while self.snapshot().pid.is_some() {
                if Instant::now() >= deadline {
                    return Err("sing-box 尚未完成停止，请重试".into());
                }
                thread::sleep(Duration::from_millis(20));
            }
        }
        if let Some(worker) = self.worker.lock().unwrap().take() {
            let _ = worker.join();
        }
        if let Some(error) = cleanup_error {
            return Err(error);
        }
        self.clear_error();
        Ok(())
    }
}
impl Drop for RuntimeService {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}
pub fn file(path: &Path) -> Result<PathBuf, String> {
    if !path.is_absolute() {
        return Err("请选择绝对文件路径".into());
    }
    let path = path.canonicalize().map_err(|e| e.to_string())?;
    if !path.is_file() {
        return Err("所选路径不是文件".into());
    }
    Ok(path)
}
