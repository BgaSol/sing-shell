use objc2::rc::{autoreleasepool, Retained};
use objc2_foundation::{NSBundle, NSString, NSURL};
use std::path::{Path, PathBuf};
#[link(name = "CoreServices", kind = "framework")]
extern "C" {
    fn LSRegisterURL(url: *const std::ffi::c_void, update: u8) -> i32;
}
pub struct Bundle {
    pub path: PathBuf,
    pub executable: PathBuf,
}

pub fn bundle(executable: &Path, installed: bool) -> Result<Bundle, String> {
    autoreleasepool(|_| {
        let main = NSBundle::mainBundle();
        let path = PathBuf::from(main.bundlePath().to_string());
        let binary = PathBuf::from(
            main.executablePath()
                .ok_or("应用 bundle 缺少可执行文件")?
                .to_string(),
        );
        if main
            .bundleIdentifier()
            .as_deref()
            .map(ToString::to_string)
            .as_deref()
            != Some("com.bgasol.singshell")
            || path.extension().is_none_or(|x| x != "app")
            || binary != path.join("Contents/MacOS/sing-shell")
            || std::fs::canonicalize(&binary).map_err(|e| e.to_string())?
                != std::fs::canonicalize(executable).map_err(|e| e.to_string())?
        {
            return Err("当前程序不是完整的 Sing Shell.app，无法修改登录项".into());
        }
        let icon = main
            .objectForInfoDictionaryKey(&NSString::from_str("CFBundleIconFile"))
            .and_then(|value| value.downcast::<NSString>().ok())
            .ok_or("应用 bundle 缺少图标声明")?
            .to_string();
        if icon != "icon.icns" || !path.join("Contents/Resources").join(&icon).is_file() {
            return Err("应用 bundle 图标资源不完整，请重新安装 Sing Shell".into());
        }
        if installed {
            let real = std::fs::canonicalize(&path).map_err(|e| e.to_string())?;
            if real.starts_with("/Volumes")
                || real.starts_with("/private/var/folders")
                || real.starts_with("/private/tmp")
                || real.starts_with(std::env::temp_dir())
                || real
                    .components()
                    .any(|x| x.as_os_str() == "AppTranslocation")
            {
                return Err(
                    "请先将 Sing Shell.app 移出磁盘映像或临时目录，再开启登录启动。".into(),
                );
            }
        }
        Ok(Bundle {
            path,
            executable: binary,
        })
    })
}

pub fn register_bundle(bundle: &Bundle) -> Result<(), String> {
    autoreleasepool(|_| {
        let path = NSString::from_str(bundle.path.to_str().ok_or("应用路径不是有效 Unicode")?);
        let url = NSURL::fileURLWithPath(&path);
        // NSURL and CFURL are toll-free bridged. Only update this application's registration.
        let result = unsafe { LSRegisterURL(Retained::as_ptr(&url).cast(), 1) };
        if result == 0 {
            Ok(())
        } else {
            Err(format!("无法登记 Sing Shell 应用身份：{result}"))
        }
    })
}
