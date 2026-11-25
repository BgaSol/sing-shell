use crate::app::Application;
use std::sync::{atomic::Ordering, Arc};
use tauri::Emitter;
pub fn request(
    app: Arc<Application>,
    handle: tauri::AppHandle,
    restart: bool,
    install: bool,
) -> Result<(), String> {
    app.gate.close(restart)?;
    tauri::async_runtime::spawn_blocking(move || {
        let result = (|| {
            app.gate.wait()?;
            let prepared = if install {
                let p = app.updates.prepared(crate::updates::Component::App)?;
                app.updates.verify(&handle, &p)?;
                Some(p)
            } else {
                None
            };
            app.runtime.stop()?;
            if let Some(p) = prepared {
                app.updates.install(&handle, &p)?;
            }
            app.updates.stop();
            app.handed_off.store(true, Ordering::Release);
            if restart {
                handle.request_restart();
            } else {
                handle.exit(0);
            }
            Ok::<_, String>(())
        })();
        if let Err(error) = result {
            app.gate.reopen();
            app.runtime.report(error.clone());
            let _ = handle.emit("shutdown-error", error);
        }
    });
    Ok(())
}
