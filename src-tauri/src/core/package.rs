use crate::updates::release::Candidate;
use crate::{
    runtime::RuntimeService,
    settings::{self, ManagedRef},
};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::{Read, Write},
    path::{Component, Path},
    sync::atomic::{AtomicBool, Ordering},
};
pub fn cancelled(cancel: &AtomicBool) -> Result<(), String> {
    if cancel.load(Ordering::Acquire) {
        Err("操作已取消".into())
    } else {
        Ok(())
    }
}
pub fn hash(path: &Path) -> Result<String, String> {
    hash_cancel(path, &AtomicBool::new(false))
}
pub fn hash_cancel(path: &Path, cancel: &AtomicBool) -> Result<String, String> {
    let mut f = File::open(path).map_err(|e| e.to_string())?;
    let mut hash = Sha256::new();
    let mut buf = [0; 65536];
    loop {
        cancelled(cancel)?;
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
fn save(
    output: &Path,
    input: &mut impl Read,
    size: u64,
    cancel: &AtomicBool,
) -> Result<(), String> {
    if size > 256 * 1024 * 1024 {
        return Err("核心展开文件过大".into());
    }
    let mut file = File::create(output).map_err(|e| e.to_string())?;
    let mut count = 0u64;
    let mut buffer = [0; 65536];
    loop {
        cancelled(cancel)?;
        let n = input.read(&mut buffer).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        count += n as u64;
        if count > size {
            return Err("核心解包超过声明大小".into());
        }
        file.write_all(&buffer[..n]).map_err(|e| e.to_string())?;
    }
    if count != size {
        return Err("核心解包大小不符".into());
    }
    file.flush()
        .and_then(|_| file.sync_all())
        .map_err(|e| e.to_string())
}
pub fn unpack(
    archive: &Path,
    output: &Path,
    windows: bool,
    cancel: &AtomicBool,
) -> Result<(), String> {
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
            cancelled(cancel)?;
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
                save(output, &mut item, size, cancel)?;
                found = true;
            }
        }
    } else {
        let gzip = flate2::read::GzDecoder::new(File::open(archive).map_err(|e| e.to_string())?);
        let mut tar = tar::Archive::new(gzip);
        for (index, item) in tar.entries().map_err(|e| e.to_string())?.enumerate() {
            cancelled(cancel)?;
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
                save(output, &mut item, size, cancel)?;
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
// Managed paths never follow links outside the application-owned tree.
pub fn owned(root: &Path, path: &Path) -> Result<(), String> {
    let relative = path.strip_prefix(root).map_err(|_| "核心路径越界")?;
    let mut current = root.to_path_buf();
    for part in relative.components() {
        if !matches!(part, Component::Normal(_)) {
            return Err("核心路径无效".into());
        }
        current.push(part);
        if let Ok(m) = fs::symlink_metadata(&current) {
            if m.file_type().is_symlink() {
                return Err("受管路径不能包含链接".into());
            }
        }
    }
    Ok(())
}
pub fn prepare(
    root: &Path,
    runtime: &RuntimeService,
    candidate: &Candidate,
    archive: &Path,
    cancel: &AtomicBool,
) -> Result<ManagedRef, String> {
    crate::updates::release::validate_engine(candidate)?;
    if hash_cancel(archive, cancel)? != candidate.sha256
        || fs::metadata(archive).map_err(|e| e.to_string())?.len() != candidate.size
    {
        return Err("核心归档摘要或大小不符".into());
    }
    let mut reference = ManagedRef {
        version: candidate.version.clone(),
        target: candidate.target.clone(),
        hash: None,
        source: None,
    };
    let path = settings::managed_path(root, &reference)?;
    owned(root, &path)?;
    fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
    let staging = path.with_extension("part");
    owned(root, &staging)?;
    let result = (|| {
        unpack(archive, &staging, cfg!(windows), cancel)?;
        verify_binary(&staging, crate::TARGET)?;
        if runtime.probe(&staging, cancel)? != reference.version {
            return Err("核心版本与候选不符".into());
        }
        reference.hash = Some(hash_cancel(&staging, cancel)?);
        reference.source = Some(candidate.clone());
        // Keep replacement separate: the coordinator alone knows whether this version is running.
        Ok(reference)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&staging);
    }
    result
}
pub fn staged_path(root: &Path, reference: &ManagedRef) -> Result<std::path::PathBuf, String> {
    Ok(settings::managed_path(root, reference)?.with_extension("part"))
}
pub fn verify_path(
    runtime: &RuntimeService,
    reference: &ManagedRef,
    path: &Path,
    cancel: &AtomicBool,
) -> Result<(), String> {
    reference.validate()?;
    verify_binary(path, crate::TARGET)?;
    if let Some(hash) = &reference.hash {
        if hash_cancel(path, cancel)? != *hash {
            return Err("受管核心已损坏或被修改".into());
        }
    }
    if runtime.probe(path, cancel)? != reference.version {
        return Err("受管核心实际版本不符".into());
    }
    Ok(())
}
pub fn verify(
    root: &Path,
    runtime: &RuntimeService,
    reference: &ManagedRef,
    cancel: &AtomicBool,
) -> Result<(), String> {
    let path = settings::managed_path(root, reference)?;
    owned(root, &path)?;
    verify_path(runtime, reference, &path, cancel)
}
pub fn cleanup(root: &Path, keep: &[String], cancel: &AtomicBool) -> Result<(), String> {
    let directory = root.join("engines/sing-box");
    owned(root, &directory)?;
    if !directory.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(&directory).map_err(|e| e.to_string())? {
        cancelled(cancel)?;
        let entry = entry.map_err(|e| e.to_string())?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if keep.contains(&name)
            || settings::stable(&name).is_err()
            || !entry.file_type().map_err(|e| e.to_string())?.is_dir()
        {
            continue;
        }
        let path = entry.path();
        owned(root, &path)?;
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
