use super::{
    protocol::{exchange, Request},
    Phase, Snapshot,
};
use crate::platform::authorization;
use crate::{
    core::{
        material::{self, Envelope},
        package,
    },
    runtime::RuntimeService,
    settings::{self, ManagedRef},
};
use std::{
    fs::{self, File},
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
};
use std::{
    sync::Mutex,
    time::{Duration, Instant},
};
pub(super) struct Transfer {
    path: PathBuf,
    file: Option<File>,
    pub received: u64,
    size: u64,
    version: String,
    envelope: Envelope,
}
impl Drop for Transfer {
    fn drop(&mut self) {
        self.file.take();
        let _ = fs::remove_file(&self.path);
    }
}
pub(super) fn existing(
    root: &Path,
    version: &str,
    runtime: &RuntimeService,
    cancel: &AtomicBool,
) -> Result<PathBuf, String> {
    crate::profile::tun::require_version(version)?;
    let record = root.join(format!("trusted-{version}.json"));
    package::owned(root, &record)?;
    let reference: ManagedRef = serde_json::from_slice(
        &fs::read(record).map_err(|_| "受保护核心尚未安装，请重试准备核心")?,
    )
    .map_err(|_| "受保护核心记录损坏")?;
    if reference.version != version || reference.source.is_none() || reference.hash.is_none() {
        return Err("受保护核心来源无效".into());
    }
    package::verify(root, runtime, &reference, cancel)?;
    settings::managed_path(root, &reference)
}
impl Transfer {
    pub fn begin(
        root: &Path,
        version: &str,
        envelope: Envelope,
        runtime: &RuntimeService,
    ) -> Result<Option<Self>, String> {
        crate::profile::tun::require_version(version)?;
        let asset = envelope.verify(version)?;
        let candidate = envelope.candidate(version)?;
        let reference = ManagedRef {
            version: version.into(),
            target: crate::TARGET.into(),
            hash: Some(asset.program_sha256),
            source: Some(candidate),
        };
        let record = root.join(format!("trusted-{version}.json"));
        package::owned(root, &record)?;
        if let Ok(bytes) = fs::read(&record) {
            if let Ok(previous) = serde_json::from_slice::<ManagedRef>(&bytes) {
                previous.validate()?;
                if previous.version != version
                    || previous.hash.as_ref() != reference.hash.as_ref()
                    || previous.source.as_ref().is_none_or(|source| {
                        !crate::core::same_asset(source, reference.source.as_ref().unwrap())
                    })
                {
                    return Err("同版本受保护核心来源与签名材料冲突，保留现有程序".into());
                }
            }
        }
        let installed = settings::managed_path(root, &reference)?;
        package::owned(root, &installed)?;
        if material::program(&envelope, version, &installed, &AtomicBool::new(false)).is_ok() {
            package::verify_path(runtime, &reference, &installed, &AtomicBool::new(false))?;
            settings::write_json(&root.join(format!("trusted-{version}.json")), &reference)?;
            return Ok(None);
        }
        if runtime.snapshot().pid.is_some() {
            return Err("运行中的受保护核心不可修复，请先断开连接".into());
        }
        crate::private_files::directory(installed.parent().ok_or("核心安装目录缺失")?)?;
        let path = installed.with_extension("upload");
        package::owned(root, &path)?;
        if path.exists() {
            fs::remove_file(&path).map_err(|e| e.to_string())?;
        }
        crate::private_files::write(&path, &[])?;
        let file = File::options()
            .write(true)
            .open(&path)
            .map_err(|e| e.to_string())?;
        Ok(Some(Self {
            path,
            file: Some(file),
            received: 0,
            size: asset.program_size,
            version: version.into(),
            envelope,
        }))
    }
    pub fn chunk(&mut self, offset: u64, bytes: &[u8]) -> Result<(), String> {
        if bytes.is_empty()
            || bytes.len() > 64 * 1024
            || offset != self.received
            || self.received + bytes.len() as u64 > self.size
        {
            return Err("核心传输分块大小或顺序无效".into());
        }
        self.file
            .as_mut()
            .ok_or("核心传输已结束")?
            .write_all(bytes)
            .map_err(|e| e.to_string())?;
        self.received += bytes.len() as u64;
        Ok(())
    }
    pub fn finish(mut self, root: &Path, runtime: &RuntimeService) -> Result<(), String> {
        if self.received != self.size {
            return Err("核心传输未完整结束".into());
        }
        self.file
            .take()
            .ok_or("核心传输已结束")?
            .sync_all()
            .map_err(|e| e.to_string())?;
        let cancel = AtomicBool::new(false);
        material::program(&self.envelope, &self.version, &self.path, &cancel)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&self.path, fs::Permissions::from_mode(0o700))
                .map_err(|e| e.to_string())?;
        }
        let reference = ManagedRef {
            version: self.version.clone(),
            target: crate::TARGET.into(),
            hash: Some(package::hash(&self.path)?),
            source: Some(self.envelope.candidate(&self.version)?),
        };
        package::verify_path(runtime, &reference, &self.path, &cancel)?;
        let installed = settings::managed_path(root, &reference)?;
        package::owned(root, &installed)?;
        fs::rename(&self.path, &installed).map_err(|e| e.to_string())?;
        settings::write_json(
            &root.join(format!("trusted-{}.json", self.version)),
            &reference,
        )
    }
}

pub(super) fn material_for(
    root: &std::path::Path,
    version: &str,
) -> Option<crate::core::material::Envelope> {
    let saved: Option<crate::settings::Settings> = std::fs::read(root.join("settings.json"))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok());
    saved
        .and_then(|s| {
            [s.managed.active, s.managed.previous]
                .into_iter()
                .flatten()
                .find(|r| r.version == version)
                .and_then(|r| r.source)
                .and_then(|c| crate::core::material::for_candidate(&c).ok())
        })
        .or_else(|| crate::core::material::bundled(version))
}
pub(super) async fn provision(
    channel: &mut authorization::Channel,
    root: &std::path::Path,
    version: &str,
    material: Option<crate::core::material::Envelope>,
    cancel: &AtomicBool,
    state: &Mutex<Snapshot>,
    operation_id: u64,
) -> Result<(), String> {
    let mut response = exchange(
        channel,
        &Request::CoreBegin {
            version: version.into(),
            material: material.clone(),
            operation_id,
        },
    )
    .await?;
    if let Some(error) = response.error {
        return Err(format!("准备受保护核心：{error}"));
    }
    if response.core_received.is_none() {
        return Ok(());
    }
    let envelope = material.ok_or("缺少核心签名材料")?;
    let asset = envelope.verify(version)?;
    let reference = crate::settings::ManagedRef {
        version: version.into(),
        target: crate::TARGET.into(),
        hash: None,
        source: None,
    };
    let path = crate::settings::managed_path(root, &reference)?;
    let mut file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let deadline = Instant::now() + Duration::from_secs(120);
    let mut offset = 0;
    let mut buffer = vec![0; 64 * 1024];
    loop {
        crate::core::package::cancelled(cancel)?;
        if Instant::now() >= deadline {
            return Err("本地核心传输超时".into());
        }
        use std::io::Read;
        let count = file.read(&mut buffer).map_err(|e| e.to_string())?;
        if count == 0 {
            break;
        }
        if offset + count as u64 > asset.program_size {
            return Err("本地核心程序大小改变".into());
        }
        response = exchange(
            channel,
            &Request::CoreChunk {
                offset,
                bytes: buffer[..count].to_vec(),
            },
        )
        .await?;
        if let Some(error) = response.error {
            return Err(format!("安装受保护核心：{error}"));
        }
        offset += count as u64;
        if response.runtime.pid.is_none() {
            response.phase = Phase::Preparing;
        }
        response.operation = Some("installingCore".into());
        *state.lock().unwrap() = response.clone();
    }
    crate::core::package::cancelled(cancel)?;
    response = exchange(channel, &Request::CoreFinish).await?;
    if let Some(error) = response.error {
        return Err(format!("校验受保护核心：{error}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "requires SING_SHELL_TEST_CORE official program"]
    fn first_install_is_offline_and_rejects_tamper_before_execution() {
        let program = PathBuf::from(std::env::var_os("SING_SHELL_TEST_CORE").unwrap());
        let version = std::env::var("CORE_VERSION").unwrap_or("1.14.4".into());
        let envelope: Envelope = std::env::var_os("CORE_MATERIAL")
            .map(|p| serde_json::from_slice(&fs::read(p).unwrap()).unwrap())
            .unwrap_or_else(|| material::bundled(&version).unwrap());
        let runtime = RuntimeService::default();
        let root = std::env::temp_dir().join(format!(
            "sing-shell-offline-install-{}-{}",
            std::process::id(),
            crate::logs::now()
        ));
        crate::private_files::directory(&root).unwrap();
        let bytes = fs::read(&program).unwrap();
        let cancel = AtomicBool::new(false);
        let mut transfer = Transfer::begin(&root, &version, envelope.clone(), &runtime)
            .unwrap()
            .unwrap();
        assert!(transfer.chunk(1, &bytes[..64]).is_err());
        transfer.chunk(0, &[0; 1024]).unwrap();
        assert!(transfer.finish(&root, &runtime).is_err());
        assert!(existing(&root, &version, &runtime, &cancel).is_err());
        let mut transfer = Transfer::begin(&root, &version, envelope.clone(), &runtime)
            .unwrap()
            .unwrap();
        for chunk in bytes.chunks(64 * 1024) {
            transfer.chunk(transfer.received, chunk).unwrap();
        }
        transfer.finish(&root, &runtime).unwrap();
        let installed = existing(&root, &version, &runtime, &cancel).unwrap();
        assert_eq!(
            package::hash(&installed).unwrap(),
            envelope.verify(&version).unwrap().program_sha256
        );
        assert!(Transfer::begin(&root, &version, envelope.clone(), &runtime)
            .unwrap()
            .is_none());
        let record = root.join(format!("trusted-{version}.json"));
        let original = fs::read(&record).unwrap();
        let mut changed: ManagedRef = serde_json::from_slice(&original).unwrap();
        changed.source.as_mut().unwrap().asset_id += 1;
        settings::write_json(&record, &changed).unwrap();
        assert!(Transfer::begin(&root, &version, envelope.clone(), &runtime).is_err());
        assert_eq!(
            package::hash(&installed).unwrap(),
            envelope.verify(&version).unwrap().program_sha256
        );
        fs::write(&record, original).unwrap();
        let mut tampered = bytes.clone();
        tampered[512] ^= 1;
        fs::write(&installed, tampered).unwrap();
        assert!(existing(&root, &version, &runtime, &cancel).is_err());
        let transfer = Transfer::begin(&root, &version, envelope.clone(), &runtime)
            .unwrap()
            .unwrap();
        let temporary = transfer.path.clone();
        drop(transfer);
        assert!(!temporary.exists());
        fs::write(&installed, &bytes).unwrap();
        let config = root.join("raw-check.json");
        fs::write(&config, r#"{"outbounds":[{"type":"direct"}]}"#).unwrap();
        let mut worker = super::super::worker::Worker::new(root.clone(), "a".repeat(64));
        worker
            .runtime
            .start(
                &crate::runtime::StartSpec {
                    executable: installed.clone(),
                    config,
                    version: Some(version.clone()),
                },
                std::sync::Arc::new(AtomicBool::new(false)),
            )
            .unwrap();
        worker.state.lock().unwrap().phase = Phase::Running;
        let pid = worker.runtime.snapshot().pid;
        assert!(pid.is_some());
        worker.begin_core(&version, Some(envelope), 42).unwrap();
        assert_eq!(worker.snapshot().phase,Phase::Running,"installation recheck must preserve the real running state even when stored runtime fields are empty");
        assert_eq!(worker.runtime.snapshot().pid, pid);
        worker.stop().unwrap();
        fs::remove_dir_all(root).unwrap();
    }
}
