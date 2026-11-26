use crate::updates::network::{Error, RequestKind, Transport};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::{
    fs,
    path::{Path, PathBuf},
};
pub fn bundled_commit() -> String {
    manifest().commit
}
pub fn validate_commit(commit: &str) -> Result<(), String> {
    if commit.len() == 40 && commit.bytes().all(|b| b.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err("站点规则版本标识无效".into())
    }
}
fn transport(cancel: &Arc<AtomicBool>) -> Result<Transport, String> {
    let mut preferences = crate::updates::network::Preferences::default();
    preferences.fallback_enabled = false;
    Transport::new(preferences, Arc::new(Default::default())).map(|t| {
        t.observed(Arc::new(|_| {}), {
            let c = cancel.clone();
            Arc::new(move || c.load(Ordering::Acquire))
        })
    })
}
pub fn latest(root: &Path, cancel: &Arc<AtomicBool>) -> Result<String, String> {
    #[derive(Deserialize)]
    struct Commit {
        sha: String,
    }
    let commit: Commit = transport(cancel)?.json(
        "https://api.github.com/repos/SagerNet/sing-geosite/commits/rule-set",
        RequestKind::Api,
        "application/vnd.github+json",
    )?;
    validate_commit(&commit.sha)?;
    prepare(root, &commit.sha, cancel)?;
    Ok(commit.sha)
}
pub fn prepare(root: &Path, commit: &str, cancel: &Arc<AtomicBool>) -> Result<(), String> {
    validate_commit(commit)?;
    if commit == bundled_commit() {
        return Ok(());
    }
    let directory = root.join("rules").join(commit);
    let record = directory.join("manifest.json");
    if let Ok(bytes) = fs::read(&record) {
        let m: Manifest = serde_json::from_slice(&bytes).map_err(|_| "规则版本记录损坏")?;
        if m.commit == commit
            && m.rules.len() == crate::profile::routing::SITE_IDS.len()
            && m.rules
                .iter()
                .all(|a| fs::read(directory.join(&a.file)).is_ok_and(|b| digest(&b) == a.sha256))
        {
            return Ok(());
        }
        return Err("规则版本内容已损坏，请重新下载".into());
    }
    let client = transport(cancel)?;
    #[derive(Deserialize)]
    struct Commit {
        sha: String,
    }
    let confirmed: Commit = client.json(
        &format!("https://api.github.com/repos/SagerNet/sing-geosite/commits/{commit}"),
        RequestKind::Api,
        "application/vnd.github+json",
    )?;
    if confirmed.sha != commit {
        return Err("规则上游提交身份不符".into());
    }
    crate::private_files::directory(&directory)?;
    let mut rules = Vec::new();
    for site in crate::profile::routing::SITE_IDS {
        if cancel.load(Ordering::Acquire) {
            return Err("规则更新已取消".into());
        }
        let file = format!("geosite-{site}.srs");
        let bytes = client.read(
            &format!("https://raw.githubusercontent.com/SagerNet/sing-geosite/{commit}/{file}"),
            RequestKind::File,
            "application/octet-stream",
            |bytes| {
                if bytes.len() > 1024 * 1024 || !bytes.starts_with(b"SRS") {
                    return Err(Error::remote("站点规则文件大小或格式无效"));
                }
                Ok(bytes.to_vec())
            },
        )?;
        crate::private_files::write(&directory.join(&file), &bytes)?;
        rules.push(Asset {
            id: (*site).into(),
            file,
            sha256: digest(&bytes),
            size: bytes.len() as u64,
        });
    }
    crate::settings::write_json(
        &record,
        &Manifest {
            repository: "SagerNet/sing-geosite".into(),
            commit: commit.into(),
            rules,
        },
    )
}

#[derive(Deserialize, Serialize)]
pub struct Manifest {
    pub repository: String,
    pub commit: String,
    pub rules: Vec<Asset>,
}
#[derive(Deserialize, Serialize)]
pub struct Asset {
    pub id: String,
    pub file: String,
    pub sha256: String,
    pub size: u64,
}
pub fn manifest() -> Manifest {
    serde_json::from_str(include_str!("../resources/rules/manifest.json"))
        .expect("bundled rule manifest")
}
pub fn bytes(id: &str) -> Result<&'static [u8], String> {
    Ok(match id {
        "github" => include_bytes!("../resources/rules/geosite-github.srs"),
        "google" => include_bytes!("../resources/rules/geosite-google.srs"),
        "youtube" => include_bytes!("../resources/rules/geosite-youtube.srs"),
        "telegram" => include_bytes!("../resources/rules/geosite-telegram.srs"),
        "microsoft" => include_bytes!("../resources/rules/geosite-microsoft.srs"),
        "apple" => include_bytes!("../resources/rules/geosite-apple.srs"),
        "openai" => include_bytes!("../resources/rules/geosite-openai.srs"),
        "cn" => include_bytes!("../resources/rules/geosite-cn.srs"),
        _ => return Err("未知站点集合".into()),
    })
}
pub fn materialize(root: &Path, id: &str) -> Result<PathBuf, String> {
    let manifest = manifest();
    let asset = manifest
        .rules
        .iter()
        .find(|r| r.id == id)
        .ok_or("未知站点集合")?;
    let data = bytes(id)?;
    if data.len() as u64 != asset.size || digest(data) != asset.sha256 {
        return Err("包内站点集合摘要无效".into());
    }
    let directory = root.join("rules").join(&manifest.commit);
    crate::private_files::directory(&directory)?;
    let path = directory.join(&asset.file);
    if !fs::read(&path).is_ok_and(|old| digest(&old) == asset.sha256) {
        crate::private_files::write(&path, data)?;
    }
    Ok(path)
}
pub fn materialize_at(root: &Path, commit: &str, id: &str) -> Result<PathBuf, String> {
    if commit == bundled_commit() {
        return materialize(root, id);
    }
    validate_commit(commit)?;
    if !crate::profile::routing::SITE_IDS.contains(&id) {
        return Err("未知站点集合".into());
    }
    let directory = root.join("rules").join(commit);
    let m: Manifest = serde_json::from_slice(
        &fs::read(directory.join("manifest.json")).map_err(|_| "站点规则版本尚未准备")?,
    )
    .map_err(|_| "站点规则版本记录损坏")?;
    let expected = format!("geosite-{id}.srs");
    let asset = m
        .rules
        .iter()
        .find(|a| a.id == id && a.file == expected)
        .ok_or("站点规则映射不符")?;
    let path = directory.join(expected);
    let bytes = fs::read(&path).map_err(|e| e.to_string())?;
    if bytes.len() as u64 != asset.size || digest(&bytes) != asset.sha256 {
        return Err("站点规则资产已损坏".into());
    }
    Ok(path)
}
pub fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bundled_assets_match_pinned_manifest() {
        let m = manifest();
        assert_eq!(m.rules.len(), crate::profile::routing::SITE_IDS.len());
        for r in m.rules {
            let bytes = bytes(&r.id).unwrap();
            assert_eq!(bytes.len() as u64, r.size);
            assert_eq!(digest(bytes), r.sha256);
            assert!(bytes.starts_with(b"SRS"));
        }
    }
}
