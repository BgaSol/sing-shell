use objc2::{
    msg_send,
    rc::{autoreleasepool, Retained},
    runtime::{AnyClass, AnyObject},
};
use objc2_foundation::{NSError, NSString};
#[link(name = "ServiceManagement", kind = "framework")]
extern "C" {}

pub fn prepare() -> Result<(), String> {
    let executable = std::env::current_exe().map_err(|e| e.to_string())?;
    let bundle = crate::platform::macos_bundle::bundle(&executable, true)?;
    let real = std::fs::canonicalize(&bundle.path).map_err(|e| e.to_string())?;
    let user_apps =
        std::env::var_os("HOME").map(|p| std::path::PathBuf::from(p).join("Applications"));
    if !real.starts_with("/Applications") && !user_apps.is_some_and(|p| real.starts_with(p)) {
        return Err(
            "请将完整的 Sing Shell.app 安装到 Applications（应用程序）后再请求网络授权".into(),
        );
    }
    autoreleasepool(|_| {
        let Some(class) = AnyClass::get(c"SMAppService") else {
            return Ok(());
        };
        let name = NSString::from_str("com.bgasol.singshell.network.plist");
        let service: Retained<AnyObject> =
            unsafe { msg_send![class, daemonServiceWithPlistName: &*name] };
        let status: usize = unsafe { msg_send![&*service, status] };
        if matches!(status, 0 | 3) {
            return Ok(());
        }
        if !matches!(status, 1 | 2) {
            return Err(format!("macOS 返回未知旧网络服务状态：{status}"));
        }
        if let Some(job) = super::super::launchd::inspect()? {
            if !job.inactive_owned_registration() {
                return Err(format!(
                    "旧网络 helper 仍在运行或无法核实归属，请先退出旧实例：{}",
                    job.diagnostic()
                ));
            }
        }
        // The plist is retained only to unregister the old bundled service.
        let mut error: *mut NSError = std::ptr::null_mut();
        let ok: bool = unsafe { msg_send![&*service, unregisterAndReturnError: &mut error] };
        if ok {
            return Ok(());
        }
        let reason = unsafe { error.as_ref() }
            .map(|e| {
                format!(
                    "{} / {}：{}",
                    e.domain(),
                    e.code(),
                    e.localizedDescription()
                )
            })
            .unwrap_or_else(|| "系统未提供原因".into());
        Err(format!("无法停用旧网络服务登记：{reason}"))
    })
}
