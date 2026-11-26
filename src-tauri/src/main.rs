#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
use sing_shell::{
    app::{Application, Kind, Receipt},
    core::Maintenance,
    lifecycle,
    session::Action,
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
    ready: std::sync::atomic::AtomicBool,
}

#[tauri::command]
fn frontend_ready(
    document: String,
    app: tauri::AppHandle,
    smoke: State<Smoke>,
    application: State<App>,
) -> Result<(), String> {
    application.exit.ready(document);
    smoke.ready.store(true, Ordering::Release);
    if smoke.enabled && !smoke.restore {
        run_smoke(app, application.inner().clone(), false);
    }
    Ok(())
}
fn run_smoke(native: tauri::AppHandle, application: App, restore: bool) {
    tauri::async_runtime::spawn(async move {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while !application.updates.snapshot().initialized
            || (restore && application.runtime.snapshot().status != "running")
        {
            if std::time::Instant::now() >= deadline {
                let queued = native.clone();
                let _ = queued.run_on_main_thread(move || {
                    smoke_failure(&native, &application, "隔离恢复/初始化超时")
                });
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        tokio::time::sleep(std::time::Duration::from_millis(800)).await;
        let queued = native.clone();
        let _ = queued.run_on_main_thread(move || {
            if let Err(e) = smoke_close(&native, &application) {
                smoke_failure(&native, &application, e);
            }
        });
    });
}
fn smoke_failure(handle: &tauri::AppHandle, app: &App, error: impl ToString) {
    let message = error.to_string();
    let value = serde_json::json!({"error":message,"desktop":app.desktop.snapshot(handle,app),"runtime":app.runtime.snapshot(),"core":app.session.snapshot()});
    let _ = std::fs::write(
        handle.state::<Smoke>().directory.join("smoke-error.json"),
        value.to_string(),
    );
    eprintln!("Sing Shell smoke failed: {value}");
    app.runtime.report(message);
    handle.exit(1);
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
                let desktop=app.desktop.snapshot(&native,&app);
                let fallback=!cfg!(target_os="linux") && !desktop.tray_available && desktop.visible && desktop.error.is_some();
                if !intercepted && !fallback {return Err(format!("关闭未收起主窗口，桌面状态：{}",serde_json::to_string(&desktop).unwrap()));}
                let accepted=intercepted || fallback;
                let smoke = native.state::<Smoke>();
                if smoke.restore {
                    let directory = smoke.directory.clone();
                    std::fs::write(
                        directory.join("smoke-hidden.json"),
                        serde_json::json!({"pid":app.runtime.snapshot().pid,"hidden":intercepted,"fallback":fallback})
                            .to_string(),
                    )
                    .map_err(|e| e.to_string())?;
                    let next = native.clone();
                    let application = app.clone();
                    tauri::async_runtime::spawn(async move {
                        let deadline =
                            std::time::Instant::now() + std::time::Duration::from_secs(20);
                        while !directory.join("smoke-continue").exists() || !next.state::<Smoke>().ready.load(Ordering::Acquire) {
                            if std::time::Instant::now() >= deadline {
                                next.exit(1);
                                return;
                            }
                            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                        }
                        let queued = next.clone();
                        let _ = queued.run_on_main_thread(move || {
                            if let Err(e) = smoke_finish(&next, &application, accepted) {
                                smoke_failure(&next,&application,e);
                            }
                        });
                    });
                    Ok(())
                } else {
                    smoke_finish(&native, &app, accepted)
                }
            })();
            if let Err(e) = result {
                smoke_failure(&native,&app,e);
            }
        });
    });
    Ok(())
}
fn smoke_finish(native: &tauri::AppHandle, app: &App, intercepted: bool) -> Result<(), String> {
    let smoke = native.state::<Smoke>();
    sing_shell::login_startup::verify_native(&smoke.directory)?;
    let desktop = app.desktop.snapshot(native, app);
    let login_registration: Option<serde_json::Value> =
        std::fs::read(smoke.directory.join("login-registration.json"))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok());
    let install = std::env::args_os().any(|a| a == "--install-test")
        && app
            .updates
            .snapshot()
            .items
            .get(&Component::App)
            .is_some_and(|i| i.phase == "ready");
    let report = serde_json::json!({"completed":true,"frontendReady":true,"closeRequestIntercepted":intercepted,"restoredPid":app.runtime.snapshot().pid,"reactivated":smoke.restore && desktop.visible,"installThroughLifecycle":install,"loginSource":std::env::args_os().any(|a|a=="--login-start"),"restartArgsNormalized":!native.env().args_os.iter().any(|a|a=="--login-start"),"draftRetained":app.dirty.load(Ordering::Acquire),"trayAvailable":desktop.tray_available,"backgroundFallback":!cfg!(target_os="linux") && !desktop.tray_available,"loginIO":true,"loginRegistration":login_registration,"version":native.package_info().version.to_string(),"target":sing_shell::TARGET,"settings":app.store.snapshot().public()});
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
fn set_restore_core(enabled: bool, app: State<App>) -> Result<serde_json::Value, String> {
    let _operation = app.gate.begin(Kind::Preferences)?;
    app.session.set_restore(enabled).map(|s| s.public())
}
#[tauri::command]
fn core_snapshot(app: State<App>) -> serde_json::Value {
    let settings = app.store.snapshot();
    let runtime = app.session.runtime_snapshot();
    let core = app.session.snapshot_for(&settings, &runtime);
    let settings_value = settings.public();
    serde_json::json!({"settings":settings_value,"runtime":runtime,"runtimeBusy":app.gate.runtime_busy(),"core":core,"logs":app.session.logs(),"storageError":app.store.error,"closing":app.gate.closing()})
}
#[tauri::command]
async fn profile_import_preview(
    input: String,
    app: State<'_, App>,
) -> Result<sing_shell::profile::service::Preview, String> {
    let application = app.inner().clone();
    tauri::async_runtime::spawn_blocking(move || application.profiles.preview(&input))
        .await
        .map_err(|e| e.to_string())?
}
#[tauri::command]
fn profile_import_cancel(app: State<App>) {
    app.profiles.cancel();
}
#[tauri::command]
fn profile_import_commit(
    ticket: u64,
    expected_revision: u64,
    app: State<App>,
) -> Result<sing_shell::profile::service::Snapshot, String> {
    app.profiles.commit(ticket, expected_revision)
}
#[tauri::command]
fn profile_select_node(
    id: String,
    expected_revision: u64,
    app: State<App>,
) -> Result<sing_shell::profile::service::Snapshot, String> {
    app.profiles.select(id, expected_revision)
}
#[tauri::command]
fn profile_rename_node(
    id: String,
    name: String,
    expected_revision: u64,
    app: State<App>,
) -> Result<sing_shell::profile::service::Snapshot, String> {
    app.profiles.rename(id, name, expected_revision)
}
#[tauri::command]
fn profile_delete_node(
    id: String,
    expected_revision: u64,
    app: State<App>,
) -> Result<sing_shell::profile::service::Snapshot, String> {
    if app.session.runtime_snapshot().pid.is_some() {
        return Err("运行时请先停止，再删除节点".into());
    }
    app.profiles.delete(id, expected_revision)
}
#[tauri::command]
fn profile_snapshot(app: State<App>) -> sing_shell::profile::service::Snapshot {
    app.profiles.snapshot()
}
#[tauri::command]
fn profile_mode(
    mode: sing_shell::profile::Mode,
    expected_revision: u64,
    app: State<App>,
) -> Result<sing_shell::profile::service::Snapshot, String> {
    app.session.mode(mode, expected_revision)
}
#[tauri::command]
async fn profile_update_rules(
    expected_revision: u64,
    app: State<'_, App>,
) -> Result<sing_shell::profile::service::Snapshot, String> {
    let operation = app.gate.begin(Kind::Rules)?;
    let application = app.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let commit = sing_shell::rulesets::latest(&application.store.root, &operation.cancel)?;
        application.store.profile(expected_revision, |p| {
            p.rule_commit = commit;
            Ok(())
        })?;
        Ok(application.profiles.snapshot())
    })
    .await
    .map_err(|e| e.to_string())?
}
#[tauri::command]
fn profile_set_routing(
    routing: sing_shell::profile::routing::Routing,
    expected_revision: u64,
    app: State<App>,
) -> Result<sing_shell::profile::service::Snapshot, String> {
    app.profiles.routing(routing, expected_revision)
}
#[tauri::command]
fn core_action(action: Action, app: State<App>) -> Result<Receipt, String> {
    app.session.action(action)
}
#[tauri::command]
fn core_maintenance(action: Maintenance, app: State<App>) -> Result<Receipt, String> {
    app.core.maintain(Some(action))
}
#[tauri::command]
fn cancel_core_maintenance(app: State<App>) {
    app.session.cancel_core();
}
#[tauri::command]
async fn use_managed_core(app: State<'_, App>) -> Result<Receipt, String> {
    let session = app.session.clone();
    tauri::async_runtime::spawn_blocking(move || session.use_managed())
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
    app.session.ensure_stopped()?;
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
                            app.session.select_core(&path)
                        } else {
                            app.session.import_config(&path)
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
) -> Result<serde_json::Value, String> {
    let _operation = app.gate.begin(Kind::Preferences)?;
    let value = app.store.preferences(expected_revision, preferences)?;
    app.updates.reset_network();
    app.core.preferences_changed();
    Ok(value.public())
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
fn open_network_settings() -> Result<(), String> {
    sing_shell::session::open_network_settings()
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
    if let Some(result) = sing_shell::session::tunnel::dispatch(&args) {
        if let Err(error) = result {
            eprintln!("Sing Shell network helper: {error}");
            std::process::exit(1);
        }
        return;
    }
    if args.iter().any(|a| a == "--smoke-test")
        && std::env::var("SING_SHELL_NETWORK_PREFLIGHT").as_deref() == Ok("1")
    {
        if let Some(index) = args.iter().position(|a| a == "--network-fixture-input") {
            let file = args.get(index + 1).expect("network fixture input missing");
            let fixture_args = vec![args[0].clone(), file.clone()];
            if let Err(error) = sing_shell::session::tunnel::fixture(&fixture_args) {
                eprintln!("{error}");
                std::process::exit(1);
            }
            return;
        }
    }
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
            let mut launch = launch;
            launch.login = !launch.restart && (launch.login || native_login);
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
                ready: std::sync::atomic::AtomicBool::new(false),
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
            let restore_test = launch.smoke && tauri.state::<Smoke>().restore;
            application.start(tauri.handle().clone(), !launch.smoke, restore_test);
            if restore_test {
                run_smoke(tauri.handle().clone(), application.clone(), true);
            }
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
                            app.desktop.tray_failed(&handle, e);
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
            open_network_settings,
            set_update_draft_dirty,
            open_update_release,
            profile_import_preview,
            profile_import_cancel,
            profile_snapshot,
            profile_mode,
            profile_update_rules,
            profile_rename_node,
            profile_delete_node,
            profile_import_commit,
            profile_select_node,
            profile_set_routing
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
