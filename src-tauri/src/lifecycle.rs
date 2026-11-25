use crate::app::Application;
use serde::{Deserialize, Serialize};
use std::sync::{atomic::Ordering, Arc, Mutex};
use tauri::Emitter;

#[derive(Clone, Copy, Debug, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Target {
    Quit,
    InstallAndRestart,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Request {
    pub id: u64,
    pub document: Option<String>,
    pub target: Target,
    pub presented: bool,
}
#[derive(Default)]
struct State {
    next: u64,
    document: Option<String>,
    had_document: bool,
    pending: Option<Request>,
}
#[derive(Default)]
pub struct Coordinator(Mutex<State>);
impl Coordinator {
    pub fn snapshot(&self) -> Option<Request> {
        self.0.lock().unwrap().pending.clone()
    }
    pub fn pending(&self) -> bool {
        self.0.lock().unwrap().pending.is_some()
    }
    pub fn page_started(&self) {
        self.0.lock().unwrap().document = None;
    }
    pub fn ready(&self, document: String) {
        let mut s = self.0.lock().unwrap();
        s.had_document = true;
        s.document = Some(document.clone());
        if let Some(p) = &mut s.pending {
            p.document = Some(document);
            p.presented = false;
        }
    }
    fn begin(&self, target: Target) -> Result<Option<u64>, String> {
        let mut s = self.0.lock().unwrap();
        if s.pending.is_some() {
            return Err("已有退出决定等待处理".into());
        }
        if !s.had_document {
            return Ok(None);
        }
        s.next += 1;
        let id = s.next;
        s.pending = Some(Request {
            id,
            document: s.document.clone(),
            target,
            presented: false,
        });
        Ok(Some(id))
    }
    fn timeout(&self, id: u64) -> bool {
        let mut s = self.0.lock().unwrap();
        if s.pending
            .as_ref()
            .is_some_and(|p| p.id == id && !p.presented)
        {
            s.pending = None;
            true
        } else {
            false
        }
    }
    pub fn presented(&self, id: u64, document: &str) -> Result<(), String> {
        let mut s = self.0.lock().unwrap();
        let p = s.pending.as_mut().ok_or("退出请求已结束")?;
        if p.id != id || p.document.as_deref() != Some(document) {
            return Err("退出回执已过期".into());
        }
        p.presented = true;
        Ok(())
    }
    fn decide(&self, id: u64, document: &str) -> Result<Target, String> {
        let mut s = self.0.lock().unwrap();
        let p = s.pending.as_ref().ok_or("退出请求已结束")?;
        if p.id != id || p.document.as_deref() != Some(document) {
            return Err("退出回执已过期".into());
        }
        Ok(s.pending.take().unwrap().target)
    }
}
pub fn request(
    app: Arc<Application>,
    handle: tauri::AppHandle,
    target: Target,
) -> Result<(), String> {
    if app.gate.closing() {
        return Err("应用正在退出或重启".into());
    }
    if app.dialog_open.load(Ordering::Acquire) {
        app.desktop.present(&handle, true)?;
        return Err("请先关闭文件选择框".into());
    }
    if app.exit.pending() {
        app.desktop.present(&handle, true)?;
        return Ok(());
    }
    match app.exit.begin(target)? {
        None => finish(app, handle, target),
        Some(id) => {
            app.desktop.present(&handle, true)?;
            let _ = handle.emit("desktop-changed", ());
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_secs(10)).await;
                if app.exit.timeout(id) {
                    let native = handle.clone();
                    let _ = handle.run_on_main_thread(move || {
                        app.desktop
                            .report(&native, "退出预检未能显示，请重新打开设置后重试")
                    });
                }
            });
            Ok(())
        }
    }
}
pub fn decision(
    app: Arc<Application>,
    handle: tauri::AppHandle,
    id: u64,
    document: &str,
    proceed: bool,
) -> Result<(), String> {
    let target = app.exit.decide(id, document)?;
    if proceed {
        finish(app, handle, target)
    } else {
        Ok(())
    }
}
fn finish(app: Arc<Application>, handle: tauri::AppHandle, target: Target) -> Result<(), String> {
    app.gate.close()?;
    app.core.invalidate_restore();
    tauri::async_runtime::spawn_blocking(move || {
        let result = (|| {
            app.gate.wait()?;
            let prepared = if target == Target::InstallAndRestart {
                let p = app.updates.prepared(crate::updates::Component::App)?;
                app.updates.verify(&handle, &p)?;
                Some(p)
            } else {
                None
            };
            app.runtime.stop()?;
            // NSIS can terminate inside install; required cleanup must precede it.
            app.updates.stop();
            app.handed_off.store(true, Ordering::Release);
            if let Some(p) = prepared {
                app.updates.install(&handle, &p)?;
                handle.request_restart();
            } else {
                handle.exit(0);
            }
            Ok::<_, String>(())
        })();
        if let Err(error) = result {
            app.handed_off.store(false, Ordering::Release);
            app.updates.resume();
            app.gate.reopen();
            app.runtime.report(error.clone());
            let native = handle.clone();
            let queued = handle.clone();
            let _ = queued.run_on_main_thread(move || {
                app.desktop.report(&native, &error);
                let _ = native.emit("shutdown-error", error);
            });
        }
    });
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reload_and_old_receipts_cannot_approve_another_document() {
        let c = Coordinator::default();
        c.ready("a".into());
        let id = c.begin(Target::Quit).unwrap().unwrap();
        c.presented(id, "a").unwrap();
        assert!(!c.timeout(id));
        c.page_started();
        c.ready("b".into());
        assert!(c.decide(id, "a").is_err());
        assert_eq!(c.decide(id, "b").unwrap(), Target::Quit);
        assert!(!c.pending());
    }
}
