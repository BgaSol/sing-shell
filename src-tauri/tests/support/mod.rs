#![allow(dead_code)]
use sing_shell::{
    logs::Log,
    runtime::RuntimeService,
    settings::{Settings, SettingsStore},
};
use std::{
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
};
pub struct TestCore {
    pub runtime: RuntimeService,
    pub store: SettingsStore,
}
pub struct Snapshot {
    pub status: String,
    pub pid: Option<u32>,
    pub error: Option<String>,
    pub settings: Settings,
    pub logs: Vec<Log>,
}
impl TestCore {
    pub fn open(root: PathBuf) -> Result<Self, String> {
        Ok(Self {
            runtime: Default::default(),
            store: SettingsStore::open(root)?,
        })
    }
    pub fn select_core(&self, path: &Path) -> Result<(), String> {
        self.runtime
            .select(&self.store, path, &AtomicBool::new(false))
    }
    pub fn import_config(&self, path: &Path) -> Result<(), String> {
        self.runtime
            .import(&self.store, path, &AtomicBool::new(false))
    }
    pub fn start(&self) -> Result<(), String> {
        self.runtime.start(&self.store, &AtomicBool::new(false))
    }
    pub fn stop(&self) -> Result<(), String> {
        self.runtime.stop()
    }
    pub fn snapshot(&self) -> Result<Snapshot, String> {
        let runtime = self.runtime.snapshot();
        Ok(Snapshot {
            status: if self.store.error.is_some() {
                "failed".into()
            } else {
                runtime.status
            },
            pid: runtime.pid,
            error: self.store.error.clone().or(runtime.error),
            settings: self.store.snapshot(),
            logs: self.runtime.logs.snapshot(),
        })
    }
}
