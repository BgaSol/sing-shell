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
}
#[derive(Debug)]
pub enum FetchError {
    Network(String),
    Package(String),
    Local(String),
    Cancelled,
}
pub struct ManagedCore {
    pub store: Arc<SettingsStore>,
    pub runtime: Arc<RuntimeService>,
    state: Mutex<Snapshot>,
    pub prepared: Mutex<Option<Prepared>>,
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
        self.state.lock().unwrap().clone()
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
        fs::create_dir_all(self.cache()).map_err(|e| e.to_string())?;
        if let Ok(bytes) = fs::read(self.cache().join("state.json")) {
            if let Ok(mut s) = serde_json::from_slice::<Snapshot>(&bytes) {
                s.phase = Phase::Idle;
                s.network = None;
                s.next_check_at = s.next_check_at.min(now() + 86_400_000);
                *self.state.lock().unwrap() = s;
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
        let c = p.reference.source.as_ref().ok_or("准备记录缺少来源")?;
        let archive = self.cache().join("package.bin");
        if package::hash_cancel(&archive, cancel)? != c.sha256
            || fs::metadata(&archive).map_err(|e| e.to_string())?.len() != c.size
        {
            return Err("缓存归档损坏".into());
        }
        let installed = crate::settings::managed_path(&self.store.root, &p.reference)?;
        let staging = package::staged_path(&self.store.root, &p.reference)?;
        let program = if staging.exists() { staging } else { installed };
        package::owned(&self.store.root, &program)?;
        package::verify_path(&self.runtime, &p.reference, &program, cancel)?;
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
        write_json(&path, &p)?;
        *self.prepared.lock().unwrap() = Some(p);
        Ok(())
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
        fs::create_dir_all(self.cache()).map_err(|e| FetchError::Local(e.to_string()))?;
        self.update(|s| s.phase = Phase::Preparing);
        let reference =
            package::prepare(&self.store.root, &self.runtime, candidate, archive, cancel).map_err(
                |e| {
                    if cancel.load(Ordering::Acquire) {
                        FetchError::Cancelled
                    } else {
                        FetchError::Package(e)
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
        write_json(&self.cache().join("prepared.json"), &p).map_err(FetchError::Local)?;
        *self.prepared.lock().unwrap() = Some(p.clone());
        self.update(|s| {
            s.phase = Phase::Ready;
            s.error = None;
        });
        Ok(p)
    }
    pub fn clear_ready(&self) {
        *self.prepared.lock().unwrap() = None;
        let _ = fs::remove_file(self.cache().join("prepared.json"));
        self.update(|s| {
            s.phase = Phase::Idle;
            s.error = None;
        });
    }
    pub fn error(&self, error: String) {
        self.runtime.logs.push("core-update", error.clone());
        self.update(|s| {
            s.phase = Phase::Error;
            s.error = Some(error);
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
