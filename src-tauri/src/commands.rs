use sing_shell::{
    app::{Application, Kind, Receipt},
    core::Maintenance,
    lifecycle,
    session::Action,
    updates::{network, Component},
};
use std::sync::{atomic::Ordering, Arc};
use tauri::{Manager, State};
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_opener::OpenerExt;
type App = Arc<Application>;
#[tauri::command]
pub(crate) fn desktop_snapshot(
    handle: tauri::AppHandle,
    app: State<App>,
) -> sing_shell::desktop::Snapshot {
    app.desktop.snapshot(&handle, &app)
}
#[tauri::command]
pub(crate) fn desktop_navigation_done(id: u64, app: State<App>) {
    app.desktop.navigation_done(id);
}
#[tauri::command]
pub(crate) fn exit_presented(id: u64, document: String, app: State<App>) -> Result<(), String> {
    app.exit.presented(id, &document)
}
#[tauri::command]
pub(crate) fn exit_decision(
    id: u64,
    document: String,
    proceed: bool,
    handle: tauri::AppHandle,
    app: State<App>,
) -> Result<(), String> {
    lifecycle::decision(app.inner().clone(), handle, id, &document, proceed)
}
#[tauri::command]
pub(crate) async fn login_startup_read(
    app: State<'_, App>,
) -> Result<sing_shell::login_startup::Status, String> {
    let application = app.inner().clone();
    tauri::async_runtime::spawn_blocking(move || application.login.refresh())
        .await
        .map_err(|e| e.to_string())
}
#[tauri::command]
pub(crate) async fn login_startup_set(
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
pub(crate) fn set_restore_core(
    enabled: bool,
    app: State<App>,
) -> Result<serde_json::Value, String> {
    let _operation = app.gate.begin(Kind::Preferences)?;
    app.session.set_restore(enabled).map(|s| s.public())
}
#[tauri::command]
pub(crate) fn app_snapshot(app: State<App>) -> serde_json::Value {
    let settings = app.store.snapshot();
    let core = app.session.snapshot_for(&settings);
    let runtime = &core.runtime;
    let settings_value = settings.public();
    serde_json::json!({"settings":settings_value,"runtime":runtime,"runtimeBusy":app.gate.runtime_busy(),"core":core,"storageError":app.store.error(),"closing":app.gate.closing()})
}
#[tauri::command]
pub(crate) fn session_snapshot(app: State<App>) -> serde_json::Value {
    let s = app.store.snapshot();
    let core = app.session.snapshot_for(&s);
    serde_json::json!({"core":core,"runtime":core.runtime,"runtimeBusy":app.gate.runtime_busy(),"closing":app.gate.closing(),"storageError":app.store.error(),"revisions":[s.nodes_revision,s.routing_revision,s.core_revision,s.preferences_revision,s.connection_revision],"startup":{"restoreCoreOnLaunch":s.startup.restore_core_on_launch,"resumeCore":s.startup.resume_core}})
}
#[tauri::command]
pub(crate) fn read_events(
    after: u64,
    limit: usize,
    app: State<App>,
) -> Result<Vec<sing_shell::activity::Event>, String> {
    if limit == 0 || limit > 100 {
        return Err("事件分页超限".into());
    }
    Ok(app.runtime.events.page(after, limit))
}
#[tauri::command]
pub(crate) async fn profile_import_preview(
    input: String,
    app: State<'_, App>,
) -> Result<sing_shell::profile::service::Preview, String> {
    let application = app.inner().clone();
    tauri::async_runtime::spawn_blocking(move || application.profiles.preview(&input))
        .await
        .map_err(|e| e.to_string())?
}
#[tauri::command]
pub(crate) fn profile_import_cancel(app: State<App>) {
    app.profiles.cancel();
}
#[tauri::command]
pub(crate) fn profile_import_commit(
    ticket: u64,
    expected_revision: u64,
    app: State<App>,
) -> Result<sing_shell::profile::service::Snapshot, String> {
    app.profiles.commit(ticket, expected_revision)
}
#[tauri::command]
pub(crate) fn profile_rename_node(
    id: String,
    name: String,
    tags: Vec<String>,
    expected_revision: u64,
    app: State<App>,
) -> Result<sing_shell::profile::service::Snapshot, String> {
    app.profiles.rename(id, name, tags, expected_revision)
}
#[tauri::command]
pub(crate) fn profile_delete_node(
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
pub(crate) fn profile_snapshot(app: State<App>) -> sing_shell::profile::service::Snapshot {
    app.profiles.snapshot()
}
#[tauri::command]
pub(crate) fn rule_sources(app: State<App>) -> Vec<sing_shell::rulesets::service::Source> {
    app.rules.snapshot()
}
#[tauri::command]
pub(crate) fn rule_update(source_id: String, app: State<App>) -> Result<Receipt, String> {
    app.rules.update(source_id)
}
#[tauri::command]
pub(crate) fn rule_cancel(app: State<App>) {
    app.rules.cancel();
}
#[tauri::command]
pub(crate) fn rule_preference(
    preference: sing_shell::rulesets::service::Preference,
    app: State<App>,
) -> Result<(), String> {
    let _operation = app.gate.begin(Kind::Preferences)?;
    if app.store.snapshot().profile.mode != sing_shell::profile::Mode::Generated {
        return Err("原始配置模式下来源偏好只读".into());
    }
    app.store.rule_preference(preference)?;
    app.rules.reconcile();
    app.wake.notify_one();
    Ok(())
}
#[tauri::command]
pub(crate) async fn settings_reset(app: State<'_, App>) -> Result<String, String> {
    let session = app.session.clone();
    tauri::async_runtime::spawn_blocking(move || {
        session.reset_settings().map(|p| p.display().to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}
#[tauri::command]
pub(crate) fn routing_change(
    change: sing_shell::profile::change::Change,
    expected_revision: u64,
    app: State<App>,
) -> Result<sing_shell::profile::service::RoutingResult, sing_shell::profile::routing::Issue> {
    let result = app.profiles.change_routing(change, expected_revision)?;
    app.rules.reconcile();
    app.wake.notify_one();
    Ok(result)
}
#[tauri::command]
pub(crate) fn set_connection_options(
    options: sing_shell::profile::ConnectionOptions,
    expected_revision: u64,
    app: State<App>,
) -> Result<serde_json::Value, String> {
    let _operation = app.gate.begin(Kind::Profile)?;
    Ok(app
        .store
        .connection_options(expected_revision, options)?
        .public())
}
#[tauri::command]
pub(crate) fn session_select_node(
    node_id: String,
    expected_revision: u64,
    app: State<App>,
) -> Result<(), String> {
    app.session.select_node(node_id, expected_revision)
}
#[tauri::command]
pub(crate) fn session_connect(node_id: String, app: State<App>) -> Result<Receipt, String> {
    app.session.connect(node_id)
}
#[tauri::command]
pub(crate) async fn activity_snapshot(
    query: sing_shell::activity::Query,
    app: State<'_, App>,
) -> Result<sing_shell::activity::Page, String> {
    let session = app.session.clone();
    tauri::async_runtime::spawn_blocking(move || session.activity(query))
        .await
        .map_err(|e| e.to_string())?
}
#[tauri::command]
pub(crate) async fn read_logs(
    after: u64,
    limit: usize,
    app: State<'_, App>,
) -> Result<Vec<sing_shell::logs::Log>, String> {
    let session = app.session.clone();
    tauri::async_runtime::spawn_blocking(move || session.read_logs(after, limit))
        .await
        .map_err(|e| e.to_string())?
}
#[tauri::command]
pub(crate) fn core_action(action: Action, app: State<App>) -> Result<Receipt, String> {
    app.session.action(action)
}
#[tauri::command]
pub(crate) fn core_maintenance(action: Maintenance, app: State<App>) -> Result<Receipt, String> {
    app.core.maintain(Some(action))
}
#[tauri::command]
pub(crate) fn cancel_core_maintenance(app: State<App>) {
    app.session.cancel_core();
}
#[tauri::command]
pub(crate) async fn use_managed_core(app: State<'_, App>) -> Result<Receipt, String> {
    let session = app.session.clone();
    tauri::async_runtime::spawn_blocking(move || session.use_managed())
        .await
        .map_err(|e| e.to_string())?
}
#[tauri::command]
pub(crate) fn choose_file(
    kind: String,
    handle: tauri::AppHandle,
    app: State<App>,
) -> Result<(), String> {
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
pub(crate) fn update_snapshot(app: State<App>) -> sing_shell::updates::service::Snapshot {
    app.updates.snapshot()
}
#[tauri::command]
pub(crate) fn check_updates(
    component: Option<Component>,
    app: State<App>,
) -> Result<Receipt, String> {
    app.updates.check(component, false)
}
#[tauri::command]
pub(crate) fn download_updates(
    components: Vec<Component>,
    handle: tauri::AppHandle,
    app: State<App>,
) -> Result<Vec<Receipt>, String> {
    app.updates.download(handle, components)
}
#[tauri::command]
pub(crate) fn cancel_update(component: Component, app: State<App>) {
    app.gate.cancel_download(component);
}
#[tauri::command]
pub(crate) fn set_automatic_updates(
    component: Component,
    expected: bool,
    enabled: bool,
    app: State<App>,
) -> Result<serde_json::Value, String> {
    let _operation = app.gate.begin(Kind::Preferences)?;
    let value = app.store.automatic_updates(component, expected, enabled)?;
    app.core.preferences_changed();
    Ok(value.public())
}
#[tauri::command]
pub(crate) fn set_download_network(
    expected_revision: u64,
    network: network::Preferences,
    app: State<App>,
) -> Result<serde_json::Value, String> {
    let _operation = app.gate.begin(Kind::Preferences)?;
    let saved = app.store.network_preferences(expected_revision, network)?;
    app.updates.reset_network();
    app.core.preferences_changed();
    Ok(saved.public())
}
#[tauri::command]
pub(crate) async fn session_switch_mode(
    mode: sing_shell::profile::Mode,
    expected_revision: u64,
    app: State<'_, App>,
) -> Result<sing_shell::profile::service::Snapshot, String> {
    let session = app.session.clone();
    tauri::async_runtime::spawn_blocking(move || session.switch_mode(mode, expected_revision))
        .await
        .map_err(|e| e.to_string())?
}
#[tauri::command]
pub(crate) async fn test_update_mirror(
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
pub(crate) fn builtin_mirrors() -> Vec<network::BuiltinMirror> {
    network::BUILTIN_MIRRORS.to_vec()
}
#[tauri::command]
pub(crate) fn request_restart(handle: tauri::AppHandle, app: State<App>) -> Result<(), String> {
    lifecycle::request(
        app.inner().clone(),
        handle,
        lifecycle::Target::InstallAndRestart,
    )
}
#[tauri::command]
pub(crate) fn set_update_draft_dirty(dirty: bool, app: State<App>) {
    app.dirty.store(dirty, Ordering::Release);
}
#[tauri::command]
pub(crate) fn request_exit(handle: tauri::AppHandle, app: State<App>) -> Result<(), String> {
    lifecycle::request(app.inner().clone(), handle, lifecycle::Target::Quit)
}
#[tauri::command]
pub(crate) fn open_update_release(
    component: Component,
    handle: tauri::AppHandle,
) -> Result<(), String> {
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
