pub const INTERFACE: &str = if cfg!(target_os = "macos") {
    "utun223"
} else {
    "sing-shell"
};
pub const ADDRESS: [&str; 2] = ["172.19.0.1/30", "fdfe:dcba:9876::1/126"];
pub const DNS: [&str; 2] = ["172.19.0.2", "fdfe:dcba:9876::2"];

pub fn require_version(version: &str) -> Result<(), String> {
    let parsed = crate::settings::stable(version)?;
    if parsed < semver::Version::new(1, 14, 4) || parsed >= semver::Version::new(1, 15, 0) {
        return Err(
            "受控 TUN 当前需要 sing-box 1.14.4 至 1.14.x，未验证的新版本不会以管理员权限执行"
                .into(),
        );
    }
    Ok(())
}
