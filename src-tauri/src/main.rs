#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
use sing_shell::{
    app::{Application, Kind, Receipt},
    lifecycle,
    settings::UpdatePreferences,
    updates::{network, Component},
};
use std::{
    path::PathBuf,
    sync::{atomic::Ordering, Arc},
};
use tauri::{Emitter, Manager, State};
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_opener::OpenerExt;
type App = Arc<Application>;
struct Smoke {
    enabled: bool,
    directory: PathBuf,
}

#[tauri::command]
fn frontend_ready(
    app: tauri::AppHandle,
    smoke: State<Smoke>,
    application: State<App>,
) -> Result<(), String> {
    if smoke.enabled {
        application.dirty.store(true, Ordering::Release);
        app.get_webview_window("main")
            .ok_or("smoke window missing")?
            .close()
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}
#[tauri::command]
fn core_snapshot(app: State<App>) -> serde_json::Value {
    serde_json::json!({"settings":app.store.snapshot(),"runtime":app.runtime.snapshot(),"runtimeBusy":app.gate.runtime_busy(),"logs":app.runtime.logs.snapshot(),"storageError":app.store.error,"closing":app.gate.closing()})
}
#[tauri::command]
fn core_action(action: String, app: State<App>) -> Result<Receipt, String> {
    if !["start", "stop", "check"].contains(&action.as_str()) {
        return Err("未知操作".into());
    }
    let operation = app.gate.begin(Kind::Runtime)?;
    let receipt = operation.receipt();
    let app = app.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let result = match action.as_str() {
            "start" => app.runtime.start(&app.store, &operation.cancel),
            "stop" => app.runtime.stop(),
            _ => app.runtime.check(&app.store, &operation.cancel),
        };
        if let Err(error) = result {
            app.runtime.report(error);
        }
        drop(operation);
    });
    Ok(receipt)
}
#[tauri::command]
fn choose_file(kind: String, handle: tauri::AppHandle, app: State<App>) -> Result<Receipt, String> {
    if !["core", "config"].contains(&kind.as_str()) {
        return Err("未知文件类型".into());
    }
    app.runtime.stopped()?;
    let operation = app.gate.begin(Kind::Selection)?;
    let receipt = operation.receipt();
    let app = app.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let dialog = handle.dialog().file();
        let dialog = if kind == "core" {
            dialog.set_title("选择 sing-box 核心")
        } else {
            dialog
                .set_title("导入 sing-box 配置")
                .add_filter("sing-box 配置", &["json", "jsonc"])
        };
        if let Some(path) = dialog.blocking_pick_file() {
            let result = path
                .into_path()
                .map_err(|e| e.to_string())
                .and_then(|path| {
                    if operation.cancel.load(Ordering::Acquire) {
                        return Err("文件选择已取消".into());
                    }
                    if kind == "core" {
                        app.runtime.select(&app.store, &path, &operation.cancel)
                    } else {
                        app.runtime.import(&app.store, &path, &operation.cancel)
                    }
                });
            if let Err(error) = result {
                app.runtime.report(error);
            }
        }
        drop(operation);
    });
    Ok(receipt)
}
#[tauri::command]
fn update_snapshot(app: State<App>) -> sing_shell::updates::service::Snapshot {
    app.updates.snapshot()
}
#[tauri::command]
fn check_updates(component: Option<Component>, app: State<App>) -> Result<Receipt, String> {
    app.updates.check(component, false)
}
#[tauri::command]
fn download_updates(
    components: Vec<Component>,
    handle: tauri::AppHandle,
    app: State<App>,
) -> Result<Vec<Receipt>, String> {
    app.updates.download(handle, components)
}
#[tauri::command]
fn cancel_update(component: Component, app: State<App>) {
    app.gate.cancel_download(component);
}
#[tauri::command]
fn set_update_preferences(
    expected_revision: u64,
    preferences: UpdatePreferences,
    app: State<App>,
) -> Result<sing_shell::settings::Settings, String> {
    let _operation = app.gate.begin(Kind::Selection)?;
    let value = app
        .store
        .save(expected_revision, |s| s.update_preferences = preferences)?;
    app.updates.reset_network();
    Ok(value)
}
#[tauri::command]
async fn test_update_mirror(
    prefix: Option<String>,
    app: State<'_, App>,
) -> Result<Vec<network::Diagnostic>, String> {
    let operation = app.gate.begin(Kind::Diagnostic)?;
    let service = app.updates.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let result = service.diagnose(prefix.as_deref(), operation.cancel.clone());
        drop(operation);
        result
    })
    .await
    .map_err(|e| e.to_string())?
}
#[tauri::command]
fn builtin_mirrors() -> Vec<network::BuiltinMirror> {
    network::BUILTIN_MIRRORS.to_vec()
}
#[tauri::command]
fn request_restart(
    components: Vec<Component>,
    handle: tauri::AppHandle,
    app: State<App>,
) -> Result<(), String> {
    if app.dirty.load(Ordering::Acquire) {
        return Err("请先保存或放弃下载网络设置".into());
    }
    lifecycle::request(app.inner().clone(), handle, true, components)
}
#[tauri::command]
fn set_update_draft_dirty(dirty: bool, app: State<App>) {
    app.dirty.store(dirty, Ordering::Release);
}
#[tauri::command]
fn request_exit(handle: tauri::AppHandle, app: State<App>) -> Result<(), String> {
    if app.dirty.load(Ordering::Acquire) {
        return Err("仍有未处理的设置草稿".into());
    }
    lifecycle::request(app.inner().clone(), handle, false, Vec::new())
}
#[tauri::command]
fn open_update_release(component: Component, handle: tauri::AppHandle) -> Result<(), String> {
    let repository = if component == Component::App {
        "BgaSol/sing-shell"
    } else {
        "SagerNet/sing-box"
    };
    handle
        .opener()
        .open_url(
            format!("https://github.com/{repository}/releases"),
            None::<String>,
        )
        .map_err(|e| e.to_string())
}
fn close_requested(handle: &tauri::AppHandle, app: &App) {
    if app.gate.closing() {
        return;
    }
    if app.dirty.load(Ordering::Acquire) {
        let _ = handle.emit("close-requested", ());
        if handle.state::<Smoke>().enabled {
            let handle = handle.clone();
            let app = app.clone();
            let queued = handle.clone();
            let _ = queued.run_on_main_thread(move || {
                // Exercise a real dirty window close without desktop input.
                if handle.get_webview_window("main").is_none() {
                    handle.exit(1);
                    return;
                }
                let smoke = handle.state::<Smoke>();
                let report = serde_json::json!({"completed":true,"frontendReady":true,"closeRequestIntercepted":true,"version":handle.package_info().version.to_string(),"target":sing_shell::TARGET,"settings":app.store.snapshot()});
                if std::fs::write(smoke.directory.join("smoke-result.json"), report.to_string()).is_err() {
                    handle.exit(1);
                    return;
                }
                app.dirty.store(false, Ordering::Release);
                let _ = lifecycle::request(app, handle, false, Vec::new());
            });
        }
    } else {
        let _ = lifecycle::request(app.clone(), handle.clone(), false, Vec::new());
    }
}
fn main() {
    let smoke = std::env::args_os().any(|a| a == "--smoke-test");
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .setup(move |tauri| {
            let directory = if smoke {
                let mut args = std::env::args_os();
                args.find(|a| a == "--test-data-dir")
                    .and_then(|_| args.next())
                    .map(PathBuf::from)
                    .ok_or("smoke test requires --test-data-dir")?
            } else {
                tauri.path().app_data_dir()?
            };
            let application = Arc::new(Application::open(
                directory.clone(),
                tauri.package_info().version.to_string(),
            )?);
            application
                .updates
                .start(tauri.handle().clone(), !smoke && !cfg!(debug_assertions));
            tauri.manage(application);
            tauri.manage(Smoke {
                enabled: smoke,
                directory,
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            frontend_ready,
            core_snapshot,
            core_action,
            choose_file,
            update_snapshot,
            check_updates,
            download_updates,
            cancel_update,
            set_update_preferences,
            test_update_mirror,
            builtin_mirrors,
            request_restart,
            request_exit,
            set_update_draft_dirty,
            open_update_release
        ])
        .build(tauri::generate_context!())
        .expect("无法启动 Sing Shell");
    app.run(|handle, event| match event {
        tauri::RunEvent::WindowEvent {
            event: tauri::WindowEvent::CloseRequested { api, .. },
            ..
        } => {
            let app = handle.state::<App>();
            if !app.handed_off.load(Ordering::Acquire) {
                api.prevent_close();
                close_requested(handle, app.inner());
            }
        }
        tauri::RunEvent::ExitRequested { api, .. } => {
            let app = handle.state::<App>();
            if !app.handed_off.load(Ordering::Acquire) {
                api.prevent_exit();
                close_requested(handle, app.inner());
            }
        }
        _ => {}
    });
}
