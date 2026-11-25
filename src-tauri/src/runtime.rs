use crate::{
    logs::Logs,
    platform::{self, ProcessGroup},
    settings::{self, Engine, SettingsStore},
};
use serde::Serialize;
use std::{
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeSnapshot {
    pub status: String,
    pub pid: Option<u32>,
    pub version: Option<String>,
    pub error: Option<String>,
}
impl Default for RuntimeSnapshot {
    fn default() -> Self {
        Self {
            status: "stopped".into(),
            pid: None,
            version: None,
            error: None,
        }
    }
}
pub struct RuntimeService {
    state: Arc<Mutex<RuntimeSnapshot>>,
    stop: Arc<AtomicBool>,
    worker: Mutex<Option<JoinHandle<()>>>,
    pub logs: Logs,
}
struct Process {
    child: Child,
    group: ProcessGroup,
    readers: Vec<JoinHandle<()>>,
    terminated: bool,
}
impl Process {
    fn spawn(
        command: &mut Command,
        logs: &Logs,
        capture: Option<Arc<Mutex<Vec<u8>>>>,
    ) -> Result<Self, String> {
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let (mut child, group) =
            platform::spawn(command).map_err(|e| format!("无法启动 sing-box：{e}"))?;
        let readers = vec![
            logs.reader(child.stdout.take().unwrap(), "stdout", capture),
            logs.reader(child.stderr.take().unwrap(), "stderr", None),
        ];
        Ok(Self {
            child,
            group,
            readers,
            terminated: false,
        })
    }
    fn terminate(&mut self) -> Result<(), String> {
        if !self.terminated {
            self.group.terminate().map_err(|e| e.to_string())?;
            self.terminated = true;
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if self.child.try_wait().map_err(|e| e.to_string())?.is_some()
                && self.readers.iter().all(|r| r.is_finished())
            {
                for reader in self.readers.drain(..) {
                    let _ = reader.join();
                }
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err("进程或日志仍在收尾，请重试停止".into());
            }
            thread::sleep(Duration::from_millis(20));
        }
    }
}
impl Drop for Process {
    fn drop(&mut self) {
        if !self.terminated {
            let _ = self.group.terminate();
        }
        let _ = self.child.try_wait();
    }
}
impl Default for RuntimeService {
    fn default() -> Self {
        Self {
            state: Arc::new(Mutex::new(Default::default())),
            stop: Arc::new(AtomicBool::new(false)),
            worker: Mutex::new(None),
            logs: Default::default(),
        }
    }
}
impl RuntimeService {
    fn clear_error(&self) {
        let mut state = self.state.lock().unwrap();
        state.error = None;
        if state.pid.is_none() && state.status == "failed" {
            state.status = "stopped".into();
        }
    }
    pub fn report(&self, error: String) {
        self.logs.push("shell", error.clone());
        self.state.lock().unwrap().error = Some(error);
    }
    pub fn snapshot(&self) -> RuntimeSnapshot {
        self.state.lock().unwrap().clone()
    }
    pub fn stopped(&self) -> Result<(), String> {
        let s = self.snapshot();
        if s.pid.is_some() || ["checking", "starting", "stopping"].contains(&s.status.as_str()) {
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
    fn command(
        &self,
        core: &Path,
        config: Option<&Path>,
        action: &str,
        cancel: &AtomicBool,
    ) -> Result<String, String> {
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
        let mut process = Process::spawn(&mut command, &self.logs, Some(capture.clone()))?;
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            if cancel.load(Ordering::Acquire) {
                process.terminate()?;
                return Err("操作已取消".into());
            }
            if let Some(status) = process.child.try_wait().map_err(|e| e.to_string())? {
                process.terminate()?;
                if !status.success() {
                    return Err(format!("sing-box 校验失败（{status}），请查看日志"));
                }
                return Ok(String::from_utf8_lossy(&capture.lock().unwrap()).into_owned());
            }
            if Instant::now() >= deadline {
                process.terminate()?;
                return Err("sing-box 校验超过 15 秒".into());
            }
            thread::sleep(Duration::from_millis(20));
        }
    }
    pub fn validate(&self, core: &Path, config: &Path, cancel: &AtomicBool) -> Result<(), String> {
        file(core)?;
        self.command(core, Some(config), "check", cancel)
            .map(|_| ())
    }
    pub fn check(&self, store: &SettingsStore, cancel: &AtomicBool) -> Result<(), String> {
        self.clear_error();
        let settings = store.snapshot();
        let engine = settings.engine.as_ref().ok_or("请先选择 sing-box 核心")?;
        self.validate(
            &store.engine_path(engine)?,
            settings.config_path.as_deref().ok_or("请先导入配置")?,
            cancel,
        )?;
        self.logs.push("shell", "配置校验通过");
        Ok(())
    }
    pub fn start(&self, store: &SettingsStore, cancel: &AtomicBool) -> Result<(), String> {
        if self.snapshot().pid.is_some() {
            return Err("sing-box 已在运行".into());
        }
        self.stopped()?;
        self.clear_error();
        self.state.lock().unwrap().status = "starting".into();
        let result: Result<(Process, String), String> = (|| {
            let settings = store.snapshot();
            let path =
                store.engine_path(settings.engine.as_ref().ok_or("请先选择 sing-box 核心")?)?;
            let version = self.probe(&path, cancel)?;
            let config = settings.config_path.as_deref().ok_or("请先导入配置")?;
            self.validate(&path, config, cancel)?;
            if cancel.load(Ordering::Acquire) {
                return Err("操作已取消".into());
            }
            let mut command = Command::new(path);
            command
                .arg("run")
                .arg("-c")
                .arg(config)
                .current_dir(config.parent().unwrap());
            let process = Process::spawn(&mut command, &self.logs, None)?;
            Ok((process, version))
        })();
        let (mut process, version) = match result {
            Ok(v) => v,
            Err(e) => {
                let mut state = self.state.lock().unwrap();
                state.status = "failed".into();
                state.error = Some(e.clone());
                return Err(e);
            }
        };
        if let Some(worker) = self.worker.lock().unwrap().take() {
            let _ = worker.join();
        }
        self.stop.store(false, Ordering::Release);
        *self.state.lock().unwrap() = RuntimeSnapshot {
            status: "running".into(),
            pid: Some(process.child.id()),
            version: Some(version),
            error: None,
        };
        let stop = self.stop.clone();
        let state = self.state.clone();
        let logs = self.logs.clone();
        *self.worker.lock().unwrap() = Some(thread::spawn(move || loop {
            let requested = stop.load(Ordering::Acquire);
            let exit = process.child.try_wait();
            if requested || !matches!(exit, Ok(None)) {
                match process.terminate() {
                    Ok(()) => {
                        let error = if requested {
                            None
                        } else {
                            Some(format!("sing-box 意外退出（{exit:?}）"))
                        };
                        if let Some(e) = &error {
                            logs.push("shell", e.clone());
                        } else {
                            logs.push("shell", "sing-box 已停止");
                        }
                        let mut state = state.lock().unwrap();
                        state.pid = None;
                        state.status = if error.is_some() { "failed" } else { "stopped" }.into();
                        state.error = error;
                        break;
                    }
                    Err(e) => {
                        state.lock().unwrap().error = Some(e);
                        thread::sleep(Duration::from_millis(100));
                    }
                }
            }
            thread::sleep(Duration::from_millis(20));
        }));
        self.logs
            .push("shell", "已启动 sing-box；连接情况请查看核心日志");
        Ok(())
    }
    pub fn stop(&self) -> Result<(), String> {
        if self.snapshot().pid.is_some() {
            self.state.lock().unwrap().status = "stopping".into();
            self.stop.store(true, Ordering::Release);
            let deadline = Instant::now() + Duration::from_secs(7);
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
        self.clear_error();
        Ok(())
    }
    pub fn select(
        &self,
        store: &SettingsStore,
        path: &Path,
        cancel: &AtomicBool,
    ) -> Result<(), String> {
        self.stopped()?;
        self.clear_error();
        let path = file(path)?;
        self.probe(&path, cancel)?;
        let settings = store.snapshot();
        store.save(settings.revision, |s| {
            s.engine = Some(Engine::External { path })
        })?;
        Ok(())
    }
    pub fn import(
        &self,
        store: &SettingsStore,
        path: &Path,
        cancel: &AtomicBool,
    ) -> Result<(), String> {
        self.stopped()?;
        self.clear_error();
        let path = file(path)?;
        let settings = store.snapshot();
        self.validate(
            &store.engine_path(settings.engine.as_ref().ok_or("请先选择 sing-box 核心")?)?,
            &path,
            cancel,
        )?;
        store.save(settings.revision, |s| s.config_path = Some(path))?;
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
