//! 工作区相对路径解析入口。
//!
//! 先做 Windows 盘符 / UNC / `//` 与保留设备名检查（policy 内核没有这两项），
//! 再委托 [`pawork_policy::resolve_workspace_path`]：symlink 逃逸、`.git` 段、
//! 非普通文件与 TOCTOU 复核。对外签名保持不变。

use std::ffi::OsStr;
use std::io;
use std::path::{Component, Path, PathBuf};

use pawork_policy::PathSafetyError;

/// 解析后的工作区相对路径。
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ResolvedPath {
    pub absolute: PathBuf,
    pub root: PathBuf,
    /// 相对命中 root，规范化，不含 `.` / `..`。
    pub relative: String,
}

/// 相对路径校验错误。
#[derive(Debug, thiserror::Error)]
pub enum WorkspacePathError {
    #[error("relative path is empty")]
    Empty,
    #[error("absolute paths are not allowed")]
    AbsolutePath,
    #[error("path traversal escapes workspace: {0}")]
    Traversal(String),
    #[error("reserved Windows device name: {0}")]
    ReservedDeviceName(String),
    #[error("path escapes workspace root via symlink")]
    SymlinkEscape,
    #[error("paths inside .git are forbidden")]
    GitInternals,
    #[error("non-regular file (device/fifo/socket) is forbidden")]
    NonRegular,
    #[error("no workspace root matched")]
    NoRoot,
    #[error(transparent)]
    Io(#[from] io::Error),
}

impl From<PathSafetyError> for WorkspacePathError {
    fn from(error: PathSafetyError) -> Self {
        match error {
            PathSafetyError::Empty => Self::Empty,
            PathSafetyError::AbsolutePath => Self::AbsolutePath,
            PathSafetyError::Traversal(path) => Self::Traversal(path),
            PathSafetyError::SymlinkEscape => Self::SymlinkEscape,
            PathSafetyError::GitInternals => Self::GitInternals,
            PathSafetyError::NonRegular => Self::NonRegular,
            PathSafetyError::NoRoot => Self::NoRoot,
            PathSafetyError::Io(error) => Self::Io(error),
        }
    }
}

const RESERVED_DEVICE_NAMES: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// 将 `relative` 解析到 `roots` 中按登记顺序命中的第一个 root。
///
/// Windows 盘符 / UNC / 保留设备名在本入口拦截；其余安全规则由 policy 内核判定。
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

    for component in Path::new(relative).components() {
        if let Component::Normal(name) = component {
            if let Some(reserved) = reserved_device_name(name) {
                return Err(WorkspacePathError::ReservedDeviceName(reserved));
            }
        }
    }

    let resolved = pawork_policy::resolve_workspace_path(roots, relative)?;
    Ok(ResolvedPath {
        absolute: resolved.absolute,
        root: resolved.root,
        relative: resolved.relative,
    })
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
        assert!(matches!(
            resolve_relative_path(&roots, "").unwrap_err(),
            WorkspacePathError::Empty
        ));
    }

    #[test]
    fn rejects_absolute_paths() {
        let (_dir, roots) = temp_roots();
        #[cfg(windows)]
        let native_abs = r"C:\Windows\System32\cmd.exe";
        #[cfg(not(windows))]
        let native_abs = "/etc/passwd";
        assert!(matches!(
            resolve_relative_path(&roots, native_abs).unwrap_err(),
            WorkspacePathError::AbsolutePath
        ));
        assert!(matches!(
            resolve_relative_path(&roots, r"C:\Windows\system32\cmd.exe").unwrap_err(),
            WorkspacePathError::AbsolutePath
        ));
        assert!(matches!(
            resolve_relative_path(&roots, r"\\server\share\file").unwrap_err(),
            WorkspacePathError::AbsolutePath
        ));
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
        let canon_root = pawork_policy::canonicalize_platform(&roots[0]).expect("canon root");
        assert_eq!(resolved.root, canon_root);
        assert_eq!(resolved.absolute, canon_root.join("b"));
    }

    #[test]
    fn no_roots_yields_no_root() {
        assert!(matches!(
            resolve_relative_path(&[], "file.txt").unwrap_err(),
            WorkspacePathError::NoRoot
        ));
    }

    #[test]
    fn first_root_wins_for_new_path_via_policy() {
        let first = tempfile::tempdir().expect("first root");
        let second = tempfile::tempdir().expect("second root");
        std::fs::write(second.path().join("only-in-second.txt"), b"ok").expect("write");
        let roots = vec![first.path().to_path_buf(), second.path().to_path_buf()];
        let resolved = resolve_relative_path(&roots, "only-in-second.txt").expect("resolve");
        let canon_first = pawork_policy::canonicalize_platform(first.path()).expect("canon first");
        assert_eq!(resolved.root, canon_first);
        assert_eq!(resolved.relative, "only-in-second.txt");
        assert!(resolved.absolute.starts_with(&canon_first));
    }

    #[test]
    fn rejects_git_internals() {
        let (_dir, roots) = temp_roots();
        for path in [".git/config", "sub/.git/refs", ".git"] {
            let err = resolve_relative_path(&roots, path).unwrap_err();
            assert!(
                matches!(err, WorkspacePathError::GitInternals),
                "{path}: {err:?}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlink_escape() {
        let tmp = tempfile::tempdir().expect("ws");
        let outside = tempfile::tempdir().expect("outside");
        std::fs::write(outside.path().join("secret"), b"top").expect("write");
        std::os::unix::fs::symlink(outside.path(), tmp.path().join("escape")).expect("symlink");
        let roots = vec![tmp.path().to_path_buf()];
        let err = resolve_relative_path(&roots, "escape").unwrap_err();
        assert!(matches!(err, WorkspacePathError::SymlinkEscape), "{err:?}");
    }
}
