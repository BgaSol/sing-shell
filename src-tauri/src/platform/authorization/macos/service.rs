use objc2::{
    msg_send,
    rc::{autoreleasepool, Retained},
    runtime::{AnyClass, AnyObject},
};
use objc2_foundation::{NSError, NSString};
use std::path::PathBuf;
#[link(name = "ServiceManagement", kind = "framework")]
extern "C" {}
pub const SOCKET: &str = "/var/run/com.bgasol.singshell.network.sock";
const APPROVAL: &str =
    "请在系统设置 → 通用 → 登录项与扩展中允许 Sing Shell 网络 helper，再重试连接";

pub fn open_settings() -> Result<(), String> {
    let class = AnyClass::get(c"SMAppService").ok_or("当前系统不支持此授权设置")?;
    unsafe {
        let _: () = msg_send![class, openSystemSettingsLoginItems];
    }
    Ok(())
}
pub fn already_allowed() -> bool {
    autoreleasepool(|_| {
        service().is_ok_and(|service| status(&service) == Ok(ServiceState::Enabled))
    })
}
#[derive(Clone, Copy, Debug, PartialEq)]
enum ServiceState {
    NotRegistered,
    Enabled,
    RequiresApproval,
    NotFound,
}
fn status(service: &AnyObject) -> Result<ServiceState, String> {
    let value: usize = unsafe { msg_send![service, status] };
    match value {
        0 => Ok(ServiceState::NotRegistered),
        1 => Ok(ServiceState::Enabled),
        2 => Ok(ServiceState::RequiresApproval),
        3 => Ok(ServiceState::NotFound),
        _ => Err(format!("macOS 返回未知网络授权状态：{value}")),
    }
}
fn service() -> Result<Retained<AnyObject>, String> {
    let class = AnyClass::get(c"SMAppService")
        .ok_or("TUN 需要 macOS 13 或更高版本；此系统可继续使用原始配置模式")?;
    let name = NSString::from_str("com.bgasol.singshell.network.plist");
    Ok(unsafe { msg_send![class, daemonServiceWithPlistName: &*name] })
}
fn reason(error: *mut NSError) -> String {
    unsafe { error.as_ref() }
        .map(|e| {
            format!(
                "{} / {}：{}",
                e.domain(),
                e.code(),
                e.localizedDescription()
            )
        })
        .unwrap_or_else(|| "系统未提供原因".into())
}
fn register(service: &AnyObject) -> Result<(), String> {
    let mut error: *mut NSError = std::ptr::null_mut();
    let ok: bool = unsafe { msg_send![service, registerAndReturnError: &mut error] };
    let state = status(service)?;
    if state == ServiceState::RequiresApproval {
        return Err(APPROVAL.into());
    }
    if ok && state == ServiceState::Enabled {
        Ok(())
    } else {
        Err(format!(
            "网络授权登记失败（状态 {state:?}）：{}",
            reason(error)
        ))
    }
}
pub fn ensure_registered() -> Result<(), String> {
    let executable = std::env::current_exe().map_err(|e| e.to_string())?;
    let bundle = crate::platform::macos_bundle::bundle(&executable, true)?;
    let real = std::fs::canonicalize(&bundle.path).map_err(|e| e.to_string())?;
    let user_apps = std::env::var_os("HOME").map(|p| PathBuf::from(p).join("Applications"));
    if !real.starts_with("/Applications") && !user_apps.is_some_and(|p| real.starts_with(p)) {
        return Err(
            "请将完整的 Sing Shell.app 安装到 Applications（应用程序）后再请求网络授权".into(),
        );
    }
    if !bundle
        .path
        .join("Contents/Library/LaunchDaemons/com.bgasol.singshell.network.plist")
        .is_file()
    {
        return Err("应用缺少网络授权服务，请重新安装完整的 Sing Shell.app".into());
    }
    crate::platform::macos_bundle::register_bundle(&bundle)?;
    autoreleasepool(|_| {
        let service = service()?;
        match status(&service)? {
            ServiceState::Enabled => Ok(()),
            ServiceState::RequiresApproval => Err(APPROVAL.into()),
            _ => register(&service),
        }
    })
}
pub fn activation_error(error: String) -> String {
    let detail = match super::super::launchd::inspect() {
        Ok(Some(job)) => job.diagnostic(),
        Ok(None) => "launchd 返回了无法识别的服务登记".into(),
        Err(error) => error,
    };
    format!("网络 helper 未能就绪：{error}；{detail}")
}

// Never unregister a running helper: SMAppService would terminate its session.
pub fn recover_registration(cancel: &std::sync::atomic::AtomicBool) -> Result<bool, String> {
    autoreleasepool(|_| {
        let service = service()?;
        match status(&service)? {
            ServiceState::RequiresApproval => return Err(APPROVAL.into()),
            ServiceState::Enabled => {}
            _ => return Ok(false),
        }
        let Some(job) = super::super::launchd::inspect()? else {
            return Ok(false);
        };
        if !job.failed_inactive_registration() {
            return Ok(false);
        }
        if cancel.load(std::sync::atomic::Ordering::Acquire) {
            return Err("网络授权已取消".into());
        }
        let mut error: *mut NSError = std::ptr::null_mut();
        let ok: bool = unsafe { msg_send![&*service, unregisterAndReturnError: &mut error] };
        if !ok {
            return Err(format!("无法撤销失效的网络服务登记：{}", reason(error)));
        }
        register(&service).map_err(|error| format!("旧网络服务登记已撤销；{error}"))?;
        Ok(true)
    })
}
