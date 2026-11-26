#[cfg(windows)]
use std::path::Path;
#[cfg(windows)]
use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};

#[cfg(windows)]
pub(crate) fn installation_scope(executable: &Path) -> Result<&'static str, String> {
    installation_scope_at(
        executable,
        "Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\Sing Shell",
    )
}

#[cfg(windows)]
fn installation_scope_at(executable: &Path, key: &str) -> Result<&'static str, String> {
    let current = executable
        .parent()
        .and_then(|p| std::fs::canonicalize(p).ok())
        .ok_or("无法确认正式安装位置")?;
    for (hive, scope) in [
        (HKEY_CURRENT_USER, "/CurrentUser"),
        (HKEY_LOCAL_MACHINE, "/AllUsers"),
    ] {
        if let Some((kind, bytes)) = super::windows_registry::query(hive, key, "InstallLocation")? {
            let value = super::windows_registry::string(kind, &bytes)?;
            if std::fs::canonicalize(value.trim_matches('"')).is_ok_and(|p| p == current) {
                return Ok(scope);
            }
        }
    }
    Err("未找到与当前程序位置匹配的 Sing Shell 安装登记，请使用正式安装包重新安装。".into())
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    #[test]
    fn update_scope_requires_the_registered_executable_location() {
        let key = format!("Software\\Sing Shell-Test\\Scope-{}", std::process::id());
        let directory = std::fs::canonicalize(std::env::temp_dir()).unwrap();
        let executable = directory.join("sing-shell.exe");
        assert!(installation_scope_at(&executable, &key).is_err());
        test_write(
            &key,
            "InstallLocation",
            Some(&format!("\"{}\"", directory.display())),
        )
        .unwrap();
        let scope = installation_scope_at(&executable, &key);
        let other = installation_scope_at(&std::env::current_exe().unwrap(), &key);
        let key_wide: Vec<u16> = key.encode_utf16().chain(Some(0)).collect();
        unsafe {
            windows_sys::Win32::System::Registry::RegDeleteKeyW(
                HKEY_CURRENT_USER,
                key_wide.as_ptr(),
            )
        };
        assert_eq!(scope.unwrap(), "/CurrentUser");
        assert!(other.is_err());
    }
}

#[cfg(all(test, windows))]
fn test_write(key: &str, name: &str, value: Option<&str>) -> Result<(), String> {
    use windows_sys::Win32::System::Registry::*;
    let wide = |v: &str| v.encode_utf16().chain(Some(0)).collect::<Vec<_>>();
    let mut handle = std::ptr::null_mut();
    let status = unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            wide(key).as_ptr(),
            0,
            std::ptr::null_mut(),
            0,
            KEY_SET_VALUE,
            std::ptr::null(),
            &mut handle,
            std::ptr::null_mut(),
        )
    };
    if status != 0 {
        return Err(status.to_string());
    }
    let bytes = wide(value.unwrap());
    let status = unsafe {
        RegSetValueExW(
            handle,
            wide(name).as_ptr(),
            0,
            REG_SZ,
            bytes.as_ptr().cast(),
            (bytes.len() * 2) as u32,
        )
    };
    unsafe {
        RegCloseKey(handle);
    }
    if status == 0 {
        Ok(())
    } else {
        Err(status.to_string())
    }
}
