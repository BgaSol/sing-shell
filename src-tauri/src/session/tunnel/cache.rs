use std::path::{Path, PathBuf};

fn linked(metadata: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}

pub(super) fn prepare(root: &Path, owner: &str) -> Result<PathBuf, String> {
    if owner.len() != 64 || !owner.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("映射缓存用户身份无效".into());
    }
    let mut directory = root.to_path_buf();
    for component in ["state", owner, "fakeip-v1"] {
        directory.push(component);
        match std::fs::symlink_metadata(&directory) {
            Ok(metadata) if linked(&metadata) || !metadata.is_dir() => {
                return Err("映射缓存目录不能是链接或普通文件".into())
            }
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.to_string()),
        }
        crate::private_files::directory(&directory)?;
    }
    let file = directory.join("cache.db");
    match std::fs::symlink_metadata(&file) {
        Ok(meta) if linked(&meta) || !meta.file_type().is_file() => {
            return Err("映射缓存必须是受保护的普通文件".into())
        }
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            crate::private_files::write(&file, &[])?
        }
        Err(e) => return Err(e.to_string()),
    }
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn owners_are_isolated_and_existing_database_is_not_replaced() {
        let root = std::env::temp_dir().join(format!(
            "sing-shell-cache-{}-{}",
            std::process::id(),
            crate::logs::now()
        ));
        let first = prepare(&root, &"a".repeat(64)).unwrap();
        std::fs::write(&first, b"existing database").unwrap();
        assert_eq!(prepare(&root, &"a".repeat(64)).unwrap(), first);
        assert_eq!(std::fs::read(&first).unwrap(), b"existing database");
        assert_ne!(prepare(&root, &"b".repeat(64)).unwrap(), first);
        assert!(prepare(&root, "../outside").is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
}
