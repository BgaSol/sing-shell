use serde::Serialize;
use std::{
    collections::VecDeque,
    io::Read,
    sync::{Arc, Mutex},
    thread::{self, JoinHandle},
    time::{SystemTime, UNIX_EPOCH},
};

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Log {
    pub received_at: u64,
    pub source: String,
    pub message: String,
}

#[derive(Clone, Default)]
pub struct Logs(Arc<Mutex<VecDeque<Log>>>);
impl Logs {
    pub fn push(&self, source: &str, message: impl Into<String>) {
        let mut logs = self.0.lock().unwrap();
        if logs.len() == 1000 {
            logs.pop_front();
        }
        logs.push_back(Log {
            received_at: now(),
            source: source.into(),
            message: message.into(),
        });
    }
    pub fn snapshot(&self) -> Vec<Log> {
        self.0.lock().unwrap().iter().cloned().collect()
    }
    pub fn reader(
        &self,
        mut input: impl Read + Send + 'static,
        source: &'static str,
        capture: Option<Arc<Mutex<Vec<u8>>>>,
    ) -> JoinHandle<()> {
        let logs = self.clone();
        thread::spawn(move || {
            let mut bytes = [0; 2048];
            let mut line = Vec::new();
            loop {
                let count = match input.read(&mut bytes) {
                    Ok(0) => break,
                    Ok(n) => n,
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(e) => {
                        logs.push("shell", format!("读取 {source} 失败：{e}"));
                        break;
                    }
                };
                if let Some(capture) = &capture {
                    let mut value = capture.lock().unwrap();
                    let remaining = 65536usize.saturating_sub(value.len());
                    value.extend_from_slice(&bytes[..count.min(remaining)]);
                }
                for byte in &bytes[..count] {
                    if *byte == b'\n' || line.len() == 8192 {
                        logs.push(
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
                logs.push(source, String::from_utf8_lossy(&line).to_string());
            }
        })
    }
}
