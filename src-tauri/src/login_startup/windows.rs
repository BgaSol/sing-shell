use super::{command_path, Status};
use std::{path::Path, ptr};
use windows_sys::Win32::System::Registry::*;

const RUN: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
const APPROVED: &str =
    "Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\StartupApproved\\Run";
const VALUE: &str = "com.bgasol.singshell";
fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}

use crate::platform::windows_registry::{query, string, Key};
fn command(path: &Path) -> Result<String, String> {
    let path = path.to_str().ok_or("启动路径不是有效 Unicode")?;
    if path.contains(['"', '\0', '\n', '\r']) {
        return Err("启动路径包含无法登记的字符".into());
    }
    Ok(format!("\"{path}\" --login-start"))
}

fn classify(value: &str, expected: &str, approval: Option<&[u8]>) -> Status {
    if approval.is_some_and(|bytes| matches!(bytes.first(), Some(3 | 7))) {
        return Status {
            enabled: false,
            state: "blocked".into(),
            reason: Some("已被 Windows 禁用。请在系统设置 → 应用 → 启动中允许 Sing Shell。".into()),
        };
    }
    if approval.is_some_and(|bytes| !matches!(bytes.first(), Some(2 | 6))) {
        return Status {
            enabled: false,
            state: "unknown".into(),
            reason: Some("Windows 启动批准状态无法识别，请检查系统启动设置".into()),
        };
    }
    if value != expected {
        return Status {
            enabled: true,
            state: "stale".into(),
            reason: Some("启动项指向旧程序位置。关闭后重新开启可登记当前位置。".into()),
        };
    }
    Status {
        enabled: true,
        state: "enabled".into(),
        reason: None,
    }
}

pub fn read() -> Result<Status, String> {
    let Some((kind, bytes)) = query(HKEY_CURRENT_USER, RUN, VALUE)? else {
        return Ok(Status::disabled());
    };
    let value = string(kind, &bytes)?;
    let approval = query(HKEY_CURRENT_USER, APPROVED, VALUE)?;
    Ok(classify(
        &value,
        &command(&command_path()?)?,
        approval
            .as_ref()
            .filter(|(kind, _)| *kind == REG_BINARY)
            .map(|(_, v)| v.as_slice()),
    ))
}

fn write(path: &str, name: &str, value: Option<&str>) -> Result<(), String> {
    let mut key = ptr::null_mut();
    let result = unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            wide(path).as_ptr(),
            0,
            ptr::null_mut(),
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE,
            ptr::null(),
            &mut key,
            ptr::null_mut(),
        )
    };
    if result != 0 {
        return Err(format!("无法打开 Windows 启动项：错误码 {result}"));
    }
    let key = Key(key);
    let result = if let Some(value) = value {
        let value = wide(value);
        unsafe {
            RegSetValueExW(
                key.0,
                wide(name).as_ptr(),
                0,
                REG_SZ,
                value.as_ptr().cast(),
                (value.len() * 2) as u32,
            )
        }
    } else {
        unsafe { RegDeleteValueW(key.0, wide(name).as_ptr()) }
    };
    if result != 0 && result != 2 {
        Err(format!("无法修改 Windows 启动项：错误码 {result}"))
    } else {
        Ok(())
    }
}

pub fn set(enabled: bool) -> Result<(), String> {
    let value = if enabled {
        Some(command(&command_path()?)?)
    } else {
        None
    };
    write(RUN, VALUE, value.as_deref())
}

pub fn installed(executable: &Path) -> Result<(), String> {
    crate::platform::installation::installation_scope(executable).map(|_| ())
}

pub(super) fn verify(directory: &Path) -> Result<(), String> {
    let key = format!("Software\\Sing Shell-Test\\Login-{}", std::process::id());
    let command = command(&directory.join("中文 space/sing-shell.exe"))?;
    let result = (|| {
        write(&key, VALUE, Some(&command))?;
        let (kind, bytes) = query(HKEY_CURRENT_USER, &key, VALUE)?.ok_or("测试登记缺失")?;
        if string(kind, &bytes)? != command {
            return Err("测试登记路径不一致".into());
        }
        write(&key, VALUE, None)?;
        if query(HKEY_CURRENT_USER, &key, VALUE)?.is_some() {
            return Err("测试登记未移除".into());
        }
        Ok(())
    })();
    unsafe {
        RegDeleteKeyW(HKEY_CURRENT_USER, wide(&key).as_ptr());
    }
    result
}
pub(super) fn verify_registration(_: &Path) -> Result<(), String> {
    installed(&command_path()?)?;
    if query(HKEY_CURRENT_USER, RUN, VALUE)?.is_some()
        || query(HKEY_CURRENT_USER, APPROVED, VALUE)?.is_some()
    {
        return Err("CI 登录项环境不干净".into());
    }
    let result = (|| {
        set(true)?;
        if read()?.state != "enabled" {
            return Err("正式安装登录登记未启用".into());
        }
        write(APPROVED, VALUE, None)?;
        let disabled = [3u8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        let code = unsafe {
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                wide(APPROVED).as_ptr(),
                wide(VALUE).as_ptr(),
                REG_BINARY,
                disabled.as_ptr().cast(),
                disabled.len() as u32,
            )
        };
        if code != 0 {
            return Err(format!("无法建立系统禁用测试事实：{code}"));
        }
        if read()?.state != "blocked" {
            return Err("未读出系统禁用事实".into());
        }
        set(true)?;
        if read()?.state != "blocked" {
            return Err("登记覆盖了系统禁用事实".into());
        }
        set(false)?;
        if read()?.state != "disabled" {
            return Err("关闭登录项失败".into());
        }
        Ok(())
    })();
    let cleanup = write(RUN, VALUE, None).and_then(|_| write(APPROVED, VALUE, None));
    result.and(cleanup)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn system_disabled_is_never_reported_as_enabled() {
        let value = "\"C:\\中文 space\\sing-shell.exe\" --login-start";
        assert_eq!(
            command(Path::new("C:\\中文 space\\sing-shell.exe")).unwrap(),
            value
        );
        assert_eq!(classify(value, value, None).state, "enabled");
        assert_eq!(classify("old", value, None).state, "stale");
        let blocked = classify(value, value, Some(&[3, 0, 0, 0]));
        assert!(!blocked.enabled);
        assert_eq!(blocked.state, "blocked");
    }
    #[test]
    fn registry_roundtrip_uses_a_non_startup_test_key() {
        verify(&std::env::temp_dir()).unwrap();
    }
}
