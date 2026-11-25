use crate::{app::Application, updates::Component};
use std::sync::{atomic::Ordering, Arc};
use tauri::Emitter;
pub fn request(
    app: Arc<Application>,
    handle: tauri::AppHandle,
    restart: bool,
    components: Vec<Component>,
) -> Result<(), String> {
    if components.len() > 2
        || components
            .iter()
            .enumerate()
            .any(|(i, c)| components[..i].contains(c))
    {
        return Err("更新选择重复或无效".into());
    }
    app.gate.close(restart)?;
    tauri::async_runtime::spawn_blocking(move || {
        let result = (|| {
            app.gate.wait()?;
            let mut prepared = Vec::new();
            for c in &components {
                let p = app.updates.prepared(*c)?;
                app.updates.verify(&handle, &p)?;
                prepared.push(p);
            }
            app.runtime.stop()?;
            for p in prepared
                .iter()
                .filter(|p| p.candidate.component == Component::Engine)
            {
                app.updates.verify(&handle, p)?;
                app.updates.apply_engine(p)?;
            }
            for p in prepared
                .iter()
                .filter(|p| p.candidate.component == Component::App)
            {
                app.updates.install(&handle, p)?;
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
