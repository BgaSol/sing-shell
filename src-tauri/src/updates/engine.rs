use super::release::Candidate;
use crate::{
    runtime::RuntimeService,
    settings::{Engine, SettingsStore},
};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::{Read, Write},
    path::{Component, Path},
    sync::atomic::AtomicBool,
};
pub fn hash(path: &Path) -> Result<String, String> {
    let mut f = File::open(path).map_err(|e| e.to_string())?;
    let mut hash = Sha256::new();
    let mut buf = [0; 65536];
    loop {
        let n = f.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        hash.update(&buf[..n]);
    }
    Ok(hash
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}
pub fn verify_binary(path: &Path, target: &str) -> Result<(), String> {
    let mut file = File::open(path).map_err(|e| e.to_string())?;
    let mut bytes = vec![0; 512];
    file.read_exact(&mut bytes).map_err(|e| e.to_string())?;
    let arm = target.starts_with("aarch64-");
    let valid = if target.ends_with("windows-msvc") {
        let offset = u32::from_le_bytes(bytes[60..64].try_into().unwrap()) as usize;
        offset + 6 <= bytes.len()
            && &bytes[..2] == b"MZ"
            && &bytes[offset..offset + 4] == b"PE\0\0"
            && u16::from_le_bytes(bytes[offset + 4..offset + 6].try_into().unwrap())
                == if arm { 0xaa64 } else { 0x8664 }
    } else if target.ends_with("linux-gnu") {
        &bytes[..4] == b"\x7fELF"
            && bytes[4] == 2
            && bytes[5] == 1
            && u16::from_le_bytes(bytes[18..20].try_into().unwrap()) == if arm { 183 } else { 62 }
    } else {
        u32::from_le_bytes(bytes[..4].try_into().unwrap()) == 0xfeedfacf
            && u32::from_le_bytes(bytes[4..8].try_into().unwrap()) == 0x100000c
    };
    if valid {
        Ok(())
    } else {
        Err("核心不是本机原生架构程序".into())
    }
}
fn safe(path: &Path) -> Result<(), String> {
    if path
        .components()
        .any(|c| !matches!(c, Component::Normal(_)))
        || path.to_string_lossy().contains(['\\', ':'])
    {
        return Err("核心归档含不安全路径".into());
    }
    Ok(())
}
fn save(output: &Path, input: &mut impl Read, size: u64) -> Result<(), String> {
    if size > 256 * 1024 * 1024 {
        return Err("核心展开文件过大".into());
    }
    let mut file = File::create(output).map_err(|e| e.to_string())?;
    let count = std::io::copy(&mut input.take(size + 1), &mut file).map_err(|e| e.to_string())?;
    if count != size {
        return Err("核心解包大小不符".into());
    }
    file.flush()
        .and_then(|_| file.sync_all())
        .map_err(|e| e.to_string())
}
pub fn unpack(archive: &Path, output: &Path, windows: bool) -> Result<(), String> {
    let expected = if windows { "sing-box.exe" } else { "sing-box" };
    let mut found = false;
    let mut total = 0u64;
    if windows {
        let mut zip = zip::ZipArchive::new(File::open(archive).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        if zip.len() > 128 {
            return Err("归档条目过多".into());
        }
        for i in 0..zip.len() {
            let mut item = zip.by_index(i).map_err(|e| e.to_string())?;
            let name = item.name().map_err(|e| e.to_string())?.into_owned();
            let path = Path::new(&name);
            safe(path)?;
            if item.unix_mode().is_some_and(|m| m & 0o170000 == 0o120000) {
                return Err("核心归档不能包含链接".into());
            }
            total = total.checked_add(item.size()).ok_or("归档过大")?;
            if total > 1024 * 1024 * 1024 {
                return Err("归档展开总量过大".into());
            }
            if path.file_name().is_some_and(|v| v == expected) {
                if found || item.is_dir() {
                    return Err("核心入口重复或无效".into());
                }
                let size = item.size();
                save(output, &mut item, size)?;
                found = true;
            }
        }
    } else {
        let gzip = flate2::read::GzDecoder::new(File::open(archive).map_err(|e| e.to_string())?);
        let mut tar = tar::Archive::new(gzip);
        for (index, item) in tar.entries().map_err(|e| e.to_string())?.enumerate() {
            if index >= 128 {
                return Err("归档条目过多".into());
            }
            let mut item = item.map_err(|e| e.to_string())?;
            let path = item.path().map_err(|e| e.to_string())?.into_owned();
            safe(&path)?;
            if !item.header().entry_type().is_file() && !item.header().entry_type().is_dir() {
                return Err("核心归档含链接或特殊文件".into());
            }
            total = total.checked_add(item.size()).ok_or("归档过大")?;
            if total > 1024 * 1024 * 1024 {
                return Err("归档展开总量过大".into());
            }
            if path.file_name().is_some_and(|v| v == expected) {
                if found || !item.header().entry_type().is_file() {
                    return Err("核心入口重复或无效".into());
                }
                let size = item.size();
                save(output, &mut item, size)?;
                found = true;
            }
        }
    }
    if !found {
        return Err("归档缺少 sing-box 程序".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(output, fs::Permissions::from_mode(0o700))
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}
pub fn prepare(
    store: &SettingsStore,
    runtime: &RuntimeService,
    candidate: &Candidate,
    archive: &Path,
    cancel: &AtomicBool,
) -> Result<String, String> {
    let engine = Engine::Managed {
        version: candidate.version.clone(),
        target: candidate.target.clone(),
    };
    let path = store.engine_path(&engine)?;
    let directory = path.parent().unwrap();
    fs::create_dir_all(directory).map_err(|e| e.to_string())?;
    let staging = directory.join("sing-box.part");
    let result = (|| {
        unpack(archive, &staging, cfg!(windows))?;
        verify_binary(&staging, crate::TARGET)?;
        if runtime.probe(&staging, cancel)? != candidate.version {
            return Err("核心版本与候选不符".into());
        }
        if let Some(config) = store.snapshot().config_path {
            runtime.validate(&staging, &config, cancel)?;
        }
        let digest = hash(&staging)?;
        if path.exists() {
            if hash(&path)? != digest {
                return Err("已有版本目录内容不一致".into());
            }
            fs::remove_file(&staging).map_err(|e| e.to_string())?;
        } else {
            fs::rename(&staging, &path).map_err(|e| e.to_string())?;
        }
        Ok(digest)
    })();
    if result.is_err() {
        let _ = fs::remove_file(staging);
    }
    result
}
pub fn verify(
    store: &SettingsStore,
    runtime: &RuntimeService,
    candidate: &Candidate,
    expected: &str,
    cancel: &AtomicBool,
) -> Result<(), String> {
    let path = store.engine_path(&Engine::Managed {
        version: candidate.version.clone(),
        target: candidate.target.clone(),
    })?;
    if hash(&path)? != expected {
        return Err("已准备核心被修改，请重新下载".into());
    }
    verify_binary(&path, crate::TARGET)?;
    if runtime.probe(&path, cancel)? != candidate.version {
        return Err("已准备核心版本不符".into());
    }
    if let Some(config) = store.snapshot().config_path {
        runtime.validate(&path, &config, cancel)?;
    }
    Ok(())
}

pub fn cleanup(store: &SettingsStore, current: &str, previous: Option<&str>) -> Result<(), String> {
    let root = store.root.join("engines/sing-box");
    for entry in fs::read_dir(&root).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name == current
            || previous == Some(name.as_str())
            || crate::settings::stable(&name).is_err()
            || !entry.file_type().map_err(|e| e.to_string())?.is_dir()
        {
            continue;
        }
        let path = entry.path();
        if path.parent() != Some(root.as_path()) {
            return Err("核心清理路径不符".into());
        }
        fs::remove_dir_all(path).map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unsafe_archive_paths_are_rejected() {
        for p in [
            "../bad",
            "/absolute",
            "C:/bad",
            "prefix/../../bad",
            "prefix\\bad",
        ] {
            assert!(safe(Path::new(p)).is_err(), "{p}");
        }
        assert!(safe(Path::new("sing-box-1.0.0/sing-box")).is_ok());
    }
}
