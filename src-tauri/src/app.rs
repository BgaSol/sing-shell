use serde::Serialize;
use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

pub struct Application {
    pub store: Arc<crate::settings::SettingsStore>,
    pub runtime: Arc<crate::runtime::RuntimeService>,
    pub updates: Arc<crate::updates::service::UpdateService>,
    pub gate: Arc<Admission>,
    pub handed_off: AtomicBool,
    pub dirty: AtomicBool,
}
impl Application {
    pub fn open(root: std::path::PathBuf, version: String) -> Result<Self, String> {
        let store = Arc::new(crate::settings::SettingsStore::open(root)?);
        let runtime = Arc::new(crate::runtime::RuntimeService::default());
        let gate = Arc::new(Admission::default());
        let updates = crate::updates::service::UpdateService::new(
            store.clone(),
            runtime.clone(),
            gate.clone(),
            version,
        );
        Ok(Self {
            store,
            runtime,
            updates,
            gate,
            handed_off: AtomicBool::new(false),
            dirty: AtomicBool::new(false),
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    Runtime,
    Selection,
    Check,
    Download(crate::updates::Component),
    Diagnostic,
}
struct Active {
    kind: Kind,
    cancel: Arc<AtomicBool>,
}
#[derive(Default)]
struct State {
    closing: bool,
    next: u64,
    active: BTreeMap<u64, Active>,
}
#[derive(Default)]
pub struct Admission(Mutex<State>);
pub struct Operation {
    pub id: u64,
    pub cancel: Arc<AtomicBool>,
    gate: Arc<Admission>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Receipt {
    pub operation_id: u64,
}
impl Operation {
    pub fn receipt(&self) -> Receipt {
        Receipt {
            operation_id: self.id,
        }
    }
}
impl Drop for Operation {
    fn drop(&mut self) {
        self.gate.0.lock().unwrap().active.remove(&self.id);
    }
}
impl Admission {
    pub fn runtime_busy(&self) -> bool {
        self.0
            .lock()
            .unwrap()
            .active
            .values()
            .any(|a| matches!(a.kind, Kind::Runtime | Kind::Selection))
    }
    pub fn begin(self: &Arc<Self>, kind: Kind) -> Result<Operation, String> {
        let mut state = self.0.lock().unwrap();
        if state.closing {
            return Err("应用正在退出或重启".into());
        }
        let conflict = state.active.values().any(|v| match (kind, v.kind) {
            (Kind::Runtime | Kind::Selection, Kind::Runtime | Kind::Selection) => true,
            (Kind::Check, Kind::Check | Kind::Download(_)) | (Kind::Download(_), Kind::Check) => {
                true
            }
            (Kind::Download(a), Kind::Download(b)) => a == b,
            (Kind::Diagnostic, Kind::Diagnostic) => true,
            _ => false,
        });
        if conflict {
            return Err("已有相关任务在处理中，请稍后重试".into());
        }
        state.next += 1;
        let id = state.next;
        let cancel = Arc::new(AtomicBool::new(false));
        state.active.insert(
            id,
            Active {
                kind,
                cancel: cancel.clone(),
            },
        );
        Ok(Operation {
            id,
            cancel,
            gate: self.clone(),
        })
    }
    pub fn cancel_download(&self, component: crate::updates::Component) {
        for active in self.0.lock().unwrap().active.values() {
            if active.kind == Kind::Download(component) {
                active.cancel.store(true, Ordering::Release);
            }
        }
    }
    pub fn close(&self, install: bool) -> Result<(), String> {
        let mut state = self.0.lock().unwrap();
        if state.closing {
            return Err("已经在退出或重启".into());
        }
        if install && !state.active.is_empty() {
            return Err("请先完成或取消在途操作".into());
        }
        state.closing = true;
        for active in state.active.values() {
            active.cancel.store(true, Ordering::Release);
        }
        Ok(())
    }
    pub fn reopen(&self) {
        self.0.lock().unwrap().closing = false;
    }
    pub fn closing(&self) -> bool {
        self.0.lock().unwrap().closing
    }
    pub fn wait(&self) -> Result<(), String> {
        let deadline = Instant::now() + Duration::from_secs(25);
        while !self.0.lock().unwrap().active.is_empty() {
            if Instant::now() >= deadline {
                return Err("任务尚未停止，请稍后重试退出".into());
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        Ok(())
    }
}
