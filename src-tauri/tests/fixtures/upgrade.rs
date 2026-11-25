//! Native updater driver, compiled only for isolated package verification.
use serde::Deserialize;
use sing_shell::updates::{application, release::Candidate, Component};
use std::{fs, path::PathBuf};
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Input {
    #[serde(default)]
    verify_only: bool,
    root: PathBuf,
    executable: PathBuf,
    package: PathBuf,
    signature: String,
    current: String,
    version: String,
}
fn main() {
    let input: Input =
        serde_json::from_slice(&fs::read(std::env::args_os().nth(1).expect("input file")).unwrap())
            .unwrap();
    let mut context = tauri::generate_context!();
    context.config_mut().app.windows.clear();
    context.package_info_mut().version = semver::Version::parse(&input.current).unwrap();
    tauri::Builder::default()
        .plugin(tauri_plugin_updater::Builder::new().build())
        .setup(move |app| {
            let handle = app.handle().clone();
            std::thread::spawn(move || {
                let result = (|| {
                    let candidate = Candidate {
                        component: Component::App,
                        version: input.version.clone(),
                        target: sing_shell::TARGET.into(),
                        url: "https://example.invalid/isolated-signed-candidate".into(),
                        signature: input.signature,
                        sha256: sing_shell::core::package::hash(&input.package)?,
                        size: fs::metadata(&input.package)
                            .map_err(|e| e.to_string())?
                            .len(),
                        notes: "isolated native upgrade acceptance".into(),
                        release_url: "https://example.invalid/isolated".into(),
                        asset_name: String::new(),
                        asset_id: 0,
                        tag: format!("v{}", input.version),
                    };
                    if input.verify_only {
                        return application::verify(&handle, &candidate, &input.package);
                    }
                    let saved: serde_json::Value = serde_json::from_slice(
                        &fs::read(input.root.join("settings.json")).map_err(|e| e.to_string())?,
                    )
                    .map_err(|e| e.to_string())?;
                    if saved["configPath"].as_str().is_none()
                        || (saved.get("coreSource").is_none() && saved.get("engine").is_none())
                    {
                        return Err("missing preserved fixture settings".into());
                    }
                    application::install_at(&handle, &candidate, &input.package, &input.executable)
                })();
                match result {
                    Ok(()) => handle.exit(0),
                    Err(e) => {
                        eprintln!("Native upgrade failed: {e}");
                        let _ = fs::write(input.root.join("upgrade-error.txt"), e);
                        handle.exit(1);
                    }
                }
            });
            Ok(())
        })
        .run(context)
        .unwrap();
}
