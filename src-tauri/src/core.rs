use crate::platform::{self, ProcessGroup};
use serde::{Deserialize, Serialize};
use std::{
    collections::VecDeque,
    fs::{self, File, OpenOptions},
    io::Read,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex},
    thread::{self, JoinHandle},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const MAX_LOGS: usize = 1000;
const MAX_LINE: usize = 8192;
const CHECK_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Settings {
    pub core_path: Option<PathBuf>,
    pub config_path: Option<PathBuf>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Log {
    pub received_at: u64,
    pub source: String,
    pub message: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub settings: Settings,
    pub status: &'static str,
    pub pid: Option<u32>,
    pub error: Option<String>,
    pub logs: Vec<Log>,
}

type Logs = Arc<Mutex<VecDeque<Log>>>;

struct Running {
    child: Child,
    group: ProcessGroup,
    readers: Vec<JoinHandle<()>>,
}

impl Running {
    fn terminate(&mut self) -> Result<(), String> {
        self.group
            .terminate()
            .map_err(|e| format!("停止 sing-box 失败：{e}"))?;
        self.child
            .wait()
            .map_err(|e| format!("等待 sing-box 退出失败：{e}"))?;
        for reader in self.readers.drain(..) {
            let _ = reader.join();
        }
        Ok(())
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.terminate();
    }
}

pub struct Core {
    directory: PathBuf,
    settings: Settings,
    running: Option<Running>,
    error: Option<String>,
    storage_error: Option<String>,
    logs: Logs,
    _lock: File,
}

fn log(logs: &Logs, source: &str, message: String) {
    let mut logs = logs.lock().unwrap_or_else(|e| e.into_inner());
    if logs.len() == MAX_LOGS {
        logs.pop_front();
    }
    logs.push_back(Log {
        received_at: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64,
        source: source.to_string(),
        message,
    });
}

fn reader(
    mut stream: impl Read + Send + 'static,
    logs: Logs,
    source: &'static str,
) -> JoinHandle<()> {
    thread::spawn(move || {
        let mut buffer = [0; 2048];
        let mut line = Vec::new();
        loop {
            let count = match stream.read(&mut buffer) {
                Ok(0) => break,
                Ok(count) => count,
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) => {
                    log(&logs, "shell", format!("读取 {source} 失败：{error}"));
                    break;
                }
            };
            for byte in &buffer[..count] {
                if *byte == b'\n' || line.len() == MAX_LINE {
                    log(
                        &logs,
                        source,
                        String::from_utf8_lossy(&line)
                            .trim_end_matches('\r')
                            .to_string(),
                    );
                    line.clear();
                }
                if *byte != b'\n' {
                    line.push(*byte);
                }
            }
        }
        if !line.is_empty() {
            log(&logs, source, String::from_utf8_lossy(&line).to_string());
        }
    })
}

fn file(path: &Path) -> Result<PathBuf, String> {
    if !path.is_absolute() {
        return Err("请选择绝对文件路径".into());
    }
    let path = path
        .canonicalize()
        .map_err(|e| format!("无法访问 {}：{e}", path.display()))?;
    if !path.is_file() {
        return Err("所选路径不是文件".into());
    }
    Ok(path)
}

impl Core {
    pub fn open(directory: PathBuf) -> Result<Self, String> {
        fs::create_dir_all(&directory).map_err(|e| format!("创建数据目录失败：{e}"))?;
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(directory.join("instance.lock"))
            .map_err(|e| format!("打开实例锁失败：{e}"))?;
        lock.try_lock().map_err(|e| {
            format!("数据目录正在使用，或无法锁定，请先关闭其他 Sing Shell 实例：{e}")
        })?;
        let mut error = None;
        let settings = match fs::read(directory.join("settings.json")) {
            Ok(bytes) => match serde_json::from_slice(&bytes) {
                Ok(settings) => settings,
                Err(e) => {
                    error = Some(format!(
                        "设置文件无法读取，已保留原文件。修复 settings.json 后重启：{e}"
                    ));
                    Settings::default()
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Settings::default(),
            Err(e) => return Err(format!("读取设置失败：{e}")),
        };
        Ok(Self {
            _lock: lock,
            directory,
            settings,
            running: None,
            storage_error: error.clone(),
            error,
            logs: Arc::new(Mutex::new(VecDeque::new())),
        })
    }

    fn require_stopped(&mut self) -> Result<(), String> {
        self.refresh()?;
        if self.running.is_some() {
            return Err("请先停止 sing-box，再更换核心或配置".into());
        }
        if let Some(error) = &self.storage_error {
            return Err(error.clone());
        }
        Ok(())
    }

    fn save(&mut self, settings: Settings) -> Result<(), String> {
        self.require_stopped()?;
        let bytes = serde_json::to_vec_pretty(&settings).map_err(|e| e.to_string())?;
        let temporary = self.directory.join("settings.json.tmp");
        fs::write(&temporary, bytes).map_err(|e| format!("保存设置失败：{e}"))?;
        fs::rename(temporary, self.directory.join("settings.json"))
            .map_err(|e| format!("提交设置失败：{e}"))?;
        self.settings = settings;
        self.error = None;
        Ok(())
    }

    pub fn select_core(&mut self, path: &Path) -> Result<(), String> {
        self.require_stopped()?;
        let core_path = file(path)?;
        self.check_command(&core_path, &["version"], None)?;
        self.save(Settings {
            core_path: Some(core_path),
            ..self.settings.clone()
        })
    }

    pub fn import_config(&mut self, path: &Path) -> Result<(), String> {
        self.require_stopped()?;
        let config_path = file(path)?;
        // Keep the original file and working directory so relative rule-set paths keep working.
        let core = self
            .settings
            .core_path
            .clone()
            .ok_or("请先选择 sing-box 核心")?;
        self.validate(&core, &config_path)?;
        self.save(Settings {
            config_path: Some(config_path),
            ..self.settings.clone()
        })
    }

    fn launch(&self, command: &mut Command) -> Result<Running, String> {
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let (mut child, group) =
            platform::spawn(command).map_err(|e| format!("无法启动 sing-box：{e}"))?;
        let readers = vec![
            reader(child.stdout.take().unwrap(), self.logs.clone(), "stdout"),
            reader(child.stderr.take().unwrap(), self.logs.clone(), "stderr"),
        ];
        Ok(Running {
            child,
            group,
            readers,
        })
    }

    fn check_command(
        &self,
        core: &Path,
        args: &[&str],
        config: Option<&Path>,
    ) -> Result<(), String> {
        let mut command = Command::new(core);
        command.args(args);
        if let Some(config) = config {
            command
                .arg("-c")
                .arg(config)
                .current_dir(config.parent().ok_or("配置目录不存在")?);
        }
        let mut check = self.launch(&mut command)?;
        let start = Instant::now();
        loop {
            if let Some(status) = check.child.try_wait().map_err(|e| e.to_string())? {
                check.terminate()?;
                return if status.success() {
                    Ok(())
                } else {
                    Err(format!("sing-box 校验失败（{status}），请查看日志"))
                };
            }
            if start.elapsed() >= CHECK_TIMEOUT {
                check.terminate()?;
                return Err("sing-box 校验超过 15 秒，已停止校验进程".into());
            }
            thread::sleep(Duration::from_millis(20));
        }
    }

    fn validate(&self, core: &Path, config: &Path) -> Result<(), String> {
        file(core)?;
        file(config)?;
        self.check_command(core, &["check"], Some(config))
    }

    pub fn check(&mut self) -> Result<(), String> {
        let core = self
            .settings
            .core_path
            .clone()
            .ok_or("请先选择 sing-box 核心")?;
        let config = self.settings.config_path.clone().ok_or("请先导入配置")?;
        self.validate(&core, &config)?;
        log(&self.logs, "shell", "配置校验通过".into());
        Ok(())
    }

    pub fn start(&mut self) -> Result<(), String> {
        self.refresh()?;
        if self.running.is_some() {
            return Err("sing-box 已在运行".into());
        }
        self.check()?;
        let mut command = Command::new(self.settings.core_path.as_ref().unwrap());
        let config = self.settings.config_path.as_ref().unwrap();
        command
            .arg("run")
            .arg("-c")
            .arg(config)
            .current_dir(config.parent().unwrap());
        self.running = Some(self.launch(&mut command)?);
        self.error = None;
        log(
            &self.logs,
            "shell",
            "已启动 sing-box；连接情况请查看核心日志".into(),
        );
        Ok(())
    }

    fn refresh(&mut self) -> Result<(), String> {
        if let Some(running) = &mut self.running {
            if let Some(status) = running
                .child
                .try_wait()
                .map_err(|e| format!("读取进程状态失败：{e}"))?
            {
                self.error = Some(format!("sing-box 意外退出（{status}）"));
                log(&self.logs, "shell", self.error.clone().unwrap());
                self.running.take();
            }
        }
        Ok(())
    }

    pub fn stop(&mut self) -> Result<(), String> {
        if let Some(running) = &mut self.running {
            running.terminate()?;
            self.running.take();
            log(&self.logs, "shell", "sing-box 已停止".into());
        }
        self.error = self.storage_error.clone();
        Ok(())
    }

    pub fn snapshot(&mut self) -> Result<Snapshot, String> {
        self.refresh()?;
        Ok(Snapshot {
            settings: self.settings.clone(),
            status: if self.running.is_some() {
                "running"
            } else if self.error.is_some() {
                "failed"
            } else {
                "stopped"
            },
            pid: self.running.as_ref().map(|running| running.child.id()),
            error: self.error.clone(),
            logs: self
                .logs
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .iter()
                .cloned()
                .collect(),
        })
    }
}
