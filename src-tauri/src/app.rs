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
    pub core: Arc<crate::core::CoreService>,
    pub wake: Arc<tokio::sync::Notify>,
    pub updates: Arc<crate::updates::service::UpdateService>,
    pub gate: Arc<Admission>,
    pub handed_off: AtomicBool,
    pub dirty: AtomicBool,
    pub dialog_open: AtomicBool,
    background_started: AtomicBool,
}
impl Application {
    pub fn open(root: std::path::PathBuf, version: String) -> Result<Self, String> {
        let store = Arc::new(crate::settings::SettingsStore::open(root)?);
        let runtime = Arc::new(crate::runtime::RuntimeService::default());
        let gate = Arc::new(Admission::default());
        let wake = Arc::new(tokio::sync::Notify::new());
        let core = crate::core::CoreService::new(
            store.clone(),
            runtime.clone(),
            gate.clone(),
            wake.clone(),
        );
        let updates = crate::updates::service::UpdateService::new(
            store.clone(),
            runtime.clone(),
            gate.clone(),
            version,
        );
        Ok(Self {
            core,
            wake,
            store,
            runtime,
            updates,
            gate,
            handed_off: AtomicBool::new(false),
            dirty: AtomicBool::new(false),
            dialog_open: AtomicBool::new(false),
            background_started: AtomicBool::new(false),
        })
    }
    pub fn start(self: &Arc<Self>, handle: tauri::AppHandle, automatic: bool) {
        if self.background_started.swap(true, Ordering::AcqRel) {
            return;
        }
        let app = self.clone();
        tauri::async_runtime::spawn_blocking(move || {
            if let Err(e) = app.core.initialize() {
                app.runtime.report(e);
            }
            app.updates.initialize(&handle);
            if !automatic {
                return;
            }
            let weak = Arc::downgrade(&app);
            let wake = app.wake.clone();
            drop(app);
            tauri::async_runtime::spawn(async move {
                let app_check_after = tokio::time::Instant::now() + Duration::from_secs(10);
                loop {
                    let Some(app) = weak.upgrade() else {
                        break;
                    };
                    if !app.gate.closing() {
                        app.core.tick();
                    }
                    if !app.gate.closing() && tokio::time::Instant::now() >= app_check_after {
                        app.updates.tick();
                    }
                    drop(app);
                    tokio::select! { _ = tokio::time::sleep(Duration::from_secs(60)) => {}, _ = wake.notified() => {} }
                }
            });
        });
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    Runtime,
    Selection,
    CoreFetch,
    Preferences,
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
    pub fn runtime_other(&self, id: u64) -> bool {
        self.0
            .lock()
            .unwrap()
            .active
            .iter()
            .any(|(key, a)| *key != id && matches!(a.kind, Kind::Runtime | Kind::Selection))
    }
    pub fn wait_runtime(&self, id: u64) -> Result<(), String> {
        let deadline = Instant::now() + Duration::from_secs(25);
        while self.runtime_other(id) {
            if Instant::now() >= deadline {
                return Err("核心操作尚未收尾，请重试".into());
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        Ok(())
    }
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
            (Kind::CoreFetch, Kind::CoreFetch)
            | (Kind::Preferences, Kind::Preferences)
            | (Kind::Diagnostic, Kind::Diagnostic) => true,
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
    pub fn cancel_core(&self) {
        self.cancel_kind(|k| k == Kind::CoreFetch);
    }
    pub fn cancel_runtime(&self) {
        self.cancel_kind(|k| k == Kind::Runtime);
    }
    pub fn cancel_kind(&self, matches: impl Fn(Kind) -> bool) {
        for active in self.0.lock().unwrap().active.values() {
            if matches(active.kind) {
                active.cancel.store(true, Ordering::Release);
            }
        }
    }
    pub fn core_fetching(&self) -> bool {
        self.0
            .lock()
            .unwrap()
            .active
            .values()
            .any(|v| v.kind == Kind::CoreFetch)
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
        let _ = install;
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
