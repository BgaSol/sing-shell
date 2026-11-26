use serde::{Deserialize, Serialize};
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

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Log {
    pub sequence: u64,
    pub received_at: u64,
    pub source: String,
    pub message: String,
}

#[derive(Clone, Default)]
pub struct Logs(Arc<Mutex<VecDeque<Log>>>, Arc<Mutex<Vec<String>>>);
impl Logs {
    pub fn redact(&self, secrets: Vec<String>) {
        *self.1.lock().unwrap() = secrets.into_iter().filter(|s| !s.is_empty()).collect();
    }
    pub fn push(&self, source: &str, message: impl Into<String>) {
        let mut message: String = message.into().chars().take(8192).collect();
        for secret in self.1.lock().unwrap().iter() {
            message = message.replace(secret, "[已隐藏]");
        }
        let mut logs = self.0.lock().unwrap();
        let sequence = logs.back().map(|l| l.sequence + 1).unwrap_or(1);
        if logs.len() == 1000 {
            logs.pop_front();
        }
        logs.push_back(Log {
            sequence,
            received_at: now(),
            source: source.into(),
            message,
        });
    }
    pub fn snapshot(&self) -> Vec<Log> {
        self.0.lock().unwrap().iter().cloned().collect()
    }
    pub fn page(&self, after: u64, limit: usize) -> Vec<Log> {
        let mut size = 0;
        self.0
            .lock()
            .unwrap()
            .iter()
            .filter(|l| l.sequence > after)
            .take(limit.min(100))
            .take_while(|log| {
                size += serde_json::to_vec(log)
                    .map(|b| b.len())
                    .unwrap_or(usize::MAX);
                size <= 256 * 1024
            })
            .cloned()
            .collect()
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
                    let remaining = (2 * 1024 * 1024usize).saturating_sub(value.len());
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
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn paging_is_bounded_by_encoded_bytes_and_keeps_redaction_across_switches() {
        let logs = Logs::default();
        logs.redact(vec!["first-secret".into()]);
        logs.push("core", "first-secret");
        logs.redact(vec!["second-secret".into()]);
        logs.push("core", "second-secret");
        assert!(logs
            .snapshot()
            .iter()
            .all(|log| !log.message.contains("secret")));
        assert_eq!(logs.1.lock().unwrap().len(), 1);
        for _ in 0..100 {
            logs.push("core", "\u{0000}".repeat(8192));
        }
        let page = logs.page(0, 100);
        assert!(!page.is_empty());
        assert!(page.len() < 100);
        assert!(serde_json::to_vec(&page).unwrap().len() < 257 * 1024);
        let next = logs.page(page.last().unwrap().sequence, 100);
        assert!(!next.is_empty());
    }
}
