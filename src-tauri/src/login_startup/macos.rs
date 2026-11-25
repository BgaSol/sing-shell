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
pub(super) fn verify_registration(_: &Path) -> Result<(), String> {
    let bundle = native::bundle(&command_path()?, true)?;
    let home = std::path::PathBuf::from(std::env::var_os("HOME").ok_or("无法确定 CI 用户")?);
    if !bundle.path.starts_with(home.join("Applications"))
        || native::status()? != native::State::NotRegistered
    {
        return Err("登录登记验收要求用户 Applications 内的完整安装和干净系统状态".into());
    }
    let result = (|| {
        set(true)?;
        if !read()?.enabled {
            return Err("系统未允许本次主应用登录登记".into());
        }
        set(false)?;
        if read()?.state != "disabled" {
            return Err("取消主应用登录项未生效".into());
        }
        Ok(())
    })();
    result.and(native::set(false))
}
