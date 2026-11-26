use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Query {
    pub session_id: Option<String>,
    pub after: Option<u64>,
    pub state: String,
    pub search: String,
    pub limit: usize,
}
impl Default for Query {
    fn default() -> Self {
        Self {
            session_id: None,
            after: None,
            state: "all".into(),
            search: String::new(),
            limit: 100,
        }
    }
}
impl Query {
    pub fn validate(&self) -> Result<(), String> {
        if self.limit == 0
            || self.limit > 100
            || self.search.len() > 256
            || !matches!(self.state.as_str(), "all" | "active" | "ended")
        {
            return Err("活动查询参数无效".into());
        }
        Ok(())
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Connection {
    pub sequence: u64,
    pub id: String,
    pub target: String,
    pub host: Option<String>,
    pub protocol: String,
    pub process: Option<String>,
    pub exit: Option<String>,
    pub policy_id: Option<String>,
    pub policy_name: Option<String>,
    pub rule: String,
    pub started: String,
    pub upload: u64,
    pub download: u64,
    pub active: bool,
    pub last_seen: u64,
}
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    pub session_id: Option<String>,
    pub state: String,
    pub observed: usize,
    pub active: usize,
    pub upload: u64,
    pub download: u64,
    pub upload_rate: Option<u64>,
    pub download_rate: Option<u64>,
    pub sampled_at: Option<u64>,
    pub error: Option<String>,
}
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Page {
    pub summary: Summary,
    pub items: Vec<Connection>,
    pub next: Option<u64>,
    pub truncated: bool,
}
#[derive(Default)]
struct Observed {
    summary: Summary,
    active: BTreeMap<String, Connection>,
    ended: VecDeque<Connection>,
    next: u64,
    last: Option<(Instant, u64, u64)>,
}
impl Observed {
    fn accept(&mut self, value: Wire, policies: &BTreeMap<String, (String, String)>) {
        let now = Instant::now();
        self.summary.upload_rate = None;
        self.summary.download_rate = None;
        if let Some((time, up, down)) = self.last {
            let elapsed = now.duration_since(time).as_secs_f64();
            if elapsed > 0.0 && value.upload_total >= up && value.download_total >= down {
                self.summary.upload_rate =
                    Some(((value.upload_total - up) as f64 / elapsed) as u64);
                self.summary.download_rate =
                    Some(((value.download_total - down) as f64 / elapsed) as u64);
            }
        }
        self.last = Some((now, value.upload_total, value.download_total));
        self.summary.upload = value.upload_total;
        self.summary.download = value.download_total;
        self.summary.sampled_at = Some(crate::logs::now());
        self.summary.error = None;
        let total = value.connections.len();
        let complete = total <= 1000;
        self.summary.state = if complete { "ready" } else { "overflow" }.into();
        let mut seen = BTreeSet::new();
        for v in value.connections.into_iter().take(1000) {
            let id = short(&v.id, 128);
            if id.is_empty() || !seen.insert(id.clone()) {
                continue;
            }
            let sequence = if let Some(old) = self.active.get(&id) {
                old.sequence
            } else {
                self.next += 1;
                self.next
            };
            let rule = short(&v.rule, 512);
            let policy = policies
                .iter()
                .find(|(tag, _)| {
                    rule.strip_prefix(&format!("rule_set={tag}"))
                        .is_some_and(|rest| rest.starts_with(" => route("))
                })
                .map(|(_, v)| v.clone());
            let policy = if rule == "final" {
                Some(("default".into(), "默认规则".into()))
            } else {
                policy
            };
            let exit = v
                .chains
                .iter()
                .find(|s| matches!(s.as_str(), "direct" | "proxy"))
                .cloned();
            let host = v
                .metadata
                .get("host")
                .and_then(|v| v.as_str())
                .filter(|v| !v.is_empty())
                .map(|v| short(v, 253));
            let ip = field(&v.metadata, "destinationIP", 128);
            let port = field(&v.metadata, "destinationPort", 16);
            let target = format!("{}:{}", host.as_deref().unwrap_or(&ip), port);
            let process = v
                .metadata
                .get("processPath")
                .or_else(|| v.metadata.get("process"))
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(|s| short(s, 512));
            self.active.insert(
                id.clone(),
                Connection {
                    sequence,
                    id,
                    target,
                    host,
                    protocol: field(&v.metadata, "network", 32),
                    process,
                    exit,
                    policy_id: policy.as_ref().map(|p| p.0.clone()),
                    policy_name: policy.map(|p| p.1),
                    rule,
                    started: short(&v.start, 64),
                    upload: v.upload,
                    download: v.download,
                    active: true,
                    last_seen: crate::logs::now(),
                },
            );
        }
        if complete {
            let ended: Vec<_> = self
                .active
                .keys()
                .filter(|id| !seen.contains(*id))
                .cloned()
                .collect();
            for id in ended {
                let mut c = self.active.remove(&id).unwrap();
                c.active = false;
                self.ended.push_front(c);
            }
            self.ended.truncate(1000);
        }
        while self.active.len() > 1000 {
            let key = self
                .active
                .iter()
                .min_by_key(|(_, v)| v.sequence)
                .map(|(k, _)| k.clone())
                .unwrap();
            self.active.remove(&key);
        }
        self.summary.active = total;
        self.summary.observed = self.active.len() + self.ended.len();
    }
    fn page(&self, q: &Query) -> Page {
        let search = q.search.to_lowercase();
        let mut all: Vec<_> = self
            .active
            .values()
            .chain(self.ended.iter())
            .filter(|c| {
                (q.state == "all" || (q.state == "active") == c.active)
                    && q.after.is_none_or(|after| c.sequence < after)
                    && (search.is_empty()
                        || c.target.to_lowercase().contains(&search)
                        || c.policy_name
                            .as_deref()
                            .unwrap_or("")
                            .to_lowercase()
                            .contains(&search))
            })
            .cloned()
            .collect();
        all.sort_by_key(|c| std::cmp::Reverse(c.sequence));
        let more = all.len() > q.limit;
        all.truncate(q.limit);
        Page {
            summary: self.summary.clone(),
            next: more.then(|| all.last().unwrap().sequence),
            items: all,
            truncated: self.summary.state == "overflow",
        }
    }
}
fn short(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}
fn field(v: &serde_json::Value, key: &str, max: usize) -> String {
    match v.get(key) {
        Some(serde_json::Value::String(s)) => short(s, max),
        Some(v) if v.is_number() => v.to_string(),
        _ => String::new(),
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Wire {
    #[serde(default)]
    upload_total: u64,
    #[serde(default)]
    download_total: u64,
    connections: Vec<WireConnection>,
}
#[derive(Deserialize)]
struct WireConnection {
    id: String,
    metadata: serde_json::Value,
    start: String,
    #[serde(default)]
    upload: u64,
    #[serde(default)]
    download: u64,
    #[serde(default)]
    chains: Vec<String>,
    #[serde(default)]
    rule: String,
}
pub struct Context {
    pub id: String,
    pub port: u16,
    pub secret: String,
}
impl Context {
    pub fn new() -> Result<Self, String> {
        let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .map_err(|e| e.to_string())?;
        let port = listener.local_addr().map_err(|e| e.to_string())?.port();
        Ok(Self {
            id: crate::platform::authorization::nonce()?,
            port,
            secret: crate::platform::authorization::nonce()?,
        })
    }
}
pub struct Collector {
    state: Arc<Mutex<Observed>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}
impl Collector {
    pub fn start(context: Context, routing: &crate::profile::routing::Routing) -> Self {
        let state = Arc::new(Mutex::new(Observed {
            summary: Summary {
                session_id: Some(context.id),
                state: "stale".into(),
                ..Default::default()
            },
            ..Default::default()
        }));
        let stop = Arc::new(AtomicBool::new(false));
        let shared = state.clone();
        let cancel = stop.clone();
        let policies = routing
            .policies
            .iter()
            .filter(|p| p.enabled)
            .map(|p| (p.tag(), (p.id.clone(), p.name.clone())))
            .collect();
        let thread = thread::spawn(move || {
            let _ = rustls::crypto::ring::default_provider().install_default();
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build();
            let Ok(runtime) = runtime else {
                let mut state = shared.lock().unwrap();
                state.summary.state = "unavailable".into();
                state.summary.error = Some("活动采集运行时无法启动".into());
                return;
            };
            let client = reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(2))
                .build();
            let Ok(client) = client else {
                let mut state = shared.lock().unwrap();
                state.summary.state = "unavailable".into();
                state.summary.error = Some("活动客户端无法初始化".into());
                return;
            };
            let url = format!("http://127.0.0.1:{}/connections", context.port);
            while !cancel.load(Ordering::Acquire) {
                let result: Result<Wire, String> = runtime.block_on(async {
                    let mut response = client
                        .get(&url)
                        .bearer_auth(&context.secret)
                        .send()
                        .await
                        .map_err(|_| "活动接口暂不可用")?
                        .error_for_status()
                        .map_err(|_| "活动接口鉴权或状态异常")?;
                    let mut bytes = Vec::new();
                    while let Some(chunk) = response.chunk().await.map_err(|_| "活动响应中断")?
                    {
                        if bytes.len() + chunk.len() > 4 * 1024 * 1024 {
                            return Err("活动响应超过 4 MiB".into());
                        }
                        bytes.extend_from_slice(&chunk);
                    }
                    serde_json::from_slice(&bytes).map_err(|_| "活动格式不受支持".into())
                });
                if cancel.load(Ordering::Acquire) {
                    break;
                }
                let mut state = shared.lock().unwrap();
                match result {
                    Ok(v) => state.accept(v, &policies),
                    Err(e) => {
                        state.summary.state = if e.contains("4 MiB") {
                            "overflow"
                        } else {
                            "stale"
                        }
                        .into();
                        state.summary.error = Some(e);
                        state.summary.upload_rate = None;
                        state.summary.download_rate = None;
                        state.last = None;
                    }
                }
                drop(state);
                for _ in 0..40 {
                    if cancel.load(Ordering::Acquire) {
                        break;
                    }
                    thread::sleep(Duration::from_millis(25));
                }
            }
        });
        Self {
            state,
            stop,
            thread: Some(thread),
        }
    }
    pub fn summary(&self) -> Summary {
        self.state.lock().unwrap().summary.clone()
    }
    pub fn page(&self, q: &Query) -> Result<Page, String> {
        q.validate()?;
        let state = self.state.lock().unwrap();
        if q.session_id
            .as_ref()
            .is_some_and(|id| Some(id) != state.summary.session_id.as_ref())
        {
            return Err("活动会话已改变，请刷新".into());
        }
        Ok(state.page(q))
    }
}
impl Drop for Collector {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn incomplete_sampling_does_not_end_connections_or_invent_rates() {
        let mut s = Observed::default();
        let wire = |id: &str| {
            serde_json::from_value(serde_json::json!({"connections":[{"id":id,"metadata":{"host":"example.com","network":"tcp"},"start":"now","rule":"final","chains":["proxy"]}]})).unwrap()
        };
        s.accept(wire("one"), &BTreeMap::new());
        assert_eq!(s.summary.upload_rate, None);
        assert_eq!(s.active["one"].policy_id.as_deref(), Some("default"));
        s.last = None;
        s.accept(wire("one"), &BTreeMap::new());
        assert_eq!(s.summary.upload_rate, None);
        s.accept(
            serde_json::from_value(serde_json::json!({"connections":[]})).unwrap(),
            &BTreeMap::new(),
        );
        assert_eq!(s.ended.len(), 1);
        assert!(!s.ended[0].active);
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Event {
    pub sequence: u64,
    pub at: u64,
    pub kind: String,
    pub outcome: String,
    pub message: String,
}
#[derive(Default)]
pub struct Events(Mutex<VecDeque<Event>>);
impl Events {
    pub fn push(&self, kind: &str, outcome: &str, message: impl Into<String>) {
        let mut events = self.0.lock().unwrap();
        let sequence = events.back().map(|e| e.sequence + 1).unwrap_or(1);
        if events.len() == 1000 {
            events.pop_front();
        }
        events.push_back(Event {
            sequence,
            at: crate::logs::now(),
            kind: kind.into(),
            outcome: outcome.into(),
            message: short(&message.into(), 1024),
        });
    }
    pub fn page(&self, after: u64, limit: usize) -> Vec<Event> {
        self.0
            .lock()
            .unwrap()
            .iter()
            .filter(|e| e.sequence > after)
            .take(limit.min(100))
            .cloned()
            .collect()
    }
}
