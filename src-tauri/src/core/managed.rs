use super::package;
use crate::{
    app::Operation,
    logs::now,
    runtime::RuntimeService,
    settings::{write_json, ManagedRef, SettingsStore},
    updates::{
        network::{self, Transport},
        release::{self, Candidate},
    },
};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum Phase {
    #[default]
    Idle,
    Checking,
    Downloading,
    Preparing,
    Ready,
    Paused,
    Error,
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum Origin {
    Automatic,
    Manual,
    Required,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Prepared {
    pub schema_version: u32,
    pub reference: ManagedRef,
    pub origin: Origin,
    pub generation: u64,
}
#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub phase: Phase,
    pub candidate: Option<Candidate>,
    pub error: Option<String>,
    pub downloaded_bytes: u64,
    pub total_bytes: Option<u64>,
    pub network: Option<network::Attempt>,
    pub last_checked_at: u64,
    pub next_check_at: u64,
    #[serde(default)]
    pub failure_kind: Option<FailureKind>,
    #[serde(default)]
    pub warning: Option<String>,
    #[serde(default)]
    pub prepared_version: Option<String>,
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum FailureKind {
    Network,
    Package,
    Cache,
    Local,
    Compatibility,
    Conflict,
}
#[derive(Debug)]
pub enum FetchError {
    Network(String),
    Package(String),
    Local(String),
    Conflict(String),
    Cancelled,
}
impl FetchError {
    pub fn message(self) -> String {
        match self {
            Self::Network(e) | Self::Package(e) | Self::Local(e) | Self::Conflict(e) => e,
            Self::Cancelled => "操作已取消".into(),
        }
    }
}
pub struct ManagedCore {
    pub store: Arc<SettingsStore>,
    pub runtime: Arc<RuntimeService>,
    state: Mutex<Snapshot>,
    prepared: Mutex<Option<Prepared>>,
    pub network: Arc<network::State>,
    pub suppressed: AtomicBool,
    writer: Mutex<()>,
    #[cfg(feature = "test-fixtures")]
    pub test_transport: Mutex<Option<Transport>>,
}
impl ManagedCore {
    pub fn new(store: Arc<SettingsStore>, runtime: Arc<RuntimeService>) -> Self {
        Self {
            store,
            runtime,
            state: Mutex::new(Default::default()),
            prepared: Mutex::new(None),
            network: Arc::new(Default::default()),
            suppressed: AtomicBool::new(false),
            writer: Mutex::new(()),
            #[cfg(feature = "test-fixtures")]
            test_transport: Mutex::new(None),
        }
    }
    pub fn snapshot(&self) -> Snapshot {
        let mut s = self.state.lock().unwrap().clone();
        s.prepared_version = self.ready().map(|p| p.reference.version);
        s
    }
    pub fn update(&self, f: impl FnOnce(&mut Snapshot)) {
        f(&mut self.state.lock().unwrap());
    }
    pub fn cache(&self) -> PathBuf {
        self.store.root.join("updates/engine")
    }
    pub fn ready(&self) -> Option<Prepared> {
        self.prepared.lock().unwrap().clone()
    }
    pub fn save_prepared(&self, p: Prepared) -> Result<(), String> {
        write_json(&self.cache().join("prepared.json"), &p)?;
        *self.prepared.lock().unwrap() = Some(p);
        Ok(())
    }
    pub fn resume(&self, generation: u64) -> Result<(), String> {
        if let Some(mut p) = self.ready() {
            p.generation = generation;
            p.origin = Origin::Manual;
            self.save_prepared(p)?;
        }
        self.suppressed.store(false, Ordering::Release);
        let prepared = self.ready().is_some();
        self.update(|s| {
            s.error = None;
            s.failure_kind = None;
            s.phase = if prepared { Phase::Ready } else { Phase::Idle };
        });
        Ok(())
    }
    pub fn persist(&self) {
        let _writer = self.writer.lock().unwrap();
        let mut s = self.snapshot();
        s.phase = if self.ready().is_some() {
            Phase::Ready
        } else {
            Phase::Idle
        };
        s.network = None;
        if let Err(e) = write_json(&self.cache().join("state.json"), &s) {
            self.runtime.logs.push("core-update", e);
        }
    }
    pub fn initialize(&self, generation: u64, cancel: &AtomicBool) -> Result<(), String> {
        *self.prepared.lock().unwrap() = None;
        *self.state.lock().unwrap() = Snapshot::default();
        fs::create_dir_all(self.cache()).map_err(|e| e.to_string())?;
        if let Ok(bytes) = fs::read(self.cache().join("state.json")) {
            if let Ok(s) = serde_json::from_slice::<Snapshot>(&bytes) {
                self.update(|current| {
                    current.last_checked_at = s.last_checked_at;
                    current.next_check_at = s.next_check_at.min(now() + 86_400_000);
                });
            }
        }
        let path = self.cache().join("prepared.json");
        if !path.exists() {
            return Ok(());
        }
        let bytes = fs::read(&path).map_err(|e| e.to_string())?;
        let mut p = match serde_json::from_slice::<Prepared>(&bytes) {
            Ok(p) => p,
            Err(_) => {
                #[derive(Deserialize)]
                struct Old {
                    schema_version: u32,
                    candidate: Candidate,
                    engine_hash: Option<String>,
                }
                let old: Old = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
                if old.schema_version != 1 {
                    return Err("不支持的核心缓存版本".into());
                }
                release::validate_engine(&old.candidate)?;
                Prepared {
                    schema_version: 2,
                    reference: ManagedRef {
                        version: old.candidate.version.clone(),
                        target: old.candidate.target.clone(),
                        hash: old.engine_hash,
                        source: Some(old.candidate),
                    },
                    origin: Origin::Automatic,
                    generation,
                }
            }
        };
        if p.schema_version != 2 {
            return Err("核心缓存格式不符".into());
        }
        p.reference.validate()?;
        let c = p.reference.source.clone().ok_or("准备记录缺少来源")?;
        self.update(|s| s.candidate = Some(c.clone()));
        let archive = self.cache().join("package.bin");
        if package::hash_cancel(&archive, cancel)? != c.sha256
            || fs::metadata(&archive).map_err(|e| e.to_string())?.len() != c.size
        {
            return Err("缓存归档损坏".into());
        }
        self.restore_program(&mut p, cancel)?;
        p.generation = generation;
        if self.store.snapshot().managed() && self.store.snapshot().managed.active.is_none() {
            p.origin = Origin::Required;
        }
        // A former manual takeover must not survive as consent to replace an external selection.
        if !self.store.snapshot().managed() {
            p.origin = Origin::Automatic;
        }
        self.update(|s| {
            s.phase = Phase::Ready;
            s.candidate = Some(c.clone());
        });
        self.save_prepared(p)?;
        Ok(())
    }
    // A valid archive can rebuild staging; an installed file alone never invents its source.
    pub fn restore_program(
        &self,
        p: &mut Prepared,
        cancel: &AtomicBool,
    ) -> Result<PathBuf, String> {
        p.reference.validate()?;
        let installed = crate::settings::managed_path(&self.store.root, &p.reference)?;
        let staging = package::staged_path(&self.store.root, &p.reference)?;
        for program in [&staging, &installed] {
            package::owned(&self.store.root, program)?;
            if program.is_file()
                && package::verify_path(&self.runtime, &p.reference, program, cancel).is_ok()
            {
                return Ok(program.clone());
            }
            package::cancelled(cancel)?;
        }
        let candidate = p.reference.source.as_ref().ok_or("准备记录缺少来源")?;
        p.reference = package::prepare(
            &self.store.root,
            &self.runtime,
            candidate,
            &self.cache().join("package.bin"),
            cancel,
        )
        .map_err(String::from)?;
        Ok(staging)
    }
    pub fn transport(self: &Arc<Self>, op: &Operation) -> Result<Transport, String> {
        let weak = Arc::downgrade(self);
        let cancel = op.cancel.clone();
        #[cfg(feature = "test-fixtures")]
        if let Some(client) = self.test_transport.lock().unwrap().clone() {
            return Ok(client.observed(
                Arc::new(move |a| {
                    if let Some(m) = weak.upgrade() {
                        m.update(|s| s.network = Some(a));
                    }
                }),
                Arc::new(move || cancel.load(Ordering::Acquire)),
            ));
        }
        Transport::new(
            self.store.snapshot().update_preferences.network,
            self.network.clone(),
        )
        .map(|t| {
            t.observed(
                Arc::new(move |a| {
                    if let Some(m) = weak.upgrade() {
                        m.update(|s| s.network = Some(a));
                    }
                }),
                Arc::new(move || cancel.load(Ordering::Acquire)),
            )
        })
    }
    pub fn prepare_with(
        &self,
        client: &Transport,
        candidate: Candidate,
        origin: Origin,
        generation: u64,
        cancel: &AtomicBool,
    ) -> Result<Prepared, FetchError> {
        release::validate_engine(&candidate).map_err(FetchError::Package)?;
        package::cancelled(cancel).map_err(|_| FetchError::Cancelled)?;
        self.update(|s| {
            s.phase = Phase::Downloading;
            s.candidate = Some(candidate.clone());
            s.error = None;
            s.failure_kind = None;
        });
        fs::create_dir_all(self.cache()).map_err(|e| FetchError::Local(e.to_string()))?;
        let part = self.cache().join("package.part");
        if part.exists() {
            fs::remove_file(&part).map_err(|e| FetchError::Local(e.to_string()))?;
        }
        let bad = AtomicBool::new(false);
        client
            .download_result(
                &candidate.url,
                &part,
                |bytes, total| {
                    self.update(|s| {
                        s.downloaded_bytes = bytes;
                        s.total_bytes = total;
                    })
                },
                |path, hash| {
                    if hash != candidate.sha256
                        || fs::metadata(path)
                            .map_err(|e| network::Error::Local(e.to_string()))?
                            .len()
                            != candidate.size
                    {
                        bad.store(true, Ordering::Release);
                        return Err(network::Error::remote("核心归档摘要或大小不符"));
                    }
                    Ok(())
                },
            )
            .map_err(|e| match e {
                network::Error::Cancelled => FetchError::Cancelled,
                network::Error::Local(e) => FetchError::Local(e),
                _ if bad.load(Ordering::Acquire) => FetchError::Package(e.message()),
                _ => FetchError::Network(e.message()),
            })?;
        if cancel.load(Ordering::Acquire) {
            let _ = fs::remove_file(&part);
            return Err(FetchError::Cancelled);
        }
        self.stage_archive(&candidate, &part, origin, generation, cancel)
    }
    pub fn stage_archive(
        &self,
        candidate: &Candidate,
        archive: &std::path::Path,
        origin: Origin,
        generation: u64,
        cancel: &AtomicBool,
    ) -> Result<Prepared, FetchError> {
        if self.ready().is_some() {
            return Err(FetchError::Local("已有核心准备结果，请先完成或清理".into()));
        }
        self.clear_ready().map_err(FetchError::Local)?;
        fs::create_dir_all(self.cache()).map_err(|e| FetchError::Local(e.to_string()))?;
        self.update(|s| s.phase = Phase::Preparing);
        let reference =
            package::prepare(&self.store.root, &self.runtime, candidate, archive, cancel).map_err(
                |e| {
                    if cancel.load(Ordering::Acquire) {
                        FetchError::Cancelled
                    } else {
                        match e {
                            package::PrepareError::Local(e) => FetchError::Local(e),
                            package::PrepareError::Invalid(e) => FetchError::Package(e),
                            package::PrepareError::Cancelled => FetchError::Cancelled,
                        }
                    }
                },
            )?;
        package::cancelled(cancel).map_err(|_| FetchError::Cancelled)?;
        let part = self.cache().join("package.part");
        if archive != part {
            let mut input =
                fs::File::open(archive).map_err(|e| FetchError::Local(e.to_string()))?;
            let mut output =
                fs::File::create(&part).map_err(|e| FetchError::Local(e.to_string()))?;
            let mut buffer = [0; 65536];
            loop {
                use std::io::{Read, Write};
                package::cancelled(cancel).map_err(|_| FetchError::Cancelled)?;
                let n = input
                    .read(&mut buffer)
                    .map_err(|e| FetchError::Local(e.to_string()))?;
                if n == 0 {
                    break;
                }
                output
                    .write_all(&buffer[..n])
                    .map_err(|e| FetchError::Local(e.to_string()))?;
            }
            output
                .sync_all()
                .map_err(|e| FetchError::Local(e.to_string()))?;
        }
        fs::rename(&part, self.cache().join("package.bin"))
            .map_err(|e| FetchError::Local(e.to_string()))?;
        let p = Prepared {
            schema_version: 2,
            reference,
            origin,
            generation,
        };
        self.save_prepared(p.clone()).map_err(FetchError::Local)?;
        self.update(|s| {
            s.phase = Phase::Ready;
            s.error = None;
            s.failure_kind = None;
        });
        Ok(p)
    }
    pub fn clear_ready(&self) -> Result<(), String> {
        match fs::remove_file(self.cache().join("prepared.json")) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.to_string()),
        }
        *self.prepared.lock().unwrap() = None;
        self.update(|s| {
            s.phase = Phase::Idle;
            s.error = None;
            s.failure_kind = None;
        });
        Ok(())
    }
    pub fn consume_ready(&self) {
        let prepared = self.ready();
        if let Err(e) = self.clear_ready() {
            *self.prepared.lock().unwrap() = None;
            self.update(|s| {
                s.phase = Phase::Idle;
                s.error = None;
                s.failure_kind = None;
            });
            self.warn(format!("核心已采用，准备记录清理未完成：{e}"));
        }
        if let Some(p) = prepared {
            if let Ok(path) = package::staged_path(&self.store.root, &p.reference) {
                if let Err(e) = package::owned(&self.store.root, &path).and_then(|_| {
                    match fs::remove_file(&path) {
                        Ok(()) => Ok(()),
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                        Err(e) => Err(e.to_string()),
                    }
                }) {
                    self.warn(format!("核心已采用，临时程序清理未完成：{e}"));
                }
            }
        }
    }
    pub fn warn(&self, message: String) {
        self.runtime.logs.push("core-update", message.clone());
        self.update(|s| s.warning = Some(message));
    }
    pub fn fail(&self, kind: FailureKind, error: String) {
        self.error(error);
        self.update(|s| s.failure_kind = Some(kind));
    }
    pub fn error(&self, error: String) {
        if self.state.lock().unwrap().error.as_ref() == Some(&error) {
            return;
        }
        self.runtime.logs.push("core-update", error.clone());
        self.update(|s| {
            s.phase = Phase::Error;
            s.error = Some(error);
            s.failure_kind = None;
        });
    }
    pub fn checked(&self, at: u64, failed: bool) {
        self.update(|s| {
            if !failed {
                s.last_checked_at = at;
            }
            s.next_check_at = at + if failed { 900_000 } else { 86_400_000 };
        });
    }
}
