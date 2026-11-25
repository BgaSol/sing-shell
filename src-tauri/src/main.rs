#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use sing_shell::core::{Core, Snapshot};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};
use tauri::{Emitter, Manager, State};
use tauri_plugin_dialog::DialogExt;

type SharedCore = Arc<Mutex<Core>>;

struct Smoke {
    enabled: bool,
    directory: PathBuf,
}

#[tauri::command]
fn frontend_ready(app: tauri::AppHandle, smoke: State<'_, Smoke>) -> Result<(), String> {
    if smoke.enabled {
        let report = serde_json::json!({ "version": app.package_info().version.to_string(), "completed": true, "frontendReady": true });
        std::fs::write(
            smoke.directory.join("smoke-result.json"),
            report.to_string(),
        )
        .map_err(|e| e.to_string())?;
        app.exit(0);
    }
    Ok(())
}

#[tauri::command]
async fn core_snapshot(state: State<'_, SharedCore>) -> Result<Snapshot, String> {
    let core = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || core.lock().map_err(|e| e.to_string())?.snapshot())
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn core_action(action: String, state: State<'_, SharedCore>) -> Result<Snapshot, String> {
    let core = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let mut core = core.lock().map_err(|e| e.to_string())?;
        match action.as_str() {
            "check" => core.check()?,
            "start" => core.start()?,
            "stop" => core.stop()?,
            _ => return Err("未知操作".into()),
        }
        core.snapshot()
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn choose_file(
    kind: String,
    app: tauri::AppHandle,
    state: State<'_, SharedCore>,
) -> Result<Option<Snapshot>, String> {
    let core = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let dialog = app.dialog().file();
        let dialog = match kind.as_str() {
            "core" => dialog.set_title("选择 sing-box 核心"),
            "config" => dialog
                .set_title("导入 sing-box 配置")
                .add_filter("sing-box 配置", &["json", "jsonc"]),
            _ => return Err("未知文件类型".into()),
        };
        let Some(path) = dialog.blocking_pick_file() else {
            return Ok(None);
        };
        let path = path.into_path().map_err(|e| e.to_string())?;
        let mut core = core.lock().map_err(|e| e.to_string())?;
        if kind == "core" {
            core.select_core(&path)?;
        } else {
            core.import_config(&path)?;
        }
        core.snapshot().map(Some)
    })
    .await
    .map_err(|e| e.to_string())?
}

fn main() {
    let smoke = std::env::args_os().any(|arg| arg == "--smoke-test");
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(move |app| {
            let directory = if smoke {
                let mut args = std::env::args_os();
                args.find(|arg| arg == "--test-data-dir")
                    .and_then(|_| args.next())
                    .map(PathBuf::from)
                    .ok_or("smoke test requires --test-data-dir")?
            } else {
                app.path().app_data_dir()?
            };
            let core = Core::open(directory.clone())?;
            app.manage(Arc::new(Mutex::new(core)));
            app.manage(Smoke {
                enabled: smoke,
                directory,
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            core_snapshot,
            core_action,
            choose_file,
            frontend_ready
        ])
        .build(tauri::generate_context!())
        .expect("无法启动 Sing Shell");
    app.run(|app, event| {
        if let tauri::RunEvent::ExitRequested { api, .. } = event {
            if let Err(error) = app
                .state::<SharedCore>()
                .lock()
                .map_err(|e| e.to_string())
                .and_then(|mut core| core.stop())
            {
                api.prevent_exit();
                let _ = app.emit("shutdown-error", error);
            }
        }
    });
}
