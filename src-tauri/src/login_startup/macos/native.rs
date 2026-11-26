use objc2::{msg_send, rc::autoreleasepool, rc::Retained, runtime::AnyClass, runtime::AnyObject};
use objc2_foundation::{NSAppleEventManager, NSError, NSString, NSURL};
use std::path::Path;

#[link(name = "ServiceManagement", kind = "framework")]
extern "C" {}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum State {
    NotRegistered,
    Enabled,
    RequiresApproval,
    NotFound,
}

fn state(value: usize) -> Result<State, String> {
    match value {
        0 => Ok(State::NotRegistered),
        1 => Ok(State::Enabled),
        2 => Ok(State::RequiresApproval),
        3 => Ok(State::NotFound),
        _ => Err(format!("macOS 返回未知登录项状态：{value}")),
    }
}

pub fn available() -> bool {
    AnyClass::get(c"SMAppService").is_some()
}

fn service() -> Result<Retained<AnyObject>, String> {
    let class = AnyClass::get(c"SMAppService").ok_or("当前系统不支持主应用登录项")?;
    // Class lookup keeps macOS 11/12 from linking a symbol introduced in macOS 13.
    let object: Retained<AnyObject> = unsafe { msg_send![class, mainAppService] };
    Ok(object)
}

pub fn status() -> Result<State, String> {
    autoreleasepool(|_| {
        let service = service()?;
        state(unsafe { msg_send![&*service, status] })
    })
}

pub fn legacy_status(path: &Path) -> Result<State, String> {
    autoreleasepool(|_| {
        let class = AnyClass::get(c"SMAppService").ok_or("当前系统不支持登录项授权查询")?;
        let path = NSString::from_str(path.to_str().ok_or("登录项路径不是有效 Unicode")?);
        let url = NSURL::fileURLWithPath(&path);
        state(unsafe { msg_send![class, statusForLegacyPlistAtURL: &*url] })
    })
}

pub fn set(enabled: bool) -> Result<(), String> {
    autoreleasepool(|_| {
        let service = service()?;
        let mut error: *mut NSError = std::ptr::null_mut();
        let success: bool = unsafe {
            if enabled {
                msg_send![&*service, registerAndReturnError: &mut error]
            } else {
                msg_send![&*service, unregisterAndReturnError: &mut error]
            }
        };
        if success {
            return Ok(());
        }
        let reason = unsafe { error.as_ref() }
            .map(|e| format!("{}（{}）", e.localizedDescription(), e.code()))
            .unwrap_or_else(|| "系统未提供具体原因".into());
        Err(format!("macOS 登录项修改未完成：{reason}"))
    })
}

pub use crate::platform::macos_bundle::{bundle, register_bundle};

/// Read synchronously during Tauri setup, inside applicationDidFinishLaunching.
pub fn launched_at_login() -> bool {
    autoreleasepool(|_| {
        NSAppleEventManager::sharedAppleEventManager()
            .currentAppleEvent()
            .is_some_and(|event| {
                let class: u32 = unsafe { msg_send![&*event, eventClass] };
                let id: u32 = unsafe { msg_send![&*event, eventID] };
                let property: Option<Retained<AnyObject>> = unsafe {
                    msg_send![&*event, paramDescriptorForKeyword: u32::from_be_bytes(*b"prdt")]
                };
                class == u32::from_be_bytes(*b"aevt")
                    && id == u32::from_be_bytes(*b"oapp")
                    && property.is_some_and(|value| {
                        let code: u32 = unsafe { msg_send![&*value, enumCodeValue] };
                        code == u32::from_be_bytes(*b"lgit")
                    })
            })
    })
}
