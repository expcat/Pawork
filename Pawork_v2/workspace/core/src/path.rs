//! S2 临时相对路径校验入口。
//!
//! 只做词法规则：空路径、绝对路径、`..` 越 root、Windows 保留设备名。
//! symlink 逃逸、`.git` 段拒绝、FIFO/TOCTOU 留给 S3 替换实现；本模块对外签名保持不变。

use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf};

/// 解析后的工作区相对路径。
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ResolvedPath {
    pub absolute: PathBuf,
    pub root: PathBuf,
    /// 相对命中 root，规范化，不含 `.` / `..`。
    pub relative: String,
}

/// 相对路径校验错误。
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum WorkspacePathError {
    #[error("relative path is empty")]
    Empty,
    #[error("absolute paths are not allowed")]
    AbsolutePath,
    #[error("path traversal escapes workspace: {0}")]
    Traversal(String),
    #[error("reserved Windows device name: {0}")]
    ReservedDeviceName(String),
    #[error("no workspace root matched")]
    NoRoot,
}

const RESERVED_DEVICE_NAMES: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// 将 `relative` 解析到 `roots` 中按登记顺序命中的第一个 root。
///
/// `absolute = root.join(normalized)`。S2 不做存在性、symlink 或 `.git` 检查。
pub fn resolve_relative_path(
    roots: &[PathBuf],
    relative: &str,
) -> Result<ResolvedPath, WorkspacePathError> {
    if relative.is_empty() {
        return Err(WorkspacePathError::Empty);
    }
    if is_absolute_input(relative) {
        return Err(WorkspacePathError::AbsolutePath);
    }

    let normalized = normalize_components(Path::new(relative))?;
    for component in &normalized {
        if let Some(name) = reserved_device_name(component) {
            return Err(WorkspacePathError::ReservedDeviceName(name));
        }
    }

    if roots.is_empty() {
        return Err(WorkspacePathError::NoRoot);
    }

    let relative = normalized
        .iter()
        .map(|component| component.to_string_lossy())
        .collect::<Vec<_>>()
        .join("/");

    let join = |root: &PathBuf| {
        let mut absolute = root.clone();
        for component in &normalized {
            absolute.push(component);
        }
        ResolvedPath {
            absolute,
            root: root.clone(),
            relative: relative.clone(),
        }
    };

    // 按登记顺序：已存在的路径优先命中；都不存在时落到第一个 root（由工具层报 NotFound）。
    for root in roots {
        let resolved = join(root);
        if resolved.absolute.exists() {
            return Ok(resolved);
        }
    }
    Ok(join(&roots[0]))
}

fn is_absolute_input(relative: &str) -> bool {
    if Path::new(relative).is_absolute() {
        return true;
    }
    // 跨平台拒绝 Windows 盘符与 UNC：Unix 上 `Path::is_absolute` 不会把它们当绝对路径。
    if relative.starts_with("\\\\") || relative.starts_with("//") {
        return true;
    }
    let mut chars = relative.chars();
    matches!(
        (chars.next(), chars.next()),
        (Some(letter), Some(':')) if letter.is_ascii_alphabetic()
    )
}

fn normalize_components(path: &Path) -> Result<Vec<std::ffi::OsString>, WorkspacePathError> {
    let mut stack = Vec::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(name) => stack.push(name.to_os_string()),
            Component::ParentDir => {
                if stack.pop().is_none() {
                    return Err(WorkspacePathError::Traversal(path.display().to_string()));
                }
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(WorkspacePathError::AbsolutePath);
            }
        }
    }
    Ok(stack)
}

fn reserved_device_name(component: &OsStr) -> Option<String> {
    let raw = component.to_string_lossy();
    let stripped = raw.trim_end_matches(['.', ' ']);
    if RESERVED_DEVICE_NAMES
        .iter()
        .any(|name| stripped.eq_ignore_ascii_case(name))
    {
        Some(raw.into_owned())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_roots() -> (tempfile::TempDir, Vec<PathBuf>) {
        let dir = tempfile::tempdir().expect("temp root");
        let roots = vec![dir.path().to_path_buf()];
        (dir, roots)
    }

    #[test]
    fn rejects_empty() {
        let (_dir, roots) = temp_roots();
        assert_eq!(
            resolve_relative_path(&roots, "").unwrap_err(),
            WorkspacePathError::Empty
        );
    }

    #[test]
    fn rejects_absolute_paths() {
        let (_dir, roots) = temp_roots();
        #[cfg(windows)]
        let native_abs = r"C:\Windows\System32\cmd.exe";
        #[cfg(not(windows))]
        let native_abs = "/etc/passwd";
        assert_eq!(
            resolve_relative_path(&roots, native_abs).unwrap_err(),
            WorkspacePathError::AbsolutePath
        );
        assert_eq!(
            resolve_relative_path(&roots, r"C:\Windows\system32\cmd.exe").unwrap_err(),
            WorkspacePathError::AbsolutePath
        );
        assert_eq!(
            resolve_relative_path(&roots, r"\\server\share\file").unwrap_err(),
            WorkspacePathError::AbsolutePath
        );
    }

    #[test]
    fn rejects_traversal_escaping_root() {
        let (_dir, roots) = temp_roots();
        assert!(matches!(
            resolve_relative_path(&roots, "../secret"),
            Err(WorkspacePathError::Traversal(_))
        ));
        assert!(matches!(
            resolve_relative_path(&roots, "a/../../../b"),
            Err(WorkspacePathError::Traversal(_))
        ));
    }

    #[test]
    fn rejects_reserved_device_names() {
        let (_dir, roots) = temp_roots();
        for name in ["NUL", "CON", "COM1", "nul", "CON.", "COM1 "] {
            let err = resolve_relative_path(&roots, name).unwrap_err();
            assert!(
                matches!(err, WorkspacePathError::ReservedDeviceName(_)),
                "{name}: {err:?}"
            );
        }
        assert!(matches!(
            resolve_relative_path(&roots, "foo/NUL/bar"),
            Err(WorkspacePathError::ReservedDeviceName(_))
        ));
    }

    #[test]
    fn allows_normalized_path_still_inside_root() {
        let (_dir, roots) = temp_roots();
        let resolved = resolve_relative_path(&roots, "a/../b").expect("resolve");
        assert_eq!(resolved.relative, "b");
        assert_eq!(resolved.root, roots[0]);
        assert_eq!(resolved.absolute, roots[0].join("b"));
    }

    #[test]
    fn no_roots_yields_no_root() {
        assert_eq!(
            resolve_relative_path(&[], "file.txt").unwrap_err(),
            WorkspacePathError::NoRoot
        );
    }

    #[test]
    fn prefers_existing_path_in_later_registered_root() {
        let first = tempfile::tempdir().expect("first root");
        let second = tempfile::tempdir().expect("second root");
        std::fs::write(second.path().join("only-in-second.txt"), b"ok").expect("write");
        let roots = vec![first.path().to_path_buf(), second.path().to_path_buf()];
        let resolved =
            resolve_relative_path(&roots, "only-in-second.txt").expect("hit second root");
        assert_eq!(resolved.root, second.path());
        assert_eq!(resolved.relative, "only-in-second.txt");
        assert!(resolved.absolute.exists());
    }
}
