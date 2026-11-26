use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::Path,
};
static PROTECTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
pub fn protect_worker_writes() {
    PROTECTED.store(true, std::sync::atomic::Ordering::Release);
}

pub fn directory(path: &Path) -> Result<(), String> {
    fs::create_dir_all(path).map_err(|e| e.to_string())?;
    if fs::symlink_metadata(path)
        .map_err(|e| e.to_string())?
        .file_type()
        .is_symlink()
    {
        return Err("私有目录不能是链接".into());
    }
    restrict(path, true)
}
pub fn write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let temporary = path.with_extension("private-tmp");
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    // A previous interrupted write belongs to the same single-writer application.
    if temporary.exists() {
        if fs::symlink_metadata(&temporary)
            .map_err(|e| e.to_string())?
            .file_type()
            .is_symlink()
        {
            return Err("临时文件不能是链接".into());
        }
        fs::remove_file(&temporary).map_err(|e| e.to_string())?;
    }
    let mut file = options.open(&temporary).map_err(|e| e.to_string())?;
    let result = (|| {
        restrict(&temporary, false)?;
        file.write_all(bytes)
            .and_then(|_| file.sync_all())
            .map_err(|e| e.to_string())?;
        drop(file);
        fs::rename(&temporary, path).map_err(|e| e.to_string())?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}
#[cfg(unix)]
fn restrict(path: &Path, directory: bool) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(
        path,
        fs::Permissions::from_mode(if directory { 0o700 } else { 0o600 }),
    )
    .map_err(|e| e.to_string())
}
#[cfg(windows)]
fn restrict(path: &Path, directory: bool) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::{
        Foundation::LocalFree,
        Security::{
            Authorization::{
                ConvertStringSecurityDescriptorToSecurityDescriptorW, SetNamedSecurityInfoW,
                SE_FILE_OBJECT,
            },
            GetSecurityDescriptorDacl, DACL_SECURITY_INFORMATION,
            PROTECTED_DACL_SECURITY_INFORMATION,
        },
    };
    let inheritance = if directory { "OICI" } else { "" };
    let owner = if PROTECTED.load(std::sync::atomic::Ordering::Acquire) {
        "BA".into()
    } else {
        crate::instance::current_user_sid()?
    };
    let descriptor: Vec<u16> =
        format!("D:P(A;{inheritance};FA;;;SY)(A;{inheritance};FA;;;{owner})")
            .encode_utf16()
            .chain(Some(0))
            .collect();
    let path: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    unsafe {
        let mut security = std::ptr::null_mut();
        if ConvertStringSecurityDescriptorToSecurityDescriptorW(
            descriptor.as_ptr(),
            1,
            &mut security,
            std::ptr::null_mut(),
        ) == 0
        {
            return Err(std::io::Error::last_os_error().to_string());
        }
        let mut present = 0;
        let mut defaulted = 0;
        let mut dacl = std::ptr::null_mut();
        if GetSecurityDescriptorDacl(security, &mut present, &mut dacl, &mut defaulted) == 0 {
            LocalFree(security);
            return Err(std::io::Error::last_os_error().to_string());
        }
        let code = SetNamedSecurityInfoW(
            path.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            dacl,
            std::ptr::null_mut(),
        );
        LocalFree(security);
        if code != 0 {
            Err(std::io::Error::from_raw_os_error(code as i32).to_string())
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn owner_can_read_replace_and_delete() {
        let root = std::env::temp_dir().join(format!("sing-shell-private-{}", crate::logs::now()));
        directory(&root).unwrap();
        let file = root.join("secret.json");
        write(&file, b"secret").unwrap();
        write(&file, b"new").unwrap();
        assert_eq!(fs::read(&file).unwrap(), b"new");
        fs::remove_file(file).unwrap();
        fs::remove_dir(root).unwrap();
    }
}
