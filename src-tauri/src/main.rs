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
use tauri::{Manager, State};
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_opener::OpenerExt;
type App = Arc<Application>;
struct Smoke {
    enabled: bool,
    directory: PathBuf,
    restore: bool,
}

#[tauri::command]
fn frontend_ready(
    document: String,
    app: tauri::AppHandle,
    smoke: State<Smoke>,
    application: State<App>,
) -> Result<(), String> {
    application.exit.ready(document);
    if smoke.enabled {
        let native = app.clone();
        let application = application.inner().clone();
        let restore = smoke.restore;
        tauri::async_runtime::spawn(async move {
            if restore {
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
                loop {
                    if application.core.snapshot().initialized {
                        application.core.restore_tick();
                    }
                    if application.runtime.snapshot().status == "running" {
                        break;
                    }
                    if std::time::Instant::now() >= deadline {
                        native.exit(1);
                        return;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                }
            }
            while !application.updates.snapshot().initialized {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
            tokio::time::sleep(std::time::Duration::from_millis(800)).await;
            let queued = native.clone();
            let _ = queued.run_on_main_thread(move || {
                if let Err(e) = smoke_close(&native, &application) {
                    application.desktop.report(&native, e);
                    native.exit(1);
                }
            });
        });
    }
    Ok(())
}
fn smoke_close(handle: &tauri::AppHandle, app: &App) -> Result<(), String> {
    app.store.background_notice()?;
    app.dirty.store(true, Ordering::Release);
    handle
        .get_webview_window("main")
        .ok_or("主窗口缺失")?
        .close()
        .map_err(|e| e.to_string())?;
    let native = handle.clone();
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        let queued = native.clone();
        let _ = queued.run_on_main_thread(move || {
            let result = (|| {
                let w = native.get_webview_window("main").ok_or("主窗口提前销毁")?;
                let intercepted = if cfg!(target_os = "linux") {
                    w.is_minimized().map_err(|e| e.to_string())?
                } else {
                    !w.is_visible().map_err(|e| e.to_string())?
                };
                if !intercepted {
                    return Err("关闭未收起主窗口".to_owned());
                }
                let smoke = native.state::<Smoke>();
                if smoke.restore {
                    let directory = smoke.directory.clone();
                    std::fs::write(
                        directory.join("smoke-hidden.json"),
                        serde_json::json!({"pid":app.runtime.snapshot().pid,"hidden":true})
                            .to_string(),
                    )
                    .map_err(|e| e.to_string())?;
                    let next = native.clone();
                    let application = app.clone();
                    tauri::async_runtime::spawn(async move {
                        let deadline =
                            std::time::Instant::now() + std::time::Duration::from_secs(20);
                        while !directory.join("smoke-continue").exists() {
                            if std::time::Instant::now() >= deadline {
                                next.exit(1);
                                return;
                            }
                            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                        }
                        let queued = next.clone();
                        let _ = queued.run_on_main_thread(move || {
                            if let Err(e) = smoke_finish(&next, &application, intercepted) {
                                application.runtime.report(e);
                                next.exit(1);
                            }
                        });
                    });
                    Ok(())
                } else {
                    smoke_finish(&native, &app, intercepted)
                }
            })();
            if let Err(e) = result {
                app.runtime.report(e);
                native.exit(1);
            }
        });
    });
    Ok(())
}
fn smoke_finish(native: &tauri::AppHandle, app: &App, intercepted: bool) -> Result<(), String> {
    let smoke = native.state::<Smoke>();
    sing_shell::login_startup::verify_native(&smoke.directory)?;
    let desktop = app.desktop.snapshot(native, app);
    let install = std::env::args_os().any(|a| a == "--install-test")
        && app
            .updates
            .snapshot()
            .items
            .get(&Component::App)
            .is_some_and(|i| i.phase == "ready");
    let report = serde_json::json!({"completed":true,"frontendReady":true,"closeRequestIntercepted":intercepted,"restoredPid":app.runtime.snapshot().pid,"reactivated":smoke.restore && desktop.visible,"installThroughLifecycle":install,"loginSource":std::env::args_os().any(|a|a=="--login-start"),"restartArgsNormalized":!native.env().args_os.iter().any(|a|a=="--login-start"),"draftRetained":app.dirty.load(Ordering::Acquire),"trayAvailable":desktop.tray_available,"loginIO":true,"version":native.package_info().version.to_string(),"target":sing_shell::TARGET,"settings":app.store.snapshot()});
    std::fs::write(
        smoke.directory.join("smoke-result.json"),
        report.to_string(),
    )
    .map_err(|e| e.to_string())?;
    std::fs::write(
        smoke.directory.join(format!(
            "smoke-result-{}.json",
            native.package_info().version
        )),
        report.to_string(),
    )
    .map_err(|e| e.to_string())?;
    lifecycle::request(
        app.clone(),
        native.clone(),
        if install {
            lifecycle::Target::InstallAndRestart
        } else {
            lifecycle::Target::Quit
        },
    )
}
#[tauri::command]
fn desktop_snapshot(handle: tauri::AppHandle, app: State<App>) -> sing_shell::desktop::Snapshot {
    app.desktop.snapshot(&handle, &app)
}
#[tauri::command]
fn desktop_navigation_done(id: u64, app: State<App>) {
    app.desktop.navigation_done(id);
}
#[tauri::command]
fn exit_presented(id: u64, document: String, app: State<App>) -> Result<(), String> {
    app.exit.presented(id, &document)
}
#[tauri::command]
fn exit_decision(
    id: u64,
    document: String,
    proceed: bool,
    handle: tauri::AppHandle,
    app: State<App>,
) -> Result<(), String> {
    lifecycle::decision(app.inner().clone(), handle, id, &document, proceed)
}
#[tauri::command]
async fn login_startup_read(
    app: State<'_, App>,
) -> Result<sing_shell::login_startup::Status, String> {
    let application = app.inner().clone();
    tauri::async_runtime::spawn_blocking(move || application.login.refresh())
        .await
        .map_err(|e| e.to_string())
}
#[tauri::command]
async fn login_startup_set(
    enabled: bool,
    app: State<'_, App>,
) -> Result<sing_shell::login_startup::Status, String> {
    let operation = app.gate.begin(Kind::Login)?;
    let application = app.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let result = application.login.set(enabled);
        drop(operation);
        result
    })
    .await
    .map_err(|e| e.to_string())?
}
#[tauri::command]
fn set_restore_core(
    enabled: bool,
    app: State<App>,
) -> Result<sing_shell::settings::Settings, String> {
    let _operation = app.gate.begin(Kind::Preferences)?;
    app.core.set_restore(enabled)
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
    app.desktop.present(&handle, true)?;
    let parent = handle.get_webview_window("main").ok_or("主窗口不可用")?;
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
        handle
            .dialog()
            .file()
            .set_parent(&parent)
            .set_title("选择 sing-box 核心")
    } else {
        handle
            .dialog()
            .file()
            .set_parent(&parent)
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
    lifecycle::request(
        app.inner().clone(),
        handle,
        lifecycle::Target::InstallAndRestart,
    )
}
#[tauri::command]
fn set_update_draft_dirty(dirty: bool, app: State<App>) {
    app.dirty.store(dirty, Ordering::Release);
}
#[tauri::command]
fn request_exit(handle: tauri::AppHandle, app: State<App>) -> Result<(), String> {
    lifecycle::request(app.inner().clone(), handle, lifecycle::Target::Quit)
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
fn main() {
    let args: Vec<_> = std::env::args_os().collect();
    let launch = sing_shell::startup::Launch::parse(&args).expect("启动参数无效");
    let app = tauri::Builder::default()
        .manage(sing_shell::startup::Launch::restart_environment(&args))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .on_page_load(|window, payload| {
            if matches!(payload.event(), tauri::webview::PageLoadEvent::Started) {
                if let Some(app) = window.try_state::<App>() {
                    app.exit.page_started();
                }
            }
        })
        .setup(move |tauri| {
            let native_login = sing_shell::login_startup::native_login_launch();
            let directory = if launch.smoke {
                launch.data.clone().ok_or("隔离目录缺失")?
            } else {
                tauri.path().app_data_dir()?
            };
            std::fs::create_dir_all(&directory)?;
            let directory = std::fs::canonicalize(directory)?;
            let Some(lease) = launch.admit(&directory)? else {
                std::process::exit(0);
            };
            let store = Arc::new(sing_shell::settings::SettingsStore::with_lease(
                directory.clone(),
                lease,
            )?);
            let application = Arc::new(Application::from_store(
                store,
                tauri.package_info().version.to_string(),
                launch.smoke,
            ));
            tauri.manage(application.clone());
            tauri.manage(Smoke {
                enabled: launch.smoke,
                directory: directory.clone(),
                restore: std::env::args_os().any(|a| a == "--restore-test"),
            });
            let native = tauri.handle().clone();
            let weak = Arc::downgrade(&application);
            let server = sing_shell::instance::Endpoint::new(&directory)?.listen(move || {
                let Some(app) = weak.upgrade() else {
                    return false;
                };
                if app.gate.closing() || app.handed_off.load(Ordering::Acquire) {
                    return false;
                }
                let handle = native.clone();
                native
                    .run_on_main_thread(move || {
                        if !app.gate.closing() {
                            if let Err(e) = app.desktop.present(&handle, true) {
                                app.desktop.report(&handle, e);
                            }
                        }
                    })
                    .is_ok()
            })?;
            tauri.manage(server);
            if let Err(e) = application.desktop.install(tauri.handle()) {
                application.desktop.report(tauri.handle(), e);
            }
            if (launch.login || native_login) && !launch.restart {
                if let Err(e) = application.desktop.hide(tauri.handle(), &application) {
                    application.desktop.report(tauri.handle(), e);
                }
            } else {
                application.desktop.present(tauri.handle(), false)?;
            }
            application.start(tauri.handle().clone(), !launch.smoke);
            let native = tauri.handle().clone();
            let weak = Arc::downgrade(&application);
            tauri::async_runtime::spawn(async move {
                loop {
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                    let Some(app) = weak.upgrade() else {
                        break;
                    };
                    if app.handed_off.load(Ordering::Acquire) {
                        break;
                    }
                    let handle = native.clone();
                    let _ = native.run_on_main_thread(move || {
                        if let Err(e) = app.desktop.refresh(&app) {
                            app.desktop.report(&handle, e);
                        }
                    });
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            frontend_ready,
            desktop_snapshot,
            desktop_navigation_done,
            exit_presented,
            exit_decision,
            login_startup_read,
            login_startup_set,
            set_restore_core,
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
                app.desktop.close_requested(handle, app.inner());
            }
        }
        tauri::RunEvent::ExitRequested { api, .. } => {
            let app = handle.state::<App>();
            if !app.handed_off.load(Ordering::Acquire) {
                api.prevent_exit();
                if let Err(e) =
                    lifecycle::request(app.inner().clone(), handle.clone(), lifecycle::Target::Quit)
                {
                    app.desktop.report(handle, e);
                }
            }
        }
        #[cfg(target_os = "macos")]
        tauri::RunEvent::Reopen { .. } => {
            let app = handle.state::<App>();
            let _ = app.desktop.present(handle, true);
        }
        _ => {}
    });
}
