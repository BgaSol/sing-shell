fn main() {
    println!(
        "cargo:rustc-env=SING_SHELL_TARGET={}",
        std::env::var("TARGET").unwrap()
    );
    tauri_build::build();
    // Rust test executables need the same dialog manifest as the desktop binary.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && std::env::var_os("CARGO_FEATURE_TEST_FIXTURES").is_some()
    {
        println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
        println!("cargo:rustc-link-arg=/MANIFESTDEPENDENCY:type='win32' name='Microsoft.Windows.Common-Controls' version='6.0.0.0' processorArchitecture='*' publicKeyToken='6595b64144ccf1df' language='*'");
        println!("cargo:rustc-link-arg-bins=/MANIFEST:NO");
    }
}
