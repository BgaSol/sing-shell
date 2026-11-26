pub mod service;
use crate::{
    profile::routing::{Match, Routing},
    updates::network::{Error, Preferences, RequestKind, Transport},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Definition {
    pub id: &'static str,
    pub name: &'static str,
    pub repository: &'static str,
    pub branch: &'static str,
    pub file: &'static str,
    pub domains: bool,
    pub license: &'static str,
}
pub const CATALOG: &[Definition] = &[
    Definition {
        id: "cn-domain",
        name: "国内站点",
        repository: "SagerNet/sing-geosite",
        branch: "rule-set",
        file: "geosite-cn.srs",
        domains: true,
        license: "GPL-3.0-or-later",
    },
    Definition {
        id: "cn-ip",
        name: "中国大陆 IP",
        repository: "Loyalsoldier/geoip",
        branch: "release",
        file: "srs/cn.srs",
        domains: false,
        license: "CC-BY-SA-4.0 / GPL-3.0",
    },
    Definition {
        id: "google",
        name: "Google 服务",
        repository: "SagerNet/sing-geosite",
        branch: "rule-set",
        file: "geosite-google.srs",
        domains: true,
        license: "GPL-3.0-or-later",
    },
    Definition {
        id: "github",
        name: "GitHub",
        repository: "SagerNet/sing-geosite",
        branch: "rule-set",
        file: "geosite-github.srs",
        domains: true,
        license: "GPL-3.0-or-later",
    },
    Definition {
        id: "telegram",
        name: "Telegram",
        repository: "SagerNet/sing-geosite",
        branch: "rule-set",
        file: "geosite-telegram.srs",
        domains: true,
        license: "GPL-3.0-or-later",
    },
];
pub fn definition(id: &str) -> Result<&'static Definition, String> {
    CATALOG
        .iter()
        .find(|s| s.id == id)
        .ok_or_else(|| "未知开源规则来源".into())
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Version {
    pub source_id: String,
    pub commit: String,
    pub sha256: String,
    pub size: u64,
}
impl Version {
    pub fn validate(&self) -> Result<(), String> {
        definition(&self.source_id)?;
        validate_commit(&self.commit)?;
        crate::updates::release::digest(&self.sha256)?;
        if self.size == 0 || self.size > 8 * 1024 * 1024 {
            return Err("规则大小无效".into());
        }
        Ok(())
    }
}
pub fn validate_commit(commit: &str) -> Result<(), String> {
    if commit.len() == 40 && commit.bytes().all(|b| b.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err("规则提交标识无效".into())
    }
}
pub fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
pub fn bundled() -> Vec<Version> {
    serde_json::from_str(include_str!("../resources/rules/manifest.json"))
        .expect("bundled rules manifest")
}
fn bytes(id: &str) -> Result<&'static [u8], String> {
    Ok(match id {
        "cn-domain" => include_bytes!("../resources/rules/geosite-cn.srs"),
        "cn-ip" => include_bytes!("../resources/rules/geoip-cn.srs"),
        "google" => include_bytes!("../resources/rules/geosite-google.srs"),
        "github" => include_bytes!("../resources/rules/geosite-github.srs"),
        "telegram" => include_bytes!("../resources/rules/geosite-telegram.srs"),
        _ => return Err("未知开源规则来源".into()),
    })
}
fn directory(root: &Path, v: &Version) -> PathBuf {
    root.join("rules/content")
        .join(&v.source_id)
        .join(&v.commit)
}
pub fn materialize_at(root: &Path, v: &Version) -> Result<PathBuf, String> {
    v.validate()?;
    let dir = directory(root, v);
    let path = dir.join("rules.srs");
    if bundled().iter().any(|b| b == v) {
        let data = bytes(&v.source_id)?;
        if data.len() as u64 != v.size || digest(data) != v.sha256 {
            return Err("包内规则摘要无效".into());
        }
        crate::private_files::directory(&dir)?;
        if !fs::read(&path).is_ok_and(|b| digest(&b) == v.sha256) {
            crate::private_files::write(&path, data)?;
        }
    }
    let data = fs::read(&path).map_err(|_| "规则版本尚未准备")?;
    if data.len() as u64 != v.size || digest(&data) != v.sha256 {
        return Err("规则内容损坏".into());
    }
    Ok(path)
}
fn transport(preferences: Preferences, cancel: &Arc<AtomicBool>) -> Result<Transport, String> {
    Transport::new(preferences, Arc::new(Default::default())).map(|t| {
        t.observed(Arc::new(|_| {}), {
            let c = cancel.clone();
            Arc::new(move || c.load(Ordering::Acquire))
        })
    })
}
#[derive(Deserialize)]
struct Commit {
    sha: String,
}
pub fn latest_commit(
    id: &str,
    preferences: Preferences,
    cancel: &Arc<AtomicBool>,
) -> Result<String, String> {
    let d = definition(id)?;
    let c: Commit = transport(preferences, cancel)?.json(
        &format!(
            "https://api.github.com/repos/{}/commits/{}",
            d.repository, d.branch
        ),
        RequestKind::Api,
        "application/vnd.github+json",
    )?;
    validate_commit(&c.sha)?;
    Ok(c.sha)
}
// Only fixed upstream identities are accepted; workers use their own protected root.
pub fn prepare(
    root: &Path,
    id: &str,
    commit: &str,
    preferences: Preferences,
    cancel: &Arc<AtomicBool>,
) -> Result<Version, String> {
    let d = definition(id)?;
    validate_commit(commit)?;
    if let Some(v) = bundled()
        .into_iter()
        .find(|v| v.source_id == id && v.commit == commit)
    {
        materialize_at(root, &v)?;
        return Ok(v);
    }
    let dir = root.join("rules/content").join(id).join(commit);
    let record = dir.join("version.json");
    if let Ok(data) = fs::read(&record) {
        let v: Version = serde_json::from_slice(&data).map_err(|_| "规则版本记录损坏")?;
        if v.source_id != id || v.commit != commit {
            return Err("规则版本身份不符".into());
        }
        materialize_at(root, &v)?;
        return Ok(v);
    }
    let client = transport(preferences, cancel)?;
    let confirmed: Commit = client.json(
        &format!(
            "https://api.github.com/repos/{}/commits/{commit}",
            d.repository
        ),
        RequestKind::Api,
        "application/vnd.github+json",
    )?;
    if confirmed.sha != commit {
        return Err("上游规则身份不符".into());
    }
    let data = client.read(
        &format!(
            "https://raw.githubusercontent.com/{}/{commit}/{}",
            d.repository, d.file
        ),
        RequestKind::File,
        "application/octet-stream",
        |b| {
            if b.len() > 8 * 1024 * 1024 || !b.starts_with(b"SRS") {
                return Err(Error::remote("规则文件大小或格式无效"));
            }
            Ok(b.to_vec())
        },
    )?;
    crate::core::package::cancelled(cancel)?;
    let v = Version {
        source_id: id.into(),
        commit: commit.into(),
        sha256: digest(&data),
        size: data.len() as u64,
    };
    crate::private_files::directory(&dir)?;
    crate::private_files::write(&dir.join("rules.srs"), &data)?;
    crate::settings::write_json(&record, &v)?;
    Ok(v)
}
pub fn selected(root: &Path, routing: &Routing) -> Result<Vec<Version>, String> {
    let index: Vec<Version> = fs::read(root.join("rules/index.json"))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    routing
        .policies
        .iter()
        .filter(|p| p.enabled)
        .filter_map(|p| {
            if let Match::Source { source_id } = &p.r#match {
                Some(source_id)
            } else {
                None
            }
        })
        .map(|id| {
            let v = index
                .iter()
                .find(|v| &v.source_id == id)
                .cloned()
                .or_else(|| bundled().into_iter().find(|v| &v.source_id == id))
                .ok_or("来源无可用版本")?;
            materialize_at(root, &v)?;
            Ok(v)
        })
        .collect()
}
pub fn validate_file(
    root: &Path,
    v: &Version,
    core: &Path,
    cancel: &AtomicBool,
) -> Result<(), String> {
    let path = materialize_at(root, v)?;
    let check = directory(root, v).join("check.json");
    crate::settings::write_json(
        &check,
        &serde_json::json!({"route":{"rule_set":[{"type":"local","tag":"candidate","format":"binary","path":path}],"rules":[{"rule_set":["candidate"],"action":"route","outbound":"direct"}]},"outbounds":[{"type":"direct","tag":"direct"}]}),
    )?;
    let runtime = crate::runtime::RuntimeService::default();
    let result = runtime.validate(core, &check, cancel);
    let _ = fs::remove_file(check);
    result
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bundled_content_has_fixed_identity() {
        let vs = bundled();
        assert_eq!(vs.len(), CATALOG.len());
        for v in vs {
            v.validate().unwrap();
            let b = bytes(&v.source_id).unwrap();
            assert_eq!(v.sha256, digest(b));
            assert_eq!(v.size, b.len() as u64);
            assert!(b.starts_with(b"SRS"));
        }
    }
}

pub fn prune(root: &Path, keep: &[Version]) -> Result<(), String> {
    let mut retained = bundled();
    retained.extend_from_slice(keep);
    for source in CATALOG {
        let directory = root.join("rules/content").join(source.id);
        let Ok(meta) = fs::symlink_metadata(&directory) else {
            continue;
        };
        if !meta.is_dir() || meta.file_type().is_symlink() {
            continue;
        }
        for entry in fs::read_dir(&directory).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let Some(commit) = entry.file_name().to_str().map(str::to_string) else {
                continue;
            };
            let kind = entry.file_type().map_err(|e| e.to_string())?;
            if kind.is_dir()
                && !kind.is_symlink()
                && validate_commit(&commit).is_ok()
                && !retained
                    .iter()
                    .any(|v| v.source_id == source.id && v.commit == commit)
            {
                let path = entry.path();
                if path.parent() != Some(directory.as_path()) {
                    return Err("规则回收路径无效".into());
                }
                fs::remove_dir_all(path).map_err(|e| e.to_string())?;
            }
        }
    }
    Ok(())
}
