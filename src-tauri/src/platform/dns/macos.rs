use std::{
    ffi::c_void,
    thread,
    time::{Duration, Instant},
};
type CF = *const c_void;
#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFDataCreate(allocator: CF, bytes: *const u8, size: isize) -> CF;
    fn CFStringCreateWithCString(allocator: CF, value: *const i8, encoding: u32) -> CF;
    fn CFPropertyListCreateWithData(
        allocator: CF,
        data: CF,
        options: usize,
        format: *mut isize,
        error: *mut CF,
    ) -> CF;
    fn CFEqual(a: CF, b: CF) -> u8;
    fn CFRelease(value: CF);
}
#[link(name = "SystemConfiguration", kind = "framework")]
extern "C" {
    fn SCError() -> i32;
    fn SCDynamicStoreCreate(allocator: CF, name: CF, callback: CF, context: CF) -> CF;
    fn SCDynamicStoreAddTemporaryValue(store: CF, key: CF, value: CF) -> u8;
    fn SCDynamicStoreCopyValue(store: CF, key: CF) -> CF;
}
struct Owned(CF);
impl Drop for Owned {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { CFRelease(self.0) };
        }
    }
}
fn string(value: &str) -> Result<Owned, String> {
    let value = std::ffi::CString::new(value).map_err(|_| "系统 DNS 字符串无效")?;
    let object =
        Owned(unsafe { CFStringCreateWithCString(std::ptr::null(), value.as_ptr(), 0x08000100) });
    if object.0.is_null() {
        return Err("无法创建系统 DNS 字符串".into());
    }
    Ok(object)
}
pub struct Registration {
    session: Option<Owned>,
    key: Owned,
    value: Owned,
}
// No callbacks; the start task transfers exclusive ownership to the worker after joining.
unsafe impl Send for Registration {}

fn view() -> Result<String, String> {
    let output = crate::platform::command::output("/usr/sbin/scutil", &["--dns"])?;
    if !output.status.success() {
        return Err("无法读取 macOS 系统解析器".into());
    }
    String::from_utf8(output.stdout).map_err(|_| "系统 DNS 输出无效".into())
}
fn physical_interface() -> Result<String, String> {
    let output = crate::platform::command::output("/sbin/route", &["-n", "get", "default"])?;
    if !output.status.success() {
        return Err("无法确认物理网络接口".into());
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .find_map(|line| line.trim().strip_prefix("interface: "))
        .filter(|name| !name.starts_with("utun") && *name != "lo0")
        .map(str::to_owned)
        .ok_or("无法确认独立物理网络接口".into())
}
pub fn physical_ready() -> Result<(), String> {
    if !super::resolver_ready(&view()?, &physical_interface()?, false) {
        return Err("当前网络没有可安全使用的本地 DNS，请检查网络后重新连接".into());
    }
    Ok(())
}
fn flush() -> Result<(), String> {
    let output = crate::platform::command::output("/usr/bin/dscacheutil", &["-flushcache"])?;
    if !output.status.success() {
        return Err("无法刷新系统 DNS 缓存".into());
    }
    Ok(())
}
impl Registration {
    pub fn install() -> Result<Self, String> {
        physical_ready()?;
        let key = string(&format!(
            "State:/Network/Service/sing-shell-{}/DNS",
            crate::platform::authorization::nonce()?
        ))?;
        let label = string("Sing Shell temporary TUN DNS")?;
        let session = Owned(unsafe {
            SCDynamicStoreCreate(
                std::ptr::null(),
                label.0,
                std::ptr::null(),
                std::ptr::null(),
            )
        });
        if session.0.is_null() {
            return Err("无法创建系统 DNS 会话".into());
        }
        let xml = format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?><plist version=\"1.0\"><dict><key>ServerAddresses</key><array><string>{}</string><string>{}</string></array><key>SupplementalMatchDomains</key><array><string></string></array><key>SupplementalMatchOrders</key><array><integer>1</integer></array></dict></plist>", crate::profile::tun::DNS[0], crate::profile::tun::DNS[1]);
        let data =
            Owned(unsafe { CFDataCreate(std::ptr::null(), xml.as_ptr(), xml.len() as isize) });
        if data.0.is_null() {
            return Err("无法创建系统 DNS 配置".into());
        }
        let value = Owned(unsafe {
            CFPropertyListCreateWithData(
                std::ptr::null(),
                data.0,
                0,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        });
        if value.0.is_null()
            || unsafe { SCDynamicStoreAddTemporaryValue(session.0, key.0, value.0) } == 0
        {
            return Err("无法登记临时系统 DNS".into());
        }
        Ok(Self {
            session: Some(session),
            key,
            value,
        })
    }
    pub fn activate(&self) -> Result<(), String> {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            match self.check() {
                Ok(()) => break,
                Err(error) if Instant::now() >= deadline => return Err(error),
                Err(_) => thread::sleep(Duration::from_millis(100)),
            }
        }
        flush()?;
        Ok(())
    }
    pub fn check(&self) -> Result<(), String> {
        let store = self.session.as_ref().ok_or("临时系统 DNS 会话已关闭")?;
        let actual = Owned(unsafe { SCDynamicStoreCopyValue(store.0, self.key.0) });
        if actual.0.is_null()
            || unsafe { CFEqual(actual.0, self.value.0) } == 0
            || !super::resolver_ready(&view()?, &physical_interface()?, true)
        {
            return Err("网络或 DNS 设置已变化，无法确认系统 DNS 接管，请重新连接".into());
        }
        Ok(())
    }
    pub fn close(&mut self) -> Result<(), String> {
        self.session.take();
        let label = string("Sing Shell DNS cleanup observation")?;
        let observation = Owned(unsafe {
            SCDynamicStoreCreate(
                std::ptr::null(),
                label.0,
                std::ptr::null(),
                std::ptr::null(),
            )
        });
        if observation.0.is_null() {
            return Err("无法确认系统 DNS 已释放".into());
        }
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let actual = Owned(unsafe { SCDynamicStoreCopyValue(observation.0, self.key.0) });
            if actual.0.is_null() {
                if unsafe { SCError() } == 1004 {
                    break;
                }
                return Err("无法确认系统 DNS 已释放，请重试停止".into());
            }
            if Instant::now() >= deadline {
                return Err("本次临时 DNS 尚未释放，请重试停止".into());
            }
            thread::sleep(Duration::from_millis(50));
        }
        flush()
    }
}
