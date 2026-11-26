//! Current-user login startup registration. The operating system is the source of truth.

use serde::Serialize;
use std::sync::Mutex;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub enabled: bool,
    pub state: String,
    pub reason: Option<String>,
}

impl Status {
    fn disabled() -> Self {
        Self {
            enabled: false,
            state: "disabled".into(),
            reason: None,
        }
    }
}

pub struct Service {
    mutation: Mutex<()>,
    available: bool,
}

impl Service {
    pub fn new(isolated: bool) -> Self {
        Self {
            mutation: Mutex::new(()),
            available: !isolated && !cfg!(debug_assertions),
        }
    }

    pub fn refresh(&self) -> Status {
        let _mutation = self.mutation.lock().unwrap();
        self.read()
    }

    fn read(&self) -> Status {
        if !self.available {
            return Status {
                enabled: false,
                state: "unavailable".into(),
                reason: Some(
                    "开发和隔离验收模式不修改系统登录项；请在正式安装的应用中设置。".into(),
                ),
            };
        }
        platform::read().unwrap_or_else(|error| Status {
            enabled: false,
            state: "unknown".into(),
            reason: Some(error),
        })
    }

    pub fn set(&self, enabled: bool) -> Result<Status, String> {
        if !self.available {
            return Err("开发和隔离验收模式不能修改系统登录项".into());
        }
        let _mutation = self.mutation.lock().unwrap();
        let result = if enabled {
            platform::installed(&command_path()?).and_then(|_| platform::set(true))
        } else {
            platform::set(false)
        };
        let state = self.read();
        result.map(|_| state)
    }
}

fn command_path() -> Result<std::path::PathBuf, String> {
    #[cfg(target_os = "linux")]
    if let Some(path) = std::env::var_os("APPIMAGE") {
        return Ok(path.into());
    }
    std::env::current_exe().map_err(|error| format!("无法确定 Sing Shell 启动位置：{error}"))
}

#[cfg(target_os = "linux")]
fn escaped_argument(path: &std::path::Path) -> String {
    let value = path.to_string_lossy();
    format!(
        "\"{}\"",
        value
            .replace('\\', "\\\\\\\\")
            .replace('"', "\\\\\"")
            .replace('$', "\\\\$")
            .replace('`', "\\\\`")
            .replace('%', "%%")
    )
}

#[cfg(target_os = "windows")]
#[path = "login_startup/windows.rs"]
mod platform;

#[cfg(target_os = "linux")]
mod platform {
    use super::{command_path, escaped_argument, Status};
    use std::{fs, path::PathBuf};

    fn path() -> Result<PathBuf, String> {
        let root = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
            .ok_or("无法确定当前用户的配置目录".to_owned())?;
        Ok(root.join("autostart/com.bgasol.singshell.desktop"))
    }

    pub fn read() -> Result<Status, String> {
        let path = path()?;
        if !path.exists() {
            return Ok(Status::disabled());
        }
        if !fs::symlink_metadata(&path)
            .map_err(|e| e.to_string())?
            .is_file()
        {
            return Err("登录项不是普通文件，已保留".into());
        }
        let text = fs::read_to_string(&path)
            .map_err(|error| format!("无法读取 Linux 登录启动项：{error}"))?;
        if !text
            .lines()
            .any(|line| line == "X-SingShell-Owner=com.bgasol.singshell")
        {
            return Err("登录项不属于 Sing Shell，已保留原文件".into());
        }
        let enabled = !text.lines().any(|line| {
            matches!(
                line.trim(),
                "Hidden=true" | "X-GNOME-Autostart-enabled=false"
            )
        });
        if !enabled {
            return Ok(Status {
                enabled: false,
                state: "disabled".into(),
                reason: Some("已被系统启动项标记为禁用".into()),
            });
        }
        if !text.lines().any(|line| {
            line == format!(
                "Exec={} --login-start",
                escaped_argument(&command_path().unwrap_or_default())
            )
        }) {
            return Ok(Status {
                enabled: true,
                state: "stale".into(),
                reason: Some("启动项指向旧程序位置。关闭后重新开启可登记当前位置。".into()),
            });
        }
        Ok(Status {
            enabled: true,
            state: "enabled".into(),
            reason: None,
        })
    }

    pub fn set(enabled: bool) -> Result<(), String> {
        let path = path()?;
        if path.exists() {
            let _ = read()?;
        }
        if !enabled {
            return fs::remove_file(&path)
                .or_else(|error| {
                    if error.kind() == std::io::ErrorKind::NotFound {
                        Ok(())
                    } else {
                        Err(error)
                    }
                })
                .map_err(|error| format!("无法关闭 Linux 登录启动项：{error}"));
        }
        let executable = command_path()?;
        let parent = path.parent().ok_or("Linux 登录启动项路径无效")?;
        fs::create_dir_all(parent)
            .map_err(|error| format!("无法创建 Linux 登录启动项目录：{error}"))?;
        let content = content(&executable)?;
        let temporary = path.with_extension("desktop.tmp");
        fs::write(&temporary, content)
            .map_err(|error| format!("无法写入 Linux 登录启动项：{error}"))?;
        fs::rename(&temporary, &path).map_err(|error| format!("无法提交 Linux 登录启动项：{error}"))
    }

    fn content(executable: &std::path::Path) -> Result<String, String> {
        if executable
            .to_str()
            .is_none_or(|s| s.contains(['\n', '\r', '\0']))
        {
            return Err("启动路径包含无法登记的字符".into());
        }
        Ok(format!("[Desktop Entry]\nType=Application\nName=Sing Shell\nX-SingShell-Owner=com.bgasol.singshell\nExec={} --login-start\nX-GNOME-Autostart-enabled=true\n",escaped_argument(executable)))
    }

    pub(super) fn verify(directory: &std::path::Path) -> Result<(), String> {
        let text = content(&directory.join("中文 space $ `%/Sing Shell.AppImage"))?;
        let path = directory.join("login-test.desktop");
        fs::write(&path, &text).map_err(|e| e.to_string())?;
        if fs::read_to_string(&path).map_err(|e| e.to_string())? != text {
            return Err("登录登记文件读回不一致".into());
        }
        fs::remove_file(path).map_err(|e| e.to_string())
    }

    pub(super) fn verify_registration(directory: &std::path::Path) -> Result<(), String> {
        let root = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .ok_or("CI 必须隔离 XDG_CONFIG_HOME")?;
        if !root.starts_with(directory) {
            return Err("登录登记测试目录未隔离".into());
        }
        if path()?.exists() {
            return Err("CI 登录项环境不干净".into());
        }
        installed(&command_path()?)?;
        let result = (|| {
            set(true)?;
            if !read()?.enabled {
                return Err("XDG 登录项未启用".into());
            }
            let file = path()?;
            let text = fs::read_to_string(&file).map_err(|e| e.to_string())?;
            fs::write(&file, format!("{text}Hidden=true\n")).map_err(|e| e.to_string())?;
            if read()?.enabled {
                return Err("XDG 系统禁用未生效".into());
            }
            set(false)?;
            if path()?.exists() {
                return Err("XDG 登录项未移除".into());
            }
            Ok(())
        })();
        result.and(set(false))
    }
    pub fn installed(executable: &std::path::Path) -> Result<(), String> {
        if std::env::var_os("APPIMAGE").is_some()
            && executable.is_file()
            && !executable.starts_with(std::env::temp_dir())
        {
            return Ok(());
        }
        if executable == std::path::Path::new("/usr/bin/sing-shell")
            || executable == std::path::Path::new("/usr/local/bin/sing-shell")
        {
            return Ok(());
        }
        Err("请先安装 DEB，或把 AppImage 放在固定目录，再开启登录启动。".into())
    }
}

#[cfg(target_os = "macos")]
#[path = "login_startup/macos.rs"]
mod platform;

/// Exercises real platform IO in a disposable location, never a system login entry.
pub fn verify_native(directory: &std::path::Path) -> Result<(), String> {
    platform::verify(directory)?;
    if std::env::var("SING_SHELL_VERIFY_REGISTRATION").as_deref() == Ok("1") {
        if std::env::var("GITHUB_ACTIONS").as_deref() != Ok("true") {
            return Err("真实登录登记验收只允许干净 CI 用户".into());
        }
        platform::verify_registration(directory)?;
    }
    Ok(())
}

/// One-shot native event, read on the UI thread during application startup.
pub fn native_login_launch() -> bool {
    #[cfg(target_os = "macos")]
    {
        platform::launched_at_login()
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

#[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
mod platform {
    use super::Status;
    pub fn read() -> Result<Status, String> {
        Ok(Status {
            enabled: false,
            state: "unsupported".into(),
            reason: Some("当前平台不支持登录后自动启动".into()),
        })
    }
    pub fn set(_: bool) -> Result<(), String> {
        Err("当前平台不支持登录后自动启动".into())
    }
}

#[cfg(test)]
mod tests {
    use super::Service;
    #[test]
    fn isolated_app_never_registers_the_real_user_login_item() {
        let service = Service::new(true);
        assert_eq!(service.refresh().state, "unavailable");
        assert!(service.set(true).is_err());
        assert!(service.set(false).is_err());
    }
}
