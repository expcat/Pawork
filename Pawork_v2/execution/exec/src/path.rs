//! 沙箱路径判定（从 V2 policy 小复制，不依赖 `pawork-policy`）。

use std::path::{Path, PathBuf};

/// 平台一致的 canonicalize：Windows 上移除 `\\?\` verbatim 前缀，
/// Unix 上与 `std::fs::canonicalize` 等价。
pub fn canonicalize_platform(path: &Path) -> std::io::Result<PathBuf> {
    dunce::canonicalize(path)
}

/// 判断 canonical 路径是否位于 canonical root 内。
///
/// Windows 文件系统路径按大小写不敏感比较盘符与组件；Unix 保持字节级比较。
pub fn path_within_root(path: &Path, root: &Path) -> bool {
    relative_to_root(path, root).is_some()
}

#[cfg(not(windows))]
pub fn relative_to_root(path: &Path, root: &Path) -> Option<PathBuf> {
    path.strip_prefix(root).ok().map(Path::to_path_buf)
}

#[cfg(windows)]
pub fn relative_to_root(path: &Path, root: &Path) -> Option<PathBuf> {
    let mut path_components = path.components();
    for root_component in root.components() {
        let path_component = path_components.next()?;
        if !path_component
            .as_os_str()
            .to_string_lossy()
            .eq_ignore_ascii_case(&root_component.as_os_str().to_string_lossy())
        {
            return None;
        }
    }
    Some(path_components.collect())
}
