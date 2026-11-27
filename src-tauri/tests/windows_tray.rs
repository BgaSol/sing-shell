#![cfg(windows)]
use std::{mem::size_of, ptr};
use windows_sys::Win32::UI::{
    Shell::{
        Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY,
        NOTIFYICONDATAW,
    },
    WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, LoadIconW, RegisterClassW, IDI_APPLICATION,
        WNDCLASSW,
    },
};

#[test]
#[ignore = "requires a disposable native Windows desktop runner"]
fn notification_area_measures_native_registration_and_update_capability() {
    let class: Vec<u16> = "SingShellNativeTrayProbe"
        .encode_utf16()
        .chain(Some(0))
        .collect();
    unsafe {
        let window_class = WNDCLASSW {
            lpfnWndProc: Some(DefWindowProcW),
            lpszClassName: class.as_ptr(),
            ..std::mem::zeroed()
        };
        assert_ne!(RegisterClassW(&window_class), 0);
        let window = CreateWindowExW(
            0,
            class.as_ptr(),
            ptr::null(),
            0,
            0,
            0,
            0,
            0,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null(),
        );
        assert!(!window.is_null());
        let mut data = NOTIFYICONDATAW {
            cbSize: size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: window,
            uID: 1,
            uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP,
            uCallbackMessage: 6002,
            hIcon: LoadIconW(ptr::null_mut(), IDI_APPLICATION),
            ..Default::default()
        };
        data.szTip[..5].copy_from_slice(&[116, 101, 115, 116, 0]);
        let added = Shell_NotifyIconW(NIM_ADD, &data);
        let add_error = std::io::Error::last_os_error();
        data.uFlags = NIF_TIP;
        let updated = Shell_NotifyIconW(NIM_MODIFY, &data);
        let update_error = std::io::Error::last_os_error();
        Shell_NotifyIconW(NIM_DELETE, &data);
        DestroyWindow(window);
        eprintln!("Native tray API: architecture={}, structSize={}, added={}, addError={}, updated={}, updateError={}", std::env::consts::ARCH, size_of::<NOTIFYICONDATAW>(), added, add_error, updated, update_error);
        if added == 0 {
            assert_eq!(std::env::consts::ARCH, "aarch64");
            assert_eq!(std::env::var("GITHUB_ACTIONS").as_deref(), Ok("true"));
            assert_eq!(add_error.raw_os_error(), Some(-2147467259));
            assert_eq!(updated, 0);
            assert_eq!(update_error.raw_os_error(), Some(-2147467259));
        } else {
            assert_ne!(updated, 0, "native update failed: {update_error}");
        }
        let report = serde_json::json!({"target":sing_shell::TARGET,"available":added!=0 && updated!=0,"registrationResult":added,"registrationErrorCode":add_error.raw_os_error(),"updateResult":updated,"updateErrorCode":update_error.raw_os_error()});
        eprintln!("Native tray capability: {report}");
        if let Some(path) = std::env::var_os("SING_SHELL_NATIVE_TRAY_REPORT") {
            let path = std::path::PathBuf::from(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, report.to_string()).unwrap();
        }
    }
}
