//! 工作区文件路径安全解析。
//!
//! [`resolve_workspace_path`] 是所有文件工具解析模型传入路径的唯一入口。它只接受
//! `workspace_id + relative_path` 语义（本函数收 `roots` 切片与相对路径字符串），
//! 拒绝绝对路径、`..` 穿越、symlink 跳出 root、`.git` 内部、设备/FIFO/socket，
//! 并在解析后立即用 canonical 路径复核仍在某个 root 内（缓解 TOCTOU）。

use std::ffi::{OsStr, OsString};
use std::fs::FileType;
use std::path::{Component, Path, PathBuf};

/// 解析后的安全路径。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedPath {
    /// canonical 化后的绝对路径（已校验仍在 root 内）。
    pub absolute: PathBuf,
    /// 命中的 canonical workspace root。
    pub root: PathBuf,
    /// 相对于 root 的路径（已规范化，不含 `.`/`..`）。
    pub relative: String,
}

/// 路径安全错误。
#[derive(Debug, thiserror::Error)]
pub enum PathSafetyError {
    #[error("relative path is empty")]
    Empty,
    #[error("absolute paths are not allowed")]
    AbsolutePath,
    #[error("path traversal escapes workspace: {0}")]
    Traversal(String),
    #[error("path escapes workspace root via symlink")]
    SymlinkEscape,
    #[error("writing inside .git is forbidden")]
    GitInternals,
    #[error("non-regular file (device/fifo/socket) is forbidden")]
    NonRegular,
    #[error("no workspace root matched")]
    NoRoot,
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// 解析工作区相对路径为安全的绝对路径。
///
/// `roots` 为工作区根目录集合（来自可信的 workspace 服务）；`relative` 为模型
/// 传入的相对路径。函数保证返回的绝对路径 canonical 后仍落在某个 root 内。
pub fn resolve_workspace_path(
    roots: &[PathBuf],
    relative: &str,
) -> Result<ResolvedPath, PathSafetyError> {
    if relative.is_empty() {
        return Err(PathSafetyError::Empty);
    }
    let rel = Path::new(relative);
    if rel.is_absolute() {
        return Err(PathSafetyError::AbsolutePath);
    }
    // 任何命中 `.git` 段的路径一律拒绝。
    if rel
        .components()
        .any(|c| matches!(c, Component::Normal(name) if is_git_component(name)))
    {
        return Err(PathSafetyError::GitInternals);
    }
    // 词法规范化以检测 `..` 穿越（与具体 root 无关）。
    let normalized = normalize_components(rel)?;

    if roots.is_empty() {
        return Err(PathSafetyError::NoRoot);
    }
    let canon_roots: Vec<PathBuf> = roots
        .iter()
        .filter_map(|r| canonicalize_platform(r).ok())
        .collect();
    if canon_roots.is_empty() {
        return Err(PathSafetyError::NoRoot);
    }

    let mut last_err = PathSafetyError::NoRoot;
    for root in roots {
        match resolve_against_root(root, &normalized, &canon_roots) {
            Ok(resolved) => return Ok(resolved),
            Err(e) => last_err = e,
        }
    }
    Err(last_err)
}

fn resolve_against_root(
    root: &Path,
    normalized: &[OsString],
    canon_roots: &[PathBuf],
) -> Result<ResolvedPath, PathSafetyError> {
    let mut lexical = root.to_path_buf();
    for comp in normalized {
        lexical.push(comp);
    }

    // 取父目录链做 canonicalize：既支持尚不存在的目标文件，又能解析中间 symlink。
    let parent_dir = if normalized.is_empty() {
        root
    } else {
        lexical.parent().unwrap_or(root)
    };
    // 父目录可能尚不存在（新建嵌套路径）：向上找到最深的已存在祖先 canonicalize，
    // 校验仍在 root 内，再拼回不存在组件。不存在组件不可能是 symlink，故安全。
    let canon_parent = canonicalize_deepest_existing(parent_dir)?;
    let resolved_abs = match normalized.last() {
        Some(name) => canon_parent.join(name),
        None => canon_parent.clone(),
    };

    // 父目录 canonical 后必须仍在某个 root 内（防中间 symlink 跳出）。
    if !within_any_root(&resolved_abs, canon_roots) {
        return Err(PathSafetyError::SymlinkEscape);
    }

    // 对已存在目标再次 canonicalize，缓解 TOCTOU（解析与使用之间被替换为 symlink），
    // 并校验其为普通文件/目录（拒绝 device/fifo/socket）。
    let resolved_abs = if let Ok(canon_file) = canonicalize_platform(&resolved_abs) {
        if !within_any_root(&canon_file, canon_roots) {
            return Err(PathSafetyError::SymlinkEscape);
        }
        let meta = std::fs::symlink_metadata(&canon_file)?;
        if is_forbidden_file_type(meta.file_type()) {
            return Err(PathSafetyError::NonRegular);
        }
        canon_file
    } else {
        resolved_abs
    };

    let matched = canon_roots
        .iter()
        .find(|r| path_within_root(&resolved_abs, r))
        .cloned()
        .ok_or(PathSafetyError::SymlinkEscape)?;
    let relative = relative_to_root(&resolved_abs, &matched)
        .map(|p| p.to_string_lossy().into_owned())
        .ok_or(PathSafetyError::SymlinkEscape)?;

    Ok(ResolvedPath {
        absolute: resolved_abs,
        root: matched,
        relative,
    })
}

fn within_any_root(path: &Path, roots: &[PathBuf]) -> bool {
    roots.iter().any(|r| path_within_root(path, r))
}

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

#[cfg(not(windows))]
fn is_git_component(name: &OsStr) -> bool {
    name == OsStr::new(".git")
}

#[cfg(windows)]
fn is_git_component(name: &OsStr) -> bool {
    name.to_string_lossy().eq_ignore_ascii_case(".git")
}

/// 向上找到最深的「已存在」祖先并 canonicalize。
///
/// 用于支持尚不存在的嵌套路径（如写工具新建 `a/b/c.txt`）：从给定目录开始逐级
/// 上溯，首个能 canonicalize 的祖先即为锚点；缺失的不存在组件随后由调用方拼回，
/// 它们尚不存在、不可能是 symlink，故不影响安全判定。
fn canonicalize_deepest_existing(dir: &Path) -> Result<PathBuf, PathSafetyError> {
    match canonicalize_platform(dir) {
        Ok(canon) => Ok(canon),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            if let Some(parent) = dir.parent() {
                // 递归 canonicalize 最深已存在祖先，再把本层缺失组件拼回。
                let canon_ancestor = canonicalize_deepest_existing(parent)?;
                // parent 是 dir 的直接父级，strip_prefix 必然成功。
                let suffix = dir.strip_prefix(parent).expect("parent is a prefix of dir");
                Ok(canon_ancestor.join(suffix))
            } else {
                Err(PathSafetyError::Io(source))
            }
        }
        Err(source) => Err(PathSafetyError::Io(source)),
    }
}

fn normalize_components(path: &Path) -> Result<Vec<OsString>, PathSafetyError> {
    let mut stack = Vec::new();
    for comp in path.components() {
        match comp {
            Component::CurDir => {}
            Component::Normal(name) => stack.push(name.to_os_string()),
            Component::ParentDir => {
                if stack.pop().is_none() {
                    return Err(PathSafetyError::Traversal(path.display().to_string()));
                }
            }
            // 相对路径不应出现根锚点或 Windows 盘符前缀。
            Component::RootDir | Component::Prefix(_) => {
                return Err(PathSafetyError::Traversal(path.display().to_string()));
            }
        }
    }
    Ok(stack)
}

fn is_forbidden_file_type(ft: FileType) -> bool {
    if ft.is_file() || ft.is_dir() {
        return false;
    }
    is_special_file_type(&ft)
}

#[cfg(unix)]
fn is_special_file_type(ft: &FileType) -> bool {
    use std::os::unix::fs::FileTypeExt;
    ft.is_block_device() || ft.is_char_device() || ft.is_fifo() || ft.is_socket()
}

#[cfg(not(unix))]
fn is_special_file_type(_ft: &FileType) -> bool {
    // 非 Unix 平台无 FIFO/socket 概念；非普通文件/目录一律保守拒绝。
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new() -> Self {
            static COUNTER: AtomicU64 = AtomicU64::new(0);
            let n = COUNTER.fetch_add(1, Ordering::SeqCst);
            let ts = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            let path = std::env::temp_dir().join(format!(
                "pawork-policy-test-{}-{}-{}",
                std::process::id(),
                ts,
                n
            ));
            std::fs::create_dir_all(&path).expect("create temp dir");
            TempDir { path }
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    fn roots(tmp: &TempDir) -> Vec<PathBuf> {
        vec![tmp.path.clone()]
    }

    #[test]
    fn rejects_empty() {
        let tmp = TempDir::new();
        let err = resolve_workspace_path(&roots(&tmp), "").unwrap_err();
        assert!(matches!(err, PathSafetyError::Empty), "{err:?}");
    }

    #[test]
    fn rejects_absolute() {
        let tmp = TempDir::new();
        // 平台各自构造一个真正的绝对路径。
        #[cfg(not(windows))]
        let abs = "/etc/passwd";
        #[cfg(windows)]
        let abs = r"C:\Windows\System32\cmd.exe";
        let err = resolve_workspace_path(&roots(&tmp), abs).unwrap_err();
        assert!(matches!(err, PathSafetyError::AbsolutePath), "{err:?}");
    }

    #[test]
    fn rejects_traversal_parent() {
        let tmp = TempDir::new();
        let err = resolve_workspace_path(&roots(&tmp), "../secret").unwrap_err();
        assert!(matches!(err, PathSafetyError::Traversal(_)), "{err:?}");
    }

    #[test]
    fn rejects_deep_traversal() {
        let tmp = TempDir::new();
        let err = resolve_workspace_path(&roots(&tmp), "a/../../../b").unwrap_err();
        assert!(matches!(err, PathSafetyError::Traversal(_)), "{err:?}");
    }

    #[test]
    fn allows_normalized_traversal() {
        let tmp = TempDir::new();
        std::fs::write(tmp.path.join("b.txt"), b"x").expect("write");
        let r = resolve_workspace_path(&roots(&tmp), "a/../b.txt").expect("resolve");
        assert!(r.absolute.ends_with("b.txt"));
        assert_eq!(r.relative, "b.txt");
    }

    #[test]
    fn rejects_git_internals() {
        let tmp = TempDir::new();
        for p in [".git/config", "sub/.git/refs", ".git"] {
            let err = resolve_workspace_path(&roots(&tmp), p).unwrap_err();
            assert!(matches!(err, PathSafetyError::GitInternals), "{p}: {err:?}");
        }
    }

    #[test]
    fn allows_dotfiles_outside_git() {
        let tmp = TempDir::new();
        std::fs::write(tmp.path.join(".gitignore"), b"target").expect("write");
        let r = resolve_workspace_path(&roots(&tmp), ".gitignore").expect("resolve");
        assert!(r.absolute.ends_with(".gitignore"));
    }

    #[test]
    fn resolves_existing_file() {
        let tmp = TempDir::new();
        std::fs::write(tmp.path.join("file.txt"), b"hello").expect("write");
        let r = resolve_workspace_path(&roots(&tmp), "file.txt").expect("resolve");
        let canon_root = canonicalize_platform(&tmp.path).expect("canonicalize");
        assert_eq!(r.absolute, canon_root.join("file.txt"));
        assert_eq!(r.root, canon_root);
        assert_eq!(r.relative, "file.txt");
    }

    #[test]
    fn resolves_new_file() {
        let tmp = TempDir::new();
        let r = resolve_workspace_path(&roots(&tmp), "new.txt").expect("resolve");
        assert!(r.absolute.ends_with("new.txt"));
        assert_eq!(r.relative, "new.txt");
    }

    #[test]
    fn resolves_nested_dirs() {
        let tmp = TempDir::new();
        std::fs::create_dir_all(tmp.path.join("a/b")).expect("mkdir");
        std::fs::write(tmp.path.join("a/b/c.txt"), b"").expect("write");
        let r = resolve_workspace_path(&roots(&tmp), "a/b/c.txt").expect("resolve");
        assert!(r.absolute.ends_with("a/b/c.txt"));
    }

    #[test]
    fn resolves_deeply_nested_nonexistent_path() {
        // 新建嵌套路径：中间目录尚不存在时，应解析到最深已存在祖先并拼回。
        let tmp = TempDir::new();
        let r = resolve_workspace_path(&roots(&tmp), "x/y/z/new.txt").expect("resolve");
        assert!(r.absolute.ends_with("x/y/z/new.txt"));
        let canon_root = canonicalize_platform(&tmp.path).expect("canon root");
        assert!(r.absolute.starts_with(&canon_root));
        // 越界穿越仍被拒。
        let err = resolve_workspace_path(&roots(&tmp), "../../escape.txt").unwrap_err();
        assert!(matches!(err, PathSafetyError::Traversal(_)));
    }

    #[test]
    fn no_roots_yields_no_root() {
        let err = resolve_workspace_path(&[], "file.txt").unwrap_err();
        assert!(matches!(err, PathSafetyError::NoRoot), "{err:?}");
    }

    #[test]
    fn root_membership_respects_component_boundaries() {
        #[cfg(not(windows))]
        {
            assert!(path_within_root(
                Path::new("/work/root/a"),
                Path::new("/work/root")
            ));
            assert!(!path_within_root(
                Path::new("/work/root-other/a"),
                Path::new("/work/root")
            ));
        }
        #[cfg(windows)]
        {
            assert!(path_within_root(
                Path::new(r"C:\Work\Root\a"),
                Path::new(r"c:\work\root")
            ));
            assert!(!path_within_root(
                Path::new(r"C:\Work\Root-other\a"),
                Path::new(r"c:\work\root")
            ));
        }
    }

    #[cfg(windows)]
    #[test]
    fn windows_normalizes_case_separators_and_verbatim_prefix() {
        let tmp = TempDir::new();
        std::fs::create_dir_all(tmp.path.join("Folder")).expect("mkdir");
        std::fs::write(tmp.path.join("Folder/File.txt"), b"x").expect("write");

        let upper_root = PathBuf::from(tmp.path.to_string_lossy().to_uppercase());
        let resolved = resolve_workspace_path(&[upper_root], r"folder\FILE.txt")
            .expect("case-insensitive resolve");
        assert_eq!(resolved.relative.replace('\\', "/"), "Folder/File.txt");
        assert!(
            !resolved.absolute.to_string_lossy().starts_with(r"\\?\"),
            "dunce should remove the verbatim prefix: {:?}",
            resolved.absolute
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_rejects_case_variant_git_directory() {
        let tmp = TempDir::new();
        let err = resolve_workspace_path(&roots(&tmp), r"sub\.GIT\config").unwrap_err();
        assert!(matches!(err, PathSafetyError::GitInternals), "{err:?}");
    }

    #[cfg(windows)]
    #[test]
    fn rejects_windows_junction_escape() {
        let tmp = TempDir::new();
        let outside = TempDir::new();
        std::fs::write(outside.path.join("secret"), b"top").expect("write");
        let junction = tmp.path.join("escape-junction");
        let status = std::process::Command::new("cmd")
            .args(["/d", "/c", "mklink", "/J"])
            .arg(&junction)
            .arg(&outside.path)
            .status()
            .expect("mklink /J");
        assert!(status.success(), "mklink /J is required for this test");

        let err = resolve_workspace_path(&roots(&tmp), "escape-junction/secret").unwrap_err();
        assert!(matches!(err, PathSafetyError::SymlinkEscape), "{err:?}");
        std::fs::remove_dir(&junction).expect("remove junction");
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlink_escape() {
        let tmp = TempDir::new();
        let outside = TempDir::new();
        std::fs::write(outside.path.join("secret"), b"top").expect("write");
        std::os::unix::fs::symlink(&outside.path, tmp.path.join("escape")).expect("symlink");
        let err = resolve_workspace_path(&roots(&tmp), "escape").unwrap_err();
        assert!(matches!(err, PathSafetyError::SymlinkEscape), "{err:?}");
    }

    #[cfg(unix)]
    #[test]
    fn allows_intra_root_symlink() {
        let tmp = TempDir::new();
        std::fs::write(tmp.path.join("real.txt"), b"x").expect("write");
        std::os::unix::fs::symlink("real.txt", tmp.path.join("link.txt")).expect("symlink");
        let r = resolve_workspace_path(&roots(&tmp), "link.txt").expect("resolve");
        assert!(r.absolute.ends_with("real.txt"), "{:?}", r.absolute);
    }

    #[cfg(unix)]
    #[test]
    fn rejects_fifo() {
        let tmp = TempDir::new();
        let fifo = tmp.path.join("pipe");
        let status = std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .expect("mkfifo");
        assert!(status.success(), "mkfifo required for this test");
        let err = resolve_workspace_path(&roots(&tmp), "pipe").unwrap_err();
        assert!(matches!(err, PathSafetyError::NonRegular), "{err:?}");
    }
}
