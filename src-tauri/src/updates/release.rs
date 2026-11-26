use super::{
    network::{RequestKind, Transport},
    Component,
};
use crate::settings::stable;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
const ACCEPT: &str = "application/vnd.github+json";
pub const REPOSITORY: &str = "BgaSol/sing-shell";

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Candidate {
    pub component: Component,
    pub version: String,
    pub target: String,
    pub url: String,
    pub signature: String,
    pub sha256: String,
    pub size: u64,
    pub notes: String,
    pub release_url: String,
    pub asset_name: String,
    pub asset_id: u64,
    pub tag: String,
}
#[derive(Deserialize)]
struct TagRef {
    #[serde(rename = "ref")]
    name: String,
}
#[derive(Deserialize)]
struct Release {
    tag_name: String,
    draft: bool,
    prerelease: bool,
    assets: Vec<Asset>,
    body: Option<String>,
    html_url: String,
}
#[derive(Deserialize)]
struct Asset {
    id: u64,
    name: String,
    browser_download_url: String,
    size: u64,
    #[serde(default)]
    digest: Option<String>,
}
#[derive(Deserialize)]
struct Manifest {
    id: String,
    version: String,
    #[serde(default)]
    notes: String,
    platforms: BTreeMap<String, Package>,
}
#[derive(Deserialize)]
struct Package {
    url: String,
    signature: String,
    sha256: String,
    size: u64,
}

pub fn engine_asset(version: &str, target: &str) -> Result<String, String> {
    stable(version)?;
    let platform = match target {
        "x86_64-pc-windows-msvc" => "windows-amd64.zip",
        "aarch64-pc-windows-msvc" => "windows-arm64.zip",
        "x86_64-unknown-linux-gnu" => "linux-amd64.tar.gz",
        "aarch64-unknown-linux-gnu" => "linux-arm64.tar.gz",
        "aarch64-apple-darwin" => "darwin-arm64.tar.gz",
        _ => return Err("未知平台".into()),
    };
    Ok(format!("sing-box-{version}-{platform}"))
}
pub fn digest(value: &str) -> Result<&str, String> {
    let value = value.strip_prefix("sha256:").unwrap_or(value);
    if value.len() != 64 || !value.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("发行包缺少有效的 SHA-256".into());
    }
    Ok(value)
}
pub fn validate_engine(c: &Candidate) -> Result<(), String> {
    if c.component != Component::Engine
        || c.target != crate::TARGET
        || c.tag != format!("v{}", c.version)
        || c.asset_name != engine_asset(&c.version, &c.target)?
        || c.url
            != format!(
                "https://github.com/SagerNet/sing-box/releases/download/{}/{}",
                c.tag, c.asset_name
            )
        || c.size == 0
        || c.size > 512 * 1024 * 1024
    {
        return Err("核心资产来源、目标或大小不符".into());
    }
    digest(&c.sha256)?;
    Ok(())
}
pub fn engine_version(transport: &Transport, version: &str) -> Result<Candidate, String> {
    stable(version)?;
    let client = transport.clone().only_endpoint(None)?;
    // The shipped bootstrap version keeps its upstream digest even when GitHub's API is rate limited.
    let release: Release = if version == "1.14.4" {
        serde_json::from_str(include_str!("../../resources/core-1.14.4.json"))
            .map_err(|_| "包内官方核心来源记录无效")?
    } else {
        client.json(
            &format!("https://api.github.com/repos/SagerNet/sing-box/releases/tags/v{version}"),
            RequestKind::Api,
            ACCEPT,
        )?
    };
    if release.draft || release.prerelease || release.tag_name != format!("v{version}") {
        return Err("核心发行状态不符".into());
    }
    let c = candidate(&client, Component::Engine, version, release)?;
    validate_engine(&c)?;
    Ok(c)
}

pub fn discover(
    transport: &Transport,
    component: Component,
    current: &str,
) -> Result<Option<Candidate>, String> {
    let repository = if component == Component::App {
        REPOSITORY
    } else {
        "SagerNet/sing-box"
    };
    // Upstream digest metadata must originate from GitHub, not a mirror controlling the bytes.
    let client = if component == Component::Engine {
        transport.clone().only_endpoint(None)?
    } else {
        transport.clone()
    };
    let refs: Vec<TagRef> = client.json(
        &format!("https://api.github.com/repos/{repository}/git/matching-refs/tags/v"),
        RequestKind::Api,
        ACCEPT,
    )?;
    let mut tags: Vec<_> = refs
        .into_iter()
        .filter_map(|r| {
            let v = r.name.strip_prefix("refs/tags/v")?;
            Some((stable(v).ok()?, format!("v{v}")))
        })
        .collect();
    tags.sort_by(|a, b| b.0.cmp(&a.0));
    for (version, tag) in tags {
        if version <= stable(current)? {
            break;
        }
        let Some(release) = client.json_optional::<Release>(
            &format!("https://api.github.com/repos/{repository}/releases/tags/{tag}"),
            RequestKind::Api,
            ACCEPT,
        )?
        else {
            continue;
        };
        if release.draft || release.prerelease {
            continue;
        }
        if release.tag_name != tag {
            return Err("请求标签与发行不一致".into());
        }
        return candidate(&client, component, &version.to_string(), release).map(Some);
    }
    Ok(None)
}
fn candidate(
    client: &Transport,
    component: Component,
    version: &str,
    release: Release,
) -> Result<Candidate, String> {
    let (asset, signature, sha256, notes) = if component == Component::App {
        let manifest_asset = release
            .assets
            .iter()
            .find(|a| a.name == "core-update.json")
            .ok_or("最新正式发行缺少更新清单")?;
        let manifest: Manifest = client.json(
            &manifest_asset.browser_download_url,
            RequestKind::File,
            ACCEPT,
        )?;
        if manifest.id != component.id() || manifest.version != version {
            return Err("更新清单组件或版本不符".into());
        }
        let p = manifest
            .platforms
            .get(crate::TARGET)
            .ok_or("新版没有本机平台的更新包")?;
        if p.signature.is_empty() {
            return Err("应用更新缺少官方签名".into());
        }
        digest(&p.sha256)?;
        let asset = release
            .assets
            .iter()
            .find(|a| a.browser_download_url == p.url && a.size == p.size)
            .ok_or("更新包不属于此正式发行或大小不符")?;
        (asset, p.signature.clone(), p.sha256.clone(), manifest.notes)
    } else {
        let name = engine_asset(version, crate::TARGET)?;
        let asset = release
            .assets
            .iter()
            .find(|a| a.name == name)
            .ok_or("最新正式发行缺少本机原生 sing-box 包")?;
        (
            asset,
            String::new(),
            digest(asset.digest.as_deref().ok_or("上游发行没有资产摘要")?)?.into(),
            release.body.clone().unwrap_or_default(),
        )
    };
    client.validate_url(&asset.browser_download_url)?;
    if !asset.browser_download_url.starts_with(&format!(
        "https://github.com/{}/releases/download/{}/",
        if component == Component::App {
            REPOSITORY
        } else {
            "SagerNet/sing-box"
        },
        release.tag_name
    )) {
        return Err("更新包来源不符".into());
    }
    if asset.size == 0 || asset.size > 512 * 1024 * 1024 {
        return Err("发行包大小无效".into());
    }
    Ok(Candidate {
        component,
        version: version.into(),
        target: crate::TARGET.into(),
        url: asset.browser_download_url.clone(),
        signature,
        sha256,
        size: asset.size,
        notes,
        release_url: release.html_url,
        asset_name: asset.name.clone(),
        asset_id: asset.id,
        tag: release.tag_name,
    })
}
pub fn recheck(client: &Transport, cached: &Candidate) -> Result<(), String> {
    let client = if cached.component == Component::Engine {
        client.clone().only_endpoint(None)?
    } else {
        client.clone()
    };
    let repository = if cached.component == Component::App {
        REPOSITORY
    } else {
        "SagerNet/sing-box"
    };
    let release: Release = client.json(
        &format!(
            "https://api.github.com/repos/{repository}/releases/tags/{}",
            cached.tag
        ),
        RequestKind::Api,
        ACCEPT,
    )?;
    if release.draft || release.prerelease {
        return Err("发行状态已改变，请重新检查".into());
    }
    let fresh = candidate(&client, cached.component, &cached.version, release)?;
    if fresh != *cached {
        return Err("发行信息已改变，请重新检查".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn platform_mapping_and_digest_are_exact() {
        assert_eq!(
            engine_asset("1.14.4", "aarch64-pc-windows-msvc").unwrap(),
            "sing-box-1.14.4-windows-arm64.zip"
        );
        assert!(engine_asset("../x", crate::TARGET).is_err());
        assert!(digest("abc").is_err());
        assert!(digest(&"a".repeat(64)).is_ok());
        let client =
            Transport::new(Default::default(), std::sync::Arc::new(Default::default())).unwrap();
        let pinned = engine_version(&client, "1.14.4").unwrap();
        validate_engine(&pinned).unwrap();
        assert_eq!(pinned.version, "1.14.4");
    }
}
