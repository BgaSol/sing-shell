use std::ptr;
use windows_sys::Win32::System::Registry::*;
pub(crate) struct Key(pub(crate) HKEY);
impl Drop for Key {
    fn drop(&mut self) {
        unsafe {
            RegCloseKey(self.0);
        }
    }
}
fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}

pub(crate) fn query(hive: HKEY, path: &str, name: &str) -> Result<Option<(u32, Vec<u8>)>, String> {
    let mut key = ptr::null_mut();
    let result = unsafe { RegOpenKeyExW(hive, wide(path).as_ptr(), 0, KEY_READ, &mut key) };
    if result == 2 {
        return Ok(None);
    }
    if result != 0 {
        return Err(format!("无法读取 Windows 启动设置：错误码 {result}"));
    }
    let key = Key(key);
    let mut kind = 0;
    let mut bytes = 0;
    let result = unsafe {
        RegQueryValueExW(
            key.0,
            wide(name).as_ptr(),
            ptr::null_mut(),
            &mut kind,
            ptr::null_mut(),
            &mut bytes,
        )
    };
    if result == 2 {
        return Ok(None);
    }
    if result != 0 || bytes > 65536 {
        return Err("Windows 启动设置无法读取或超过长度上限".into());
    }
    let mut data = vec![0u8; bytes as usize];
    let result = unsafe {
        RegQueryValueExW(
            key.0,
            wide(name).as_ptr(),
            ptr::null_mut(),
            &mut kind,
            data.as_mut_ptr(),
            &mut bytes,
        )
    };
    if result != 0 {
        return Err(format!("无法确认 Windows 启动设置：错误码 {result}"));
    }
    data.truncate(bytes as usize);
    Ok(Some((kind, data)))
}

pub(crate) fn string(kind: u32, bytes: &[u8]) -> Result<String, String> {
    if kind != REG_SZ || !bytes.len().is_multiple_of(2) {
        return Err("Windows 启动项格式无法识别".into());
    }
    let value: Vec<_> = bytes
        .chunks_exact(2)
        .map(|b| u16::from_le_bytes([b[0], b[1]]))
        .take_while(|c| *c != 0)
        .collect();
    String::from_utf16(&value).map_err(|_| "Windows 启动项文字无法解析".into())
}
