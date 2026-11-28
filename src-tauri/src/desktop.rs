use crate::{app::Application, lifecycle};
use serde::Serialize;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use tauri::{Emitter, Manager};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons};
mod tray;
use tray::Tray;

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
        self.state.lock().unwrap().tray = Some(Tray::install(handle)?);
        Ok(())
    }
    pub fn refresh(&self, app: &Application) -> Result<(), String> {
        if let Some(tray) = &mut self.state.lock().unwrap().tray {
            tray.refresh(app)?;
        }
        Ok(())
    }
    pub fn verify_native_menu(&self) -> Result<Option<serde_json::Value>, String> {
        self.state
            .lock()
            .unwrap()
            .tray
            .as_mut()
            .map(Tray::verify_native)
            .transpose()
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
            let error = self.state.lock().unwrap().error.clone();
            return Err(format!(
                "托盘不可用，已保留主窗口{}",
                error.map(|e| format!("：{e}")).unwrap_or_default()
            ));
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
        handle.dialog().message(if cfg!(target_os="linux") {"关闭窗口会最小化，核心和设置草稿继续保留。可从系统窗口入口返回；请使用应用菜单中的“退出应用”真正退出。"}else{"关闭窗口后 Sing Shell 仍在托盘运行，核心和设置草稿继续保留。点击托盘可返回，选择“退出”会停止核心并退出。"})
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
