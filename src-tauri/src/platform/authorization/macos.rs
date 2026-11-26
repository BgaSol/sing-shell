use objc2::{
    msg_send,
    rc::{autoreleasepool, Retained},
    runtime::{AnyClass, AnyObject},
};
use objc2_foundation::{NSError, NSString};
#[link(name = "ServiceManagement", kind = "framework")]
extern "C" {}
pub const SOCKET: &str = "/var/run/com.bgasol.singshell.network.sock";
pub fn open_settings() -> Result<(), String> {
    let class = AnyClass::get(c"SMAppService").ok_or("当前系统不支持此授权设置")?;
    unsafe {
        let _: () = msg_send![class, openSystemSettingsLoginItems];
    }
    Ok(())
}
pub fn already_allowed() -> bool {
    autoreleasepool(|_| {
        let Some(class) = AnyClass::get(c"SMAppService") else {
            return false;
        };
        let name = NSString::from_str("com.bgasol.singshell.network.plist");
        let service: Retained<AnyObject> =
            unsafe { msg_send![class,daemonServiceWithPlistName:&*name] };
        let status: usize = unsafe { msg_send![&*service, status] };
        status == 1
    })
}
pub fn ensure_registered() -> Result<(), String> {
    let executable = std::env::current_exe().map_err(|e| e.to_string())?;
    crate::platform::macos_bundle::bundle(&executable, true)?;
    let bundle = executable
        .parent()
        .and_then(std::path::Path::parent)
        .and_then(std::path::Path::parent)
        .ok_or("应用 bundle 路径无效")?;
    if !bundle
        .join("Contents/Library/LaunchDaemons/com.bgasol.singshell.network.plist")
        .is_file()
    {
        return Err("应用缺少网络授权服务，请重新安装完整的 Sing Shell.app".into());
    }
    autoreleasepool(|_| {
        let class = AnyClass::get(c"SMAppService")
            .ok_or("TUN 需要 macOS 13 或更高版本；此系统可继续使用原始配置模式")?;
        let name = NSString::from_str("com.bgasol.singshell.network.plist");
        let service: Retained<AnyObject> =
            unsafe { msg_send![class,daemonServiceWithPlistName:&*name] };
        let status: usize = unsafe { msg_send![&*service, status] };
        if status == 1 {
            return Ok(());
        }
        if status == 2 {
            return Err(
                "请在系统设置 → 通用 → 登录项与扩展中允许 Sing Shell 网络 helper，再重试连接"
                    .into(),
            );
        }
        let mut error: *mut NSError = std::ptr::null_mut();
        let ok: bool = unsafe { msg_send![&*service,registerAndReturnError:&mut error] };
        let status: usize = unsafe { msg_send![&*service, status] };
        if status == 2 {
            return Err(
                "请在系统设置 → 通用 → 登录项与扩展中允许 Sing Shell 网络 helper，再重试连接"
                    .into(),
            );
        }
        if ok && status == 1 {
            Ok(())
        } else {
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
            Err(format!("网络授权登记失败（状态 {status}）：{reason}"))
        }
    })
}

use std::{ffi::c_void, os::fd::AsRawFd, path::PathBuf};
type CF = *const c_void;
#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFDataCreate(allocator: CF, bytes: *const u8, size: isize) -> CF;
    fn CFDataGetLength(data: CF) -> isize;
    fn CFDataGetBytePtr(data: CF) -> *const u8;
    fn CFDictionaryCreate(
        allocator: CF,
        keys: *const CF,
        values: *const CF,
        count: isize,
        keys_cb: CF,
        values_cb: CF,
    ) -> CF;
    fn CFDictionaryGetValue(dictionary: CF, key: CF) -> CF;
    fn CFRelease(object: CF);
    static kCFTypeDictionaryKeyCallBacks: c_void;
    static kCFTypeDictionaryValueCallBacks: c_void;
}
#[link(name = "Security", kind = "framework")]
extern "C" {
    fn SecCodeCopySelf(flags: u32, code: *mut CF) -> i32;
    fn SecCodeCopyGuestWithAttributes(host: CF, attrs: CF, flags: u32, code: *mut CF) -> i32;
    fn SecCodeCheckValidity(code: CF, flags: u32, requirement: CF) -> i32;
    fn SecCodeCopySigningInformation(code: CF, flags: u32, information: *mut CF) -> i32;
    static kSecGuestAttributeAudit: CF;
    static kSecCodeInfoUnique: CF;
}
extern "C" {
    fn csops(pid: i32, operation: u32, address: *mut c_void, size: usize) -> i32;
    fn csops_audittoken(
        pid: i32,
        operation: u32,
        address: *mut c_void,
        size: usize,
        token: *const u32,
    ) -> i32;
    fn proc_pidpath(pid: i32, buffer: *mut c_void, size: u32) -> i32;
}
struct Owned(CF);
impl Drop for Owned {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { CFRelease(self.0) };
        }
    }
}
fn checked(status: i32, stage: &str) -> Result<(), String> {
    if status == 0 {
        Ok(())
    } else {
        Err(format!(
            "网络程序身份校验失败（{stage}：{status}），请退出旧实例并从当前安装重新打开"
        ))
    }
}
fn path(pid: i32) -> Result<PathBuf, String> {
    use std::os::unix::ffi::OsStrExt;
    let mut bytes = [0u8; 4096];
    if unsafe { proc_pidpath(pid, bytes.as_mut_ptr().cast(), bytes.len() as u32) } <= 0 {
        return Err("无法读取网络对端程序路径".into());
    }
    let value = std::ffi::CStr::from_bytes_until_nul(&bytes).map_err(|e| e.to_string())?;
    std::fs::canonicalize(std::ffi::OsStr::from_bytes(value.to_bytes())).map_err(|e| e.to_string())
}
fn signed_hash(code: CF) -> Result<Vec<u8>, String> {
    unsafe {
        checked(SecCodeCheckValidity(code, 0, std::ptr::null()), "运行代码")?;
        let mut info = std::ptr::null();
        checked(
            SecCodeCopySigningInformation(code, 2, &mut info),
            "签名信息",
        )?;
        let info = Owned(info);
        let value = CFDictionaryGetValue(info.0, kSecCodeInfoUnique);
        if value.is_null() || CFDataGetLength(value) != 20 {
            return Err("网络程序缺少有效构建签名".into());
        }
        Ok(std::slice::from_raw_parts(CFDataGetBytePtr(value), 20).to_vec())
    }
}
pub fn token(stream: &tokio::net::UnixStream) -> Result<[u32; 8], String> {
    let mut token = [0u32; 8];
    let mut size = std::mem::size_of_val(&token) as libc::socklen_t;
    if unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_LOCAL,
            libc::LOCAL_PEERTOKEN,
            token.as_mut_ptr().cast(),
            &mut size,
        )
    } != 0
        || size as usize != std::mem::size_of_val(&token)
    {
        return Err("无法取得网络对端审计身份".into());
    }
    Ok(token)
}
pub fn exited(token: &[u32; 8]) -> Result<bool, String> {
    let mut hash = [0u8; 20];
    if unsafe {
        csops_audittoken(
            token[5] as i32,
            5,
            hash.as_mut_ptr().cast(),
            20,
            token.as_ptr(),
        )
    } == 0
    {
        return Ok(false);
    }
    let error = std::io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::ESRCH) {
        Ok(true)
    } else {
        Err(format!("无法确认网络 helper 退出：{error}"))
    }
}
pub fn verify(stream: &tokio::net::UnixStream, pid: u32, root: bool) -> Result<(), String> {
    let token = token(stream)?;
    if token[5] != pid || root && token[1] != 0 {
        return Err("网络对端运行身份已改变".into());
    }
    let own_pid = std::process::id() as i32;
    if path(pid as i32)? != path(own_pid)? {
        return Err("网络 helper 来自另一安装位置，请退出其他副本并从当前安装重试".into());
    }
    unsafe {
        let audit = Owned(CFDataCreate(
            std::ptr::null(),
            token.as_ptr().cast(),
            std::mem::size_of_val(&token) as isize,
        ));
        let key = kSecGuestAttributeAudit;
        let attrs = Owned(CFDictionaryCreate(
            std::ptr::null(),
            &key,
            &audit.0,
            1,
            &kCFTypeDictionaryKeyCallBacks,
            &kCFTypeDictionaryValueCallBacks,
        ));
        let mut peer = std::ptr::null();
        checked(
            SecCodeCopyGuestWithAttributes(std::ptr::null(), attrs.0, 0, &mut peer),
            "对端审计令牌",
        )?;
        let peer = Owned(peer);
        let mut own = std::ptr::null();
        checked(SecCodeCopySelf(0, &mut own), "当前程序")?;
        let own = Owned(own);
        let mut peer_running = [0u8; 20];
        let mut own_running = [0u8; 20];
        if csops_audittoken(
            pid as i32,
            5,
            peer_running.as_mut_ptr().cast(),
            20,
            token.as_ptr(),
        ) != 0
            || csops(own_pid, 5, own_running.as_mut_ptr().cast(), 20) != 0
        {
            return Err(format!(
                "无法核对网络程序运行映像：{}",
                std::io::Error::last_os_error()
            ));
        }
        if peer_running != own_running
            || signed_hash(peer.0)? != peer_running
            || signed_hash(own.0)? != own_running
        {
            return Err("网络控制端与已授权程序构建不符，请退出旧实例后重新打开当前应用".into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::fd::FromRawFd;
    #[test]
    fn inherited_listener_authenticates_accepting_process_after_ready() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        if let Ok(fd) = std::env::var("SING_SHELL_TEST_INHERITED_LISTENER") {
            let listener =
                unsafe { std::os::unix::net::UnixListener::from_raw_fd(fd.parse().unwrap()) };
            std::thread::sleep(std::time::Duration::from_millis(200));
            listener.set_nonblocking(true).unwrap();
            runtime.block_on(async {
                let listener = tokio::net::UnixListener::from_std(listener).unwrap();
                let (mut stream, _) = listener.accept().await.unwrap();
                let peer = token(&stream).unwrap();
                verify(&stream, peer[5], false).unwrap();
                super::super::handshake::server(&mut stream).await.unwrap();
            });
            return;
        }
        let endpoint = std::env::temp_dir().join(format!(
            "ss-peer-{}-{}.sock",
            std::process::id(),
            crate::logs::now()
        ));
        let listener = std::os::unix::net::UnixListener::bind(&endpoint).unwrap();
        let fd = listener.as_raw_fd();
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
        assert_eq!(
            unsafe { libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) },
            0
        );
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "platform::authorization::macos::tests::inherited_listener_authenticates_accepting_process_after_ready", "--nocapture"])
            .env("SING_SHELL_TEST_INHERITED_LISTENER", fd.to_string()).spawn().unwrap();
        assert_eq!(unsafe { libc::fcntl(fd, libc::F_SETFD, flags) }, 0);
        runtime.block_on(async {
            let mut stream = tokio::net::UnixStream::connect(&endpoint).await.unwrap();
            assert_eq!(token(&stream).unwrap()[5], std::process::id());
            super::super::handshake::bounded(
                &std::sync::atomic::AtomicBool::new(false),
                super::super::handshake::ACTIVATION,
                super::super::handshake::client(&mut stream, |stream| {
                    assert_eq!(token(stream)?[5], child.id());
                    verify(stream, child.id(), false)
                }),
            )
            .await
            .unwrap();
        });
        assert!(child.wait().unwrap().success());
        drop(listener);
        std::fs::remove_file(endpoint).unwrap();
    }
}
