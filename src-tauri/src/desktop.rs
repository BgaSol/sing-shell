use crate::{
    app::Application,
    lifecycle::{self, Target},
    session::{Action, Restore},
};
use serde::Serialize;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent},
    Emitter, Manager,
};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons};

#[derive(Default)]
pub struct Desktop {
    state: Mutex<State>,
    notice_open: AtomicBool,
}
#[derive(Default)]
struct State {
    tray: Option<Tray>,
    error: Option<String>,
    navigation: Option<Navigation>,
    next: u64,
}
struct Tray {
    _icon: TrayIcon,
    status: MenuItem<tauri::Wry>,
    action: MenuItem<tauri::Wry>,
    projection: Projection,
}
#[derive(Clone, Copy, Debug, PartialEq)]
enum IconState {
    Idle,
    Busy,
    Running,
    Attention,
}
#[derive(Clone, Debug, PartialEq)]
struct Projection {
    status: &'static str,
    action: &'static str,
    enabled: bool,
    icon: IconState,
}
impl Projection {
    fn for_session(s: &crate::session::Snapshot, storage_error: bool) -> Self {
        let issue = s.runtime.error.is_some()
            || s.core.config_error.is_some()
            || s.core.maintenance.error.is_some()
            || storage_error;
        let (status, icon) = if s.runtime.cleanup_pending {
            ("网络资源待清理 · 点击查看", IconState::Attention)
        } else if s.phase == "stopping" {
            ("正在停止", IconState::Busy)
        } else if s.phase == "authorizing" {
            ("正在请求网络授权", IconState::Busy)
        } else if matches!(s.phase.as_str(), "preparing" | "starting") {
            ("正在启动", IconState::Busy)
        } else if s.phase == "running" && s.runtime.pid.is_some() {
            (
                if issue {
                    "核心运行中 · 有问题待处理"
                } else {
                    "核心运行中"
                },
                IconState::Running,
            )
        } else if s.restore == Restore::Waiting {
            ("等待核心自动恢复", IconState::Busy)
        } else if issue {
            ("需要处理 · 点击查看", IconState::Attention)
        } else {
            ("核心已停止", IconState::Idle)
        };
        Self {
            status,
            action: if s.can_stop {
                "停止核心"
            } else {
                "启动核心"
            },
            enabled: s.can_start
                || s.can_stop
                || (s.mode == crate::profile::Mode::Generated
                    && s.core.initialized
                    && !storage_error),
            icon,
        }
    }
    fn tooltip(&self) -> String {
        format!("Sing Shell · {}", self.status)
    }
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Navigation {
    pub id: u64,
    pub target: String,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub tray_available: bool,
    pub visible: bool,
    pub error: Option<String>,
    pub navigation: Option<Navigation>,
    pub exit_request: Option<lifecycle::Request>,
}
impl Desktop {
    pub fn modal_active(&self) -> bool {
        self.notice_open.load(Ordering::Acquire)
    }
    pub fn snapshot(&self, handle: &tauri::AppHandle, app: &Application) -> Snapshot {
        let s = self.state.lock().unwrap();
        Snapshot {
            tray_available: s.tray.is_some(),
            visible: handle
                .get_webview_window("main")
                .is_some_and(|w| w.is_visible().unwrap_or(false)),
            error: s.error.clone(),
            navigation: s.navigation.clone(),
            exit_request: app.exit.snapshot(),
        }
    }
    pub fn report(&self, handle: &tauri::AppHandle, error: impl ToString) {
        self.state.lock().unwrap().error = Some(error.to_string());
        let _ = self.present(handle, true);
    }
    pub fn tray_failed(&self, handle: &tauri::AppHandle, error: impl ToString) {
        let tray = self.state.lock().unwrap().tray.take();
        drop(tray);
        self.report(handle, error);
    }
    pub fn present(&self, handle: &tauri::AppHandle, focus: bool) -> Result<(), String> {
        #[cfg(target_os = "macos")]
        handle
            .set_activation_policy(tauri::ActivationPolicy::Regular)
            .map_err(|e| e.to_string())?;
        let w = handle.get_webview_window("main").ok_or("主窗口不可用")?;
        w.unminimize().map_err(|e| e.to_string())?;
        w.show().map_err(|e| e.to_string())?;
        if focus {
            w.set_focus().map_err(|e| e.to_string())?;
        }
        Ok(())
    }
    pub fn navigate(&self, handle: &tauri::AppHandle, target: &str) -> Result<(), String> {
        {
            let mut s = self.state.lock().unwrap();
            s.next += 1;
            s.navigation = Some(Navigation {
                id: s.next,
                target: target.into(),
            });
        }
        self.present(handle, true)?;
        let _ = handle.emit("desktop-changed", ());
        Ok(())
    }
    pub fn navigation_done(&self, id: u64) {
        let mut s = self.state.lock().unwrap();
        if s.navigation.as_ref().is_some_and(|n| n.id == id) {
            s.navigation = None;
        }
    }
    pub fn install(&self, handle: &tauri::AppHandle) -> Result<(), String> {
        let app = handle.state::<Arc<Application>>();
        let projection =
            Projection::for_session(&app.session.snapshot(), app.store.error().is_some());
        let status = MenuItem::with_id(handle, "status", projection.status, true, None::<&str>)
            .map_err(|e| e.to_string())?;
        let show = MenuItem::with_id(handle, "show", "显示主窗口", true, None::<&str>)
            .map_err(|e| e.to_string())?;
        let action = MenuItem::with_id(
            handle,
            "core",
            projection.action,
            projection.enabled,
            None::<&str>,
        )
        .map_err(|e| e.to_string())?;
        let settings = MenuItem::with_id(handle, "settings", "设置", true, None::<&str>)
            .map_err(|e| e.to_string())?;
        let quit = MenuItem::with_id(handle, "quit", "退出", true, Some("CmdOrCtrl+Q"))
            .map_err(|e| e.to_string())?;
        let menu = Menu::with_items(handle, &[&status, &show, &action, &settings, &quit])
            .map_err(|e| e.to_string())?;
        let tray = TrayIconBuilder::with_id("sing-shell")
            .icon(icon(projection.icon))
            .icon_as_template(cfg!(target_os = "macos"))
            .tooltip(projection.tooltip())
            .menu(&menu)
            .show_menu_on_left_click(!cfg!(windows))
            .on_menu_event(|handle, event| {
                let app = handle.state::<Arc<Application>>();
                let result = match event.id.as_ref() {
                    "quit" => lifecycle::request(app.inner().clone(), handle.clone(), Target::Quit),
                    "settings" => app.desktop.navigate(handle, "settings"),
                    "core" => {
                        let s = app.session.snapshot();
                        if s.can_stop {
                            app.session.action(Action::Stop).map(|_| ())
                        } else if app.store.snapshot().profile.mode
                            == crate::profile::Mode::Generated
                        {
                            if let Some(id) = app.store.snapshot().profile.selected_node {
                                app.session.connect(id).map(|_| ())
                            } else {
                                app.desktop.navigate(handle, "nodes")
                            }
                        } else {
                            app.session.action(Action::Start).map(|_| ())
                        }
                    }
                    _ => app.desktop.navigate(handle, "nodes"),
                };
                if let Err(e) = result {
                    app.desktop.report(handle, e);
                }
            })
            .on_tray_icon_event(|tray, event| {
                if matches!(
                    event,
                    TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    }
                ) && cfg!(windows)
                {
                    let handle = tray.app_handle();
                    let app = handle.state::<Arc<Application>>();
                    if let Err(e) = app.desktop.present(handle, true) {
                        app.desktop.report(handle, e);
                    }
                }
            })
            .build(handle)
            .map_err(|e| e.to_string())?;
        self.state.lock().unwrap().tray = Some(Tray {
            _icon: tray,
            status,
            action,
            projection,
        });
        Ok(())
    }
    pub fn refresh(&self, app: &Application) -> Result<(), String> {
        let next = Projection::for_session(&app.session.snapshot(), app.store.error().is_some());
        let mut s = self.state.lock().unwrap();
        if let Some(tray) = &mut s.tray {
            if tray.projection != next {
                if tray.projection.status != next.status {
                    tray.status
                        .set_text(next.status)
                        .map_err(|e| e.to_string())?;
                    tray._icon
                        .set_tooltip(Some(next.tooltip()))
                        .map_err(|e| e.to_string())?;
                }
                if tray.projection.action != next.action {
                    tray.action
                        .set_text(next.action)
                        .map_err(|e| e.to_string())?;
                }
                if tray.projection.enabled != next.enabled {
                    tray.action
                        .set_enabled(next.enabled)
                        .map_err(|e| e.to_string())?;
                }
                if tray.projection.icon != next.icon {
                    tray._icon
                        .set_icon_with_as_template(Some(icon(next.icon)), cfg!(target_os = "macos"))
                        .map_err(|e| e.to_string())?;
                }
                tray.projection = next;
            }
        }
        Ok(())
    }
    pub fn hide(&self, handle: &tauri::AppHandle, app: &Application) -> Result<(), String> {
        if app.dialog_open.load(Ordering::Acquire)
            || app.exit.pending()
            || self.notice_open.load(Ordering::Acquire)
        {
            return self.present(handle, true);
        }
        let w = handle.get_webview_window("main").ok_or("主窗口不可用")?;
        if cfg!(target_os = "linux") {
            w.show().map_err(|e| e.to_string())?;
            return w.minimize().map_err(|e| e.to_string());
        }
        if self.state.lock().unwrap().tray.is_none() {
            return Err("托盘不可用，已保留主窗口".into());
        }
        w.hide().map_err(|e| e.to_string())?;
        #[cfg(target_os = "macos")]
        if let Err(e) = handle.set_activation_policy(tauri::ActivationPolicy::Accessory) {
            let _ = w.show();
            return Err(e.to_string());
        }
        Ok(())
    }
    pub fn close_requested(&self, handle: &tauri::AppHandle, app: &Arc<Application>) {
        if app.gate.closing() {
            return;
        }
        if app.dialog_open.load(Ordering::Acquire) || app.exit.pending() {
            let _ = self.present(handle, true);
            return;
        }
        if app.store.snapshot().desktop.background_notice_shown {
            if let Err(e) = self.hide(handle, app) {
                self.report(handle, e);
            }
            return;
        }
        if self.notice_open.swap(true, Ordering::AcqRel) {
            return;
        }
        let native = handle.clone();
        let app = app.clone();
        handle.dialog().message(if cfg!(target_os="linux") {"关闭窗口会最小化，核心和设置草稿继续保留。可从系统窗口入口返回；请使用设置中的“退出应用”真正退出。"}else{"关闭窗口后 Sing Shell 仍在托盘运行，核心和设置草稿继续保留。点击托盘可返回，选择“退出”会停止核心并退出。"})
            .title("Sing Shell 将继续运行").buttons(MessageDialogButtons::OkCancel).show(move |yes| {
                let queued=native.clone();
                let _=queued.run_on_main_thread(move || {
                    app.desktop.notice_open.store(false,Ordering::Release);
                    if yes {
                        match app.desktop.hide(&native,&app) {
                            Ok(())=>if let Err(e)=app.store.background_notice(){app.desktop.report(&native,e);},
                            Err(e)=>app.desktop.report(&native,e),
                        }
                    }
                });
            });
    }
}
fn icon(state: IconState) -> tauri::image::Image<'static> {
    #[cfg(target_os = "macos")]
    let image = match state {
        IconState::Idle => tauri::include_image!("icons/tray-template-idle@2x.png"),
        IconState::Busy => tauri::include_image!("icons/tray-template-busy@2x.png"),
        IconState::Running => tauri::include_image!("icons/tray-template-running@2x.png"),
        IconState::Attention => tauri::include_image!("icons/tray-template-attention@2x.png"),
    };
    #[cfg(not(target_os = "macos"))]
    let image = match state {
        IconState::Idle => tauri::include_image!("icons/tray-idle.png"),
        IconState::Busy => tauri::include_image!("icons/tray-busy.png"),
        IconState::Running => tauri::include_image!("icons/tray-running.png"),
        IconState::Attention => tauri::include_image!("icons/tray-attention.png"),
    };
    image
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cleanup_startup_and_maintenance_have_distinct_tray_projections() {
        let root = std::env::temp_dir().join(format!(
            "sing-shell-tray-{}-{}",
            std::process::id(),
            crate::logs::now()
        ));
        let app = Application::open(root.clone(), "0.0.0".into()).unwrap();
        let mut s = app.session.snapshot();
        s.phase = "running".into();
        s.runtime.pid = Some(42);
        s.can_stop = true;
        assert_eq!(Projection::for_session(&s, false).icon, IconState::Running);
        s.core.maintenance.error = Some("download failed".into());
        assert_eq!(Projection::for_session(&s, false).icon, IconState::Running);
        s.runtime.cleanup_pending = true;
        assert_eq!(
            Projection::for_session(&s, false).icon,
            IconState::Attention
        );
        s.runtime.cleanup_pending = false;
        s.phase = "starting".into();
        assert_eq!(Projection::for_session(&s, false).icon, IconState::Busy);
        s.runtime.pid = None;
        s.phase = "stopped".into();
        s.core.maintenance.error = None;
        assert_eq!(Projection::for_session(&s, false).icon, IconState::Idle);
        for state in [
            IconState::Idle,
            IconState::Busy,
            IconState::Running,
            IconState::Attention,
        ] {
            let image = icon(state);
            assert_eq!(
                image.rgba().len(),
                image.width() as usize * image.height() as usize * 4
            );
            assert!(image.rgba().chunks_exact(4).any(|p| p[3] > 0 && p[3] < 255));
        }
        drop(app);
        let _ = std::fs::remove_dir_all(root);
    }
}
