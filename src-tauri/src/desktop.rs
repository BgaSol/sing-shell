use crate::{
    app::Application,
    core::{Action, Restore},
    lifecycle::{self, Target},
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
    projection: (String, bool, bool),
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
        let status = MenuItem::with_id(handle, "status", "核心已停止", true, None::<&str>)
            .map_err(|e| e.to_string())?;
        let show = MenuItem::with_id(handle, "show", "显示主窗口", true, None::<&str>)
            .map_err(|e| e.to_string())?;
        let action = MenuItem::with_id(handle, "core", "启动核心", false, None::<&str>)
            .map_err(|e| e.to_string())?;
        let settings = MenuItem::with_id(handle, "settings", "设置", true, None::<&str>)
            .map_err(|e| e.to_string())?;
        let quit = MenuItem::with_id(handle, "quit", "退出", true, Some("CmdOrCtrl+Q"))
            .map_err(|e| e.to_string())?;
        let menu = Menu::with_items(handle, &[&status, &show, &action, &settings, &quit])
            .map_err(|e| e.to_string())?;
        let tray = TrayIconBuilder::with_id("sing-shell")
            .icon(icon(false, false))
            .icon_as_template(cfg!(target_os = "macos"))
            .tooltip("Sing Shell · 核心已停止")
            .menu(&menu)
            .show_menu_on_left_click(!cfg!(windows))
            .on_menu_event(|handle, event| {
                let app = handle.state::<Arc<Application>>();
                let result = match event.id.as_ref() {
                    "quit" => lifecycle::request(app.inner().clone(), handle.clone(), Target::Quit),
                    "settings" => app.desktop.navigate(handle, "updates"),
                    "core" => {
                        let s = app.core.snapshot();
                        app.core
                            .action(if s.can_stop {
                                Action::Stop
                            } else {
                                Action::Start
                            })
                            .map(|_| ())
                    }
                    _ => app.desktop.navigate(handle, "runtime"),
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
            projection: (String::new(), false, false),
        });
        Ok(())
    }
    pub fn refresh(&self, app: &Application) -> Result<(), String> {
        let r = app.runtime.snapshot();
        let c = app.core.snapshot_for(&app.store.snapshot(), &r);
        let issue = r.error.is_some()
            || c.config_error.is_some()
            || c.maintenance.error.is_some()
            || app.store.error.is_some();
        let status = if r.status == "stopping" {
            "正在停止"
        } else if r.pid.is_some() {
            "核心运行中"
        } else if r.status == "starting" {
            "正在启动"
        } else if matches!(c.restore, Restore::Waiting) {
            "等待核心自动恢复"
        } else if issue {
            "需要处理 · 点击查看"
        } else {
            "核心已停止"
        };
        let next = (status.to_owned(), c.can_stop, c.can_start || c.can_stop);
        let mut s = self.state.lock().unwrap();
        if let Some(tray) = &mut s.tray {
            if tray.projection != next {
                tray.status.set_text(status).map_err(|e| e.to_string())?;
                tray.action
                    .set_text(if c.can_stop {
                        "停止核心"
                    } else {
                        "启动核心"
                    })
                    .map_err(|e| e.to_string())?;
                tray.action.set_enabled(next.2).map_err(|e| e.to_string())?;
                tray._icon
                    .set_tooltip(Some(format!("Sing Shell · {status}")))
                    .map_err(|e| e.to_string())?;
                tray._icon
                    .set_icon(Some(icon(r.pid.is_some(), issue && r.pid.is_none())))
                    .map_err(|e| e.to_string())?;
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
fn icon(running: bool, error: bool) -> tauri::image::Image<'static> {
    let mut rgba = vec![0; 32 * 32 * 4];
    for y in 0i32..32 {
        for x in 0i32..32 {
            let d = (x - 16) * (x - 16) + (y - 16) * (y - 16);
            let fill = if error {
                (14..=18).contains(&x) && (6..=25).contains(&y)
            } else if running {
                d <= 100
            } else {
                (65..=121).contains(&d)
            };
            if fill {
                let i = ((y * 32 + x) * 4) as usize;
                rgba[i..i + 4].copy_from_slice(if cfg!(target_os = "macos") {
                    &[0, 0, 0, 255]
                } else {
                    &[70, 135, 185, 255]
                });
            }
        }
    }
    tauri::image::Image::new_owned(rgba, 32, 32)
}
