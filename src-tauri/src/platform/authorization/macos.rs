mod legacy;
pub use legacy::prepare;

use std::{ffi::c_void, os::fd::AsRawFd, path::PathBuf};
const AUTHORIZE: &str = r#"on run argv
    set command to "exec " & quoted form of item 1 of argv & " --network-worker " & quoted form of item 2 of argv & " " & quoted form of item 3 of argv
    do shell script command with administrator privileges with prompt "Sing Shell 需要管理员权限来建立 TUN 网络；断开后释放网络并退出。"
end run"#;
pub fn launcher(executable: &std::path::Path, endpoint: &std::path::Path) -> std::process::Command {
    let mut command = std::process::Command::new("/usr/bin/osascript");
    command
        .args(["-e", AUTHORIZE])
        .arg(executable)
        .arg(endpoint)
        .arg(std::process::id().to_string());
    command
}
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
    fn authorization_quotes_arguments_without_running_shell_text_from_paths() {
        let path = "/tmp/a b'\"; $(exit 71) `exit 72`\ncontrol.sock";
        let output = std::process::Command::new("/usr/bin/osascript")
            .args(["-e", &AUTHORIZE.replace(" with administrator privileges with prompt \"Sing Shell 需要管理员权限来建立 TUN 网络；断开后释放网络并退出。\"", " without altering line endings")])
            .args(["/bin/echo", path, "123"]).output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            String::from_utf8(output.stdout).unwrap(),
            format!("--network-worker {path} 123\n\n")
        );
    }
    #[test]
    fn replaced_executable_cannot_relabel_running_processes_as_the_new_build() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        if let Some(directory) = std::env::var_os("SING_SHELL_TEST_REPLACED_IMAGE") {
            let directory = PathBuf::from(directory);
            let endpoint = directory.join("control.sock");
            if std::env::var_os("SING_SHELL_TEST_REPLACED_CLIENT").is_some() {
                runtime.block_on(async {
                    use tokio::io::{AsyncReadExt, AsyncWriteExt};
                    let mut stream = tokio::net::UnixStream::connect(endpoint).await.unwrap();
                    stream.write_u8(1).await.unwrap();
                    let mut end = [0];
                    tokio::time::timeout(std::time::Duration::from_secs(15), stream.read(&mut end))
                        .await
                        .unwrap()
                        .unwrap();
                });
                return;
            }
            runtime.block_on(async {
                use tokio::io::AsyncReadExt;
                let listener = tokio::net::UnixListener::bind(endpoint).unwrap();
                std::fs::write(directory.join("listening"), "ready").unwrap();
                let (mut stream, _) =
                    tokio::time::timeout(std::time::Duration::from_secs(15), listener.accept())
                        .await
                        .unwrap()
                        .unwrap();
                stream.read_u8().await.unwrap();
                let peer = token(&stream).unwrap();
                verify(&stream, peer[5], false).unwrap();
                std::fs::write(directory.join("verified-old"), "ready").unwrap();
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
                while !directory.join("replaced").exists() {
                    assert!(std::time::Instant::now() < deadline);
                    tokio::time::sleep(std::time::Duration::from_millis(25)).await;
                }
                assert!(
                    verify(&stream, peer[5], false).is_err(),
                    "old running images must not adopt a replacement's disk identity"
                );
            });
            return;
        }
        let directory = std::env::temp_dir().join(format!(
            "ss-image-{}-{}",
            std::process::id(),
            crate::logs::now()
        ));
        std::fs::create_dir(&directory).unwrap();
        let original = std::env::current_exe().unwrap();
        let executable = directory.join("peer");
        std::fs::copy(&original, &executable).unwrap();
        let command = || {
            let mut command = std::process::Command::new(&executable);
            command.args(["--exact", "platform::authorization::macos::tests::replaced_executable_cannot_relabel_running_processes_as_the_new_build", "--nocapture"])
                .env("SING_SHELL_TEST_REPLACED_IMAGE", &directory);
            command
        };
        let mut server = command().spawn().unwrap();
        let wait = |name: &str| {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
            while !directory.join(name).exists() {
                assert!(std::time::Instant::now() < deadline, "timeout {name}");
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
        };
        wait("listening");
        let mut client = command()
            .env("SING_SHELL_TEST_REPLACED_CLIENT", "1")
            .spawn()
            .unwrap();
        wait("verified-old");
        let replacement = directory.join("replacement");
        std::fs::copy(&original, &replacement).unwrap();
        assert!(std::process::Command::new("/usr/bin/codesign")
            .args([
                "--force",
                "--sign",
                "-",
                "--identifier",
                "com.bgasol.singshell.replacement-test"
            ])
            .arg(&replacement)
            .status()
            .unwrap()
            .success());
        std::fs::rename(replacement, executable).unwrap();
        std::fs::write(directory.join("replaced"), "ready").unwrap();
        assert!(server.wait().unwrap().success());
        assert!(client.wait().unwrap().success());
        std::fs::remove_dir_all(directory).unwrap();
    }
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
