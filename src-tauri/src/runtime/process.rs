use crate::{
    logs::Logs,
    platform::{self, ProcessGroup},
};
use std::{
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
pub(crate) struct Process {
    pub(crate) child: Child,
    group: ProcessGroup,
    readers: Vec<JoinHandle<()>>,
    terminated: bool,
}
impl Process {
    pub(crate) fn spawn(
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
    pub(crate) fn terminate(&mut self) -> Result<(), String> {
        if !self.terminated {
            if self.child.try_wait().map_err(|e| e.to_string())?.is_none()
                && self.group.request_shutdown().unwrap_or(false)
            {
                let graceful = Instant::now() + Duration::from_secs(3);
                while Instant::now() < graceful {
                    if self.child.try_wait().map_err(|e| e.to_string())?.is_some() {
                        break;
                    }
                    thread::sleep(Duration::from_millis(20));
                }
            }
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
