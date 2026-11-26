use super::release::Candidate;
use std::{fs, path::Path};
use tauri_plugin_updater::{Update, UpdaterExt};
pub fn restore<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    candidate: &Candidate,
) -> Result<Update, String> {
    restore_at(app, candidate, None)
}
fn restore_at<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    candidate: &Candidate,
    executable: Option<&Path>,
) -> Result<Update, String> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    if candidate.target != crate::TARGET {
        return Err("更新包目标平台不符".into());
    }
    let raw = serde_json::json!({"version":candidate.version,"notes":candidate.notes,"platforms":{(crate::TARGET):{"url":candidate.url,"signature":candidate.signature}}});
    let builder = app
        .updater_builder()
        .target(crate::TARGET)
        .endpoints(vec![format!(
            "https://github.com/{}/releases/download/{}/core-update.json",
            super::release::REPOSITORY,
            candidate.tag
        )
        .parse()
        .map_err(|_| "更新地址无效")?])
        .map_err(|e| e.to_string())?;
    #[cfg(feature = "test-fixtures")]
    let builder = if let Some(executable) = executable {
        let b = builder
            .executable_path(executable)
            .restart_after_install(false);
        #[cfg(windows)]
        let b = b.installer_arg("/S");
        b
    } else {
        builder
    };
    #[cfg(not(feature = "test-fixtures"))]
    let _ = executable;
    #[cfg(windows)]
    let builder = if let Some(executable) = executable {
        builder.installer_args([
            crate::login_startup::installation_scope(executable)?.to_string(),
            format!(
                "/D={}",
                executable.parent().ok_or("缺少安装路径")?.display()
            ),
        ])
    } else {
        builder
    };
    builder
        .build()
        .map_err(|e| e.to_string())?
        .restore_update(raw)
        .map_err(|e| e.to_string())?
        .ok_or("准备版本不高于当前应用版本".into())
}
#[cfg(feature = "test-fixtures")]
pub fn install_at<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    candidate: &Candidate,
    path: &Path,
    executable: &Path,
) -> Result<(), String> {
    let update = restore_at(app, candidate, Some(executable))?;
    let bytes = fs::read(path).map_err(|e| e.to_string())?;
    update.verify(&bytes).map_err(|e| e.to_string())?;
    update.install(bytes).map_err(|e| e.to_string())
}
pub fn verify<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    candidate: &Candidate,
    path: &Path,
) -> Result<(), String> {
    if fs::metadata(path).map_err(|e| e.to_string())?.len() > 512 * 1024 * 1024 {
        return Err("缓存包过大".into());
    }
    let update = restore(app, candidate)?;
    let bytes = fs::read(path).map_err(|e| e.to_string())?;
    update
        .verify(&bytes)
        .map_err(|e| format!("应用签名校验失败：{e}"))
}

#[cfg(all(test, feature = "test-fixtures"))]
mod tests {
    use super::*;
    const KEY:&str="dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IEE0OUVGNDc0NkZBNkU2RDcKUldUWDVxWnZkUFNlcERHV2cyYno4bjZBclFuQk13Z1FLMWdlVm9zb3JjYkx6Y3VuRCtLSitjWWU=";
    const SIGNATURE:&str="dW50cnVzdGVkIGNvbW1lbnQ6IHNpZ25hdHVyZSBmcm9tIHRhdXJpIHNlY3JldCBrZXkKUlVUWDVxWnZkUFNlcFBGMnpzanZQL0wvVFF0QkxSbldSdmFxKzRvbUsxVUV1Z3RRcC9VU3JHaUkxdjc2MnNEeTNpZDNhb3I3YlJDOUVUeWVvdDZsTUhIcldvWjVoalVNMVFjPQp0cnVzdGVkIGNvbW1lbnQ6IHRpbWVzdGFtcDoxNzkxNjQ5MDUxCWZpbGU6cGF5bG9hZAl2ZXJzaW9uOjIuMC4wCkREcTJDTE9FbDVjUW1DMmVKUkhsblpQVlZpT1pzdTFDanBQS1FvZWk0QS9YQmZrRHFHTzBCK1FNcmNySGdFUTVxVjNtbmU3NXRkRldoSnBxVVhoOUFBPT0=";
    #[test]
    fn official_verifier_rejects_modified_bytes_and_forged_versions() {
        let mut context = tauri::test::mock_context(tauri::test::noop_assets());
        context
            .config_mut()
            .plugins
            .0
            .insert("updater".into(), serde_json::json!({"pubkey":KEY}));
        let app = tauri::test::mock_builder()
            .plugin(tauri_plugin_updater::Builder::new().build())
            .build(context)
            .unwrap();
        let raw = serde_json::json!({"version":"2.0.0","platforms":{(crate::TARGET):{"url":"https://example.invalid/package","signature":SIGNATURE}}});
        let update = app
            .handle()
            .updater_builder()
            .target(crate::TARGET)
            .endpoints(vec!["https://example.invalid/update".parse().unwrap()])
            .unwrap()
            .build()
            .unwrap()
            .restore_update(raw.clone())
            .unwrap()
            .unwrap();
        update.verify(b"test").unwrap();
        assert!(update.verify(b"corrupt").is_err());
        let mut forged = raw;
        forged["version"] = serde_json::json!("3.0.0");
        let update = app
            .handle()
            .updater_builder()
            .target(crate::TARGET)
            .endpoints(vec!["https://example.invalid/update".parse().unwrap()])
            .unwrap()
            .build()
            .unwrap()
            .restore_update(forged)
            .unwrap()
            .unwrap();
        assert!(update.verify(b"test").is_err());
    }
}
pub fn install<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    candidate: &Candidate,
    path: &Path,
) -> Result<(), String> {
    verify(app, candidate, path)?;
    #[cfg(windows)]
    let update = restore_at(
        app,
        candidate,
        Some(&std::env::current_exe().map_err(|e| e.to_string())?),
    )?;
    #[cfg(not(windows))]
    let update = restore(app, candidate)?;
    update
        .install(fs::read(path).map_err(|e| e.to_string())?)
        .map_err(|e| format!("应用更新安装失败：{e}"))
}
