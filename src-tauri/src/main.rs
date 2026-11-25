#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
use sing_shell::{
    app::{Application, Kind, Receipt},
    core::{Action, Maintenance},
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
    let settings = app.store.snapshot();
    let runtime = app.runtime.snapshot();
    let core = app.core.snapshot_for(&settings, &runtime);
    serde_json::json!({"settings":settings,"runtime":runtime,"runtimeBusy":app.gate.runtime_busy(),"core":core,"logs":app.runtime.logs.snapshot(),"storageError":app.store.error,"closing":app.gate.closing()})
}
#[tauri::command]
fn core_action(action: Action, app: State<App>) -> Result<Receipt, String> {
    app.core.action(action)
}
#[tauri::command]
fn core_maintenance(action: Maintenance, app: State<App>) -> Result<Receipt, String> {
    app.core.maintain(Some(action))
}
#[tauri::command]
fn cancel_core_maintenance(app: State<App>) {
    app.core.cancel();
}
#[tauri::command]
async fn use_managed_core(app: State<'_, App>) -> Result<Receipt, String> {
    let core = app.core.clone();
    tauri::async_runtime::spawn_blocking(move || core.use_managed())
        .await
        .map_err(|e| e.to_string())?
}
#[tauri::command]
fn choose_file(kind: String, handle: tauri::AppHandle, app: State<App>) -> Result<(), String> {
    if !["core", "config"].contains(&kind.as_str()) {
        return Err("未知文件类型".into());
    }
    app.runtime.stopped()?;
    if app.gate.closing() {
        return Err("应用正在退出".into());
    }
    if app.dialog_open.swap(true, Ordering::AcqRel) {
        return Err("已有文件选择框".into());
    }
    let app = app.inner().clone();
    let generation = app.core.generation();
    let dialog = if kind == "core" {
        handle.dialog().file().set_title("选择 sing-box 核心")
    } else {
        handle
            .dialog()
            .file()
            .set_title("导入 sing-box 配置")
            .add_filter("sing-box 配置", &["json", "jsonc"])
    };
    dialog.pick_file(move |path| {
        app.dialog_open.store(false, Ordering::Release);
        if app.gate.closing() || app.core.generation() != generation {
            return;
        }
        if let Some(path) = path {
            tauri::async_runtime::spawn_blocking(move || {
                let result = path
                    .into_path()
                    .map_err(|e| e.to_string())
                    .and_then(|path| {
                        if kind == "core" {
                            app.core.select(&path)
                        } else {
                            app.core.import(&path)
                        }
                    });
                if let Err(error) = result {
                    app.runtime.report(error);
                }
            });
        }
    });
    Ok(())
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
    let _operation = app.gate.begin(Kind::Preferences)?;
    let value = app.store.preferences(expected_revision, preferences)?;
    app.updates.reset_network();
    app.core.preferences_changed();
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
fn request_restart(handle: tauri::AppHandle, app: State<App>) -> Result<(), String> {
    if app.dirty.load(Ordering::Acquire) {
        return Err("请先保存或放弃下载网络设置".into());
    }
    lifecycle::request(app.inner().clone(), handle, true, true)
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
    lifecycle::request(app.inner().clone(), handle, false, false)
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
                let _ = lifecycle::request(app, handle, false, false);
            });
        }
    } else {
        let _ = lifecycle::request(app.clone(), handle.clone(), false, false);
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
            application.start(tauri.handle().clone(), !smoke);
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
            core_maintenance,
            cancel_core_maintenance,
            use_managed_core,
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
