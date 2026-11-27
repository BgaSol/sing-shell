use crate::updates::{
    network::{RequestKind, Transport},
    release::{self, Candidate},
    Component,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::Path, sync::atomic::AtomicBool};

pub const BASELINE: &str = "1.14.4";
const PURPOSE: &str = "com.bgasol.singshell.core/v1";
const ROOT: &str = "https://github.com/BgaSol/sing-shell/releases/download";
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Envelope {
    pub payload: String,
    pub signature: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Manifest {
    purpose: String,
    schema_version: u32,
    version: String,
    repository: String,
    platforms: BTreeMap<String, Asset>,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Asset {
    pub asset_id: u64,
    pub archive_sha256: String,
    pub archive_size: u64,
    pub program_sha256: String,
    pub program_size: u64,
}
impl Envelope {
    pub fn verify(&self, version: &str) -> Result<Asset, String> {
        if self.payload.len() > 32 * 1024 || self.signature.len() > 128 {
            return Err("核心签名材料过大".into());
        }
        let decode = |s: &str| {
            STANDARD
                .decode(s)
                .map_err(|_| "核心签名编码无效".to_string())
        };
        let bytes = decode(&self.payload)?;
        let signature = decode(&self.signature)?;
        let key = decode(include_str!("../../resources/cores/public-key.txt").trim())?;
        let provider = rustls::crypto::ring::default_provider();
        let algorithm = provider
            .signature_verification_algorithms
            .mapping
            .iter()
            .find(|(scheme, _)| *scheme == rustls::SignatureScheme::ED25519)
            .ok_or("Ed25519 验证不可用")?
            .1[0];
        let valid = algorithm.verify_signature(&key, &bytes, &signature).is_ok();
        #[cfg(feature = "test-fixtures")]
        let valid = valid
            || algorithm
                .verify_signature(
                    &decode(include_str!("../../tests/fixtures/material/public.txt").trim())?,
                    &bytes,
                    &signature,
                )
                .is_ok();
        if !valid {
            return Err("核心材料项目签名无效".into());
        }
        let manifest: Manifest = serde_json::from_slice(&bytes).map_err(|_| "核心材料格式无效")?;
        crate::settings::stable(version)?;
        if manifest.schema_version != 1
            || manifest.purpose != PURPOSE
            || manifest.repository != "SagerNet/sing-box"
            || manifest.version != version
            || manifest.platforms.len() != 5
        {
            return Err("核心材料用途、版本或来源不符".into());
        }
        for (target, asset) in &manifest.platforms {
            release::engine_asset(version, target)?;
            release::digest(&asset.archive_sha256)?;
            release::digest(&asset.program_sha256)?;
            if asset.asset_id == 0
                || asset.archive_size == 0
                || asset.archive_size > 512 * 1024 * 1024
                || asset.program_size < 512
                || asset.program_size > 256 * 1024 * 1024
            {
                return Err("核心材料大小无效".into());
            }
        }
        manifest
            .platforms
            .get(crate::TARGET)
            .cloned()
            .ok_or("核心材料缺少本机平台".into())
    }
    pub fn candidate(&self, version: &str) -> Result<Candidate, String> {
        let asset = self.verify(version)?;
        let name = release::engine_asset(version, crate::TARGET)?;
        Ok(Candidate {
            component: Component::Engine,
            version: version.into(),
            target: crate::TARGET.into(),
            url: format!(
                "https://github.com/SagerNet/sing-box/releases/download/v{version}/{name}"
            ),
            signature: serde_json::to_string(self).map_err(|e| e.to_string())?,
            sha256: asset.archive_sha256,
            size: asset.archive_size,
            notes: "Sing Shell 已验证的官方核心；项目签名材料支持离线受保护安装。".into(),
            release_url: format!("https://github.com/SagerNet/sing-box/releases/tag/v{version}"),
            asset_name: name,
            asset_id: asset.asset_id,
            tag: format!("v{version}"),
        })
    }
}

#[cfg(feature = "test-fixtures")]
pub fn fixture(candidate: &Candidate, program: &Path) -> Envelope {
    let mut platforms = serde_json::Map::new();
    for target in [
        "x86_64-pc-windows-msvc",
        "aarch64-pc-windows-msvc",
        "x86_64-unknown-linux-gnu",
        "aarch64-unknown-linux-gnu",
        "aarch64-apple-darwin",
    ] {
        platforms.insert(target.into(),serde_json::json!({"assetId":candidate.asset_id,"archiveSha256":candidate.sha256,"archiveSize":candidate.size,"programSha256":super::package::hash(program).unwrap(),"programSize":std::fs::metadata(program).unwrap().len()}));
    }
    let payload=serde_json::to_vec(&serde_json::json!({"schemaVersion":1,"purpose":PURPOSE,"repository":"SagerNet/sing-box","version":candidate.version,"platforms":platforms})).unwrap();
    let key = rustls::pki_types::PrivatePkcs8KeyDer::from(
        include_bytes!("../../tests/fixtures/material/key.der").to_vec(),
    );
    let signer = rustls::crypto::ring::sign::any_supported_type(&key.into())
        .unwrap()
        .choose_scheme(&[rustls::SignatureScheme::ED25519])
        .unwrap();
    Envelope {
        signature: STANDARD.encode(signer.sign(&payload).unwrap()),
        payload: STANDARD.encode(payload),
    }
}
pub fn bundled(version: &str) -> Option<Envelope> {
    let json = match version {
        "1.14.4" => include_str!("../../resources/cores/1.14.4.json"),
        "1.14.5" => include_str!("../../resources/cores/1.14.5.json"),
        _ => return None,
    };
    serde_json::from_str(json).ok()
}
pub fn for_candidate(c: &Candidate) -> Result<Envelope, String> {
    let envelope = if c.signature.is_empty() {
        bundled(&c.version).ok_or("此核心缺少项目签名材料，请在核心与更新中准备已验证版本")?
    } else {
        serde_json::from_str(&c.signature).map_err(|_| "核心签名材料无效")?
    };
    let expected = envelope.candidate(&c.version)?;
    if !crate::core::same_asset(c, &expected) {
        return Err("核心来源与签名材料不符".into());
    }
    Ok(envelope)
}
pub fn program(
    envelope: &Envelope,
    version: &str,
    path: &Path,
    cancel: &AtomicBool,
) -> Result<(), String> {
    let asset = envelope.verify(version)?;
    if std::fs::metadata(path).map_err(|e| e.to_string())?.len() != asset.program_size
        || super::package::hash_cancel(path, cancel)? != asset.program_sha256
    {
        return Err("核心程序与项目签名材料不符".into());
    }
    super::package::verify_binary(path, crate::TARGET)
}
pub fn version(client: &Transport, version: &str) -> Result<Candidate, String> {
    crate::settings::stable(version)?;
    let envelope = match bundled(version) {
        Some(e) => e,
        None => client.json(
            &format!("{ROOT}/core-v{version}/material.json"),
            RequestKind::File,
            "application/json",
        )?,
    };
    envelope.candidate(version)
}
pub fn discover(client: &Transport, current: &str) -> Result<Option<Candidate>, String> {
    let envelope: Envelope = client.json(
        &format!("{ROOT}/core-channel/recommended.json"),
        RequestKind::File,
        "application/json",
    )?;
    // Read the untrusted version only to select the expected value; verify the complete material next.
    let bytes = STANDARD
        .decode(&envelope.payload)
        .map_err(|_| "核心材料编码无效")?;
    let raw: serde_json::Value = serde_json::from_slice(&bytes).map_err(|_| "核心材料格式无效")?;
    let candidate = envelope.candidate(raw["version"].as_str().ok_or("缺少核心版本")?)?;
    Ok(
        (crate::settings::stable(&candidate.version)? > crate::settings::stable(current)?)
            .then_some(candidate),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn signed_material_binds_program_version_and_purpose() {
        let envelope = bundled(BASELINE).unwrap();
        let candidate = envelope.candidate(BASELINE).unwrap();
        assert!(for_candidate(&candidate).is_ok());
        assert!(envelope.verify("1.14.5").is_err());
        let mut modified = envelope.clone();
        modified.payload.replace_range(3..4, "A");
        assert!(modified.verify(BASELINE).is_err());
        let mut modified = envelope.clone();
        modified.signature.replace_range(3..4, "A");
        assert!(modified.verify(BASELINE).is_err());
        let mut candidate = candidate;
        candidate.sha256 = "a".repeat(64);
        assert!(for_candidate(&candidate).is_err());
    }
    #[cfg(feature = "test-fixtures")]
    #[test]
    fn approved_versions_do_not_contact_a_rate_limited_source() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let client = Transport::test_server(
            &format!("http://{}", listener.local_addr().unwrap()),
            vec![],
            std::sync::Arc::new(Default::default()),
        );
        for approved in ["1.14.4", "1.14.5"] {
            let candidate = version(&client, approved).unwrap();
            crate::updates::release::recheck(&client, &candidate).unwrap();
        }
        assert_eq!(
            listener.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock,
            "offline material must not send an HTTP request"
        );
    }
}
