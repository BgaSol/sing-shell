use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
};

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
