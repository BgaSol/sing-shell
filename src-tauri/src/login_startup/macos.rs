use super::{command_path, Status};
use std::path::Path;
#[path = "macos/legacy.rs"]
mod legacy;
#[path = "macos/native.rs"]
mod native;
pub fn read() -> Result<Status, String> {
    native::bundle(&command_path()?, false)?;
    if native::available() {
        let (enabled, state, reason) = match native::status()? {
            native::State::NotRegistered => (false, "disabled", None),
            native::State::Enabled => (true, "enabled", None),
            native::State::RequiresApproval => (
                false,
                "blocked",
                Some("请在系统设置 → 通用 → 登录项中允许 Sing Shell".into()),
            ),
            native::State::NotFound => (
                false,
                "stale",
                Some("系统未找到已登记的应用，请重新安装".into()),
            ),
        };
        Ok(Status {
            enabled,
            state: state.into(),
            reason,
        })
    } else {
        Ok(match legacy::inspect(&command_path()?)? {
            Some(e) => Status {
                enabled: e.enabled,
                state: if e.enabled { "enabled" } else { "blocked" }.into(),
                reason: if e.enabled {
                    None
                } else {
                    Some("登录项被系统禁用".into())
                },
            },
            None => Status::disabled(),
        })
    }
}
pub fn set(enabled: bool) -> Result<(), String> {
    if native::available() {
        if enabled {
            native::register_bundle(&native::bundle(&command_path()?, true)?)?;
        }
        native::set(enabled)
    } else {
        let old = legacy::inspect(&command_path()?)?;
        if enabled {
            legacy::write(&command_path()?)
        } else {
            if let Some(entry) = old {
                legacy::remove(&entry)?;
            }
            Ok(())
        }
    }
}
pub fn installed(executable: &Path) -> Result<(), String> {
    native::bundle(executable, true).map(|_| ())
}
pub(super) fn verify(directory: &Path) -> Result<(), String> {
    native::bundle(&command_path()?, false)?;
    if native::available() {
        let _ = native::status()?;
    }
    legacy::verify(directory)
}
pub fn launched_at_login() -> bool {
    native::launched_at_login()
}
pub(super) fn verify_registration(directory: &Path) -> Result<(), String> {
    let bundle = native::bundle(&command_path()?, true)?;
    let home = std::path::PathBuf::from(std::env::var_os("HOME").ok_or("无法确定 CI 用户")?);
    let applications =
        std::fs::canonicalize(home.join("Applications")).map_err(|e| e.to_string())?;
    let actual = std::fs::canonicalize(&bundle.path).map_err(|e| e.to_string())?;
    if !actual.starts_with(&applications) {
        return Err(format!(
            "登录登记验收安装位置不符：{}，要求 {}",
            actual.display(),
            applications.display()
        ));
    }
    native::register_bundle(&bundle)?;
    let before = native::status()?;
    if !matches!(
        before,
        native::State::NotRegistered | native::State::NotFound
    ) {
        return Err(format!("CI 登录项初态不是未登记：{before:?}；保留系统状态"));
    }
    let registered = (|| -> Result<Status, String> {
        set(true)?;
        let status = read()?;
        if !matches!(status.state.as_str(), "enabled" | "blocked") {
            return Err(format!("登录登记未得到有效系统事实：{status:?}"));
        }
        Ok(status)
    })();
    let cleanup = native::set(false).and_then(|_| {
        let status = read()?;
        if status.state != "disabled" {
            return Err(format!("取消登录项未生效：{status:?}"));
        }
        Ok(())
    });
    let registered = match (registered, cleanup) {
        (Ok(status), Ok(())) => status,
        (Err(e), Ok(())) => return Err(e),
        (Ok(_), Err(e)) => return Err(e),
        (Err(e), Err(cleanup)) => return Err(format!("{e}；登录项清理失败：{cleanup}")),
    };
    std::fs::write(directory.join("login-registration.json"),serde_json::json!({"before":format!("{before:?}"),"registered":registered,"removed":true,"bundle":actual}).to_string()).map_err(|e|e.to_string())
}
