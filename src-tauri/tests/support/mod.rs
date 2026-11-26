#![allow(dead_code)]
use sing_shell::{
    app::Admission,
    core::CoreService,
    logs::Log,
    runtime::RuntimeService,
    settings::{Settings, SettingsStore},
};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};
pub struct TestCore {
    pub core: Arc<CoreService>,
    pub session: Arc<sing_shell::session::SessionService>,
    pub runtime: Arc<RuntimeService>,
    pub store: Arc<SettingsStore>,
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
        let store = Arc::new(SettingsStore::open(root)?);
        // Existing lifecycle fixtures exercise raw-config sessions with synthetic core versions.
        if store.error.is_none()
            && store.snapshot().profile.nodes.is_empty()
            && store.snapshot().profile.mode == sing_shell::profile::Mode::Generated
        {
            store.profile(store.snapshot().profile_revision, |p| {
                p.mode = sing_shell::profile::Mode::ExternalConfig;
                Ok(())
            })?;
        }
        let runtime = Arc::new(RuntimeService::default());
        let wake = Arc::new(tokio::sync::Notify::new());
        let core = CoreService::new(
            store.clone(),
            runtime.clone(),
            Arc::new(Admission::default()),
            wake.clone(),
        );
        let session = sing_shell::session::SessionService::new(core.clone(), wake);
        if let Err(e) = session.initialize() {
            runtime.report(e);
        }
        Ok(Self {
            core,
            session,
            runtime,
            store,
        })
    }
    pub fn select_core(&self, path: &Path) -> Result<(), String> {
        self.session.select_core(path)
    }
    pub fn import_config(&self, path: &Path) -> Result<(), String> {
        self.session.import_config(path)
    }
    pub fn start(&self) -> Result<(), String> {
        let result = self.session.start();
        if let Err(e) = &result {
            self.runtime.report(e.clone());
        }
        result
    }
    pub fn stop(&self) -> Result<(), String> {
        self.session.stop()
    }
    pub fn snapshot(&self) -> Result<Snapshot, String> {
        let r = self.runtime.snapshot();
        Ok(Snapshot {
            status: if self.store.error.is_some() {
                "failed".into()
            } else {
                r.status
            },
            pid: r.pid,
            error: self.store.error.clone().or(r.error),
            settings: self.store.snapshot(),
            logs: self.runtime.logs.snapshot(),
        })
    }
}
