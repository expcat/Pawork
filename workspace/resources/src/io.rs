use std::{
    fs::{self, File},
    io::Read,
    path::{Component, Path, PathBuf},
};

use pawork_workspace::resolve_relative_path;

use crate::{error::ResourceFileError, source::ResourceIssue};

/// 平台一致的 canonicalize：Windows 上去掉 `\\?\` verbatim 前缀。
pub(crate) fn canonicalize_platform(path: &Path) -> std::io::Result<PathBuf> {
    dunce::canonicalize(path)
}

/// 判断 canonical 路径是否位于 canonical root 内。
pub(crate) fn path_within_root(path: &Path, root: &Path) -> bool {
    relative_to_root(path, root).is_some()
}

#[cfg(not(windows))]
pub(crate) fn relative_to_root(path: &Path, root: &Path) -> Option<PathBuf> {
    path.strip_prefix(root).ok().map(Path::to_path_buf)
}

#[cfg(windows)]
pub(crate) fn relative_to_root(path: &Path, root: &Path) -> Option<PathBuf> {
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

/// 把工作区相对路径接到 root 上（走 `resolve_relative_path` 词法规则）。
pub(crate) fn join_under_root(root: &Path, relative: &str) -> Result<PathBuf, ResourceFileError> {
    let roots = [root.to_path_buf()];
    resolve_relative_path(&roots, relative)
        .map(|resolved| resolved.absolute)
        .map_err(|_| ResourceFileError::OutsideRoot)
}

pub(crate) fn read_utf8_bounded(
    path: &Path,
    max_file_bytes: u64,
) -> Result<String, ResourceFileError> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(ResourceFileError::NotFound)
        }
        Err(error) => return Err(ResourceFileError::Io(error)),
    };
    let metadata = match file.metadata() {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(ResourceFileError::NotFound)
        }
        Err(error) => return Err(ResourceFileError::Io(error)),
    };
    if !metadata.is_file() {
        return Err(ResourceFileError::NotRegularFile);
    }
    if metadata.len() > max_file_bytes {
        return Err(ResourceFileError::TooLarge {
            limit: max_file_bytes,
            actual: metadata.len(),
        });
    }
    let mut bytes = Vec::with_capacity(
        usize::try_from(metadata.len().min(max_file_bytes).min(64 * 1024)).unwrap_or_default(),
    );
    file.take(max_file_bytes.saturating_add(1))
        .read_to_end(&mut bytes)?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > max_file_bytes {
        return Err(ResourceFileError::TooLarge {
            limit: max_file_bytes,
            actual: u64::try_from(bytes.len()).unwrap_or(u64::MAX),
        });
    }
    String::from_utf8(bytes).map_err(|_| ResourceFileError::InvalidUtf8)
}

pub(crate) fn read_utf8_bounded_within(
    path: &Path,
    root: &Path,
    max_file_bytes: u64,
) -> Result<String, ResourceFileError> {
    let canonical = canonical_within(path, root)?;
    read_utf8_bounded(&canonical, max_file_bytes)
}

pub(crate) fn sorted_children(
    directory: &Path,
    maximum: usize,
) -> Result<Vec<PathBuf>, ResourceFileError> {
    let mut paths = match fs::read_dir(directory) {
        Ok(entries) => entries
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .collect::<Vec<_>>(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(ResourceFileError::Io(error)),
    };
    paths.sort_by_key(|path| path_key(path));
    paths.truncate(maximum);
    Ok(paths)
}

pub(crate) fn sorted_children_within(
    directory: &Path,
    root: &Path,
    maximum: usize,
) -> Result<Vec<PathBuf>, ResourceFileError> {
    let canonical = match canonical_within(directory, root) {
        Ok(path) => path,
        Err(ResourceFileError::NotFound) => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    sorted_children(&canonical, maximum)
}

fn canonical_within(path: &Path, root: &Path) -> Result<PathBuf, ResourceFileError> {
    let canonical_root = canonicalize_platform(root).map_err(ResourceFileError::Io)?;
    let canonical_path = match canonicalize_platform(path) {
        Ok(path) => path,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(ResourceFileError::NotFound)
        }
        Err(error) => return Err(ResourceFileError::Io(error)),
    };
    if !path_within_root(&canonical_path, &canonical_root) {
        return Err(ResourceFileError::OutsideRoot);
    }
    Ok(canonical_path)
}

pub(crate) fn path_key(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

pub(crate) fn workspace_relative_key(path: &Path, root: &Path) -> String {
    relative_to_root(path, root).map_or_else(
        || {
            path_key(
                path.file_name()
                    .map_or_else(|| Path::new("resource"), Path::new),
            )
        },
        |relative| path_key(&relative),
    )
}

/// 资源 id / 参数名共用校验：非空，且仅由 ASCII 字母数字与 `-`、`_`（可选 `.`）组成。
pub(crate) fn is_valid_identifier(id: &str, allow_dot: bool) -> bool {
    !id.is_empty()
        && id.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(byte, b'-' | b'_')
                || (allow_dot && byte == b'.')
        })
}

/// 资源声明的相对路径校验：拒绝空串（含纯空白）、绝对路径、`..`、Windows 盘符前缀，
/// 以及不含正常分量的路径；允许普通相对路径与 `./` 前缀。
pub(crate) fn is_safe_relative_reference(raw: &str) -> bool {
    if raw.trim().is_empty() {
        return false;
    }
    let bytes = raw.as_bytes();
    if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        return false;
    }
    let path = Path::new(raw);
    if path.is_absolute() {
        return false;
    }
    let mut has_normal = false;
    for component in path.components() {
        match component {
            Component::Normal(_) => has_normal = true,
            Component::CurDir => {}
            _ => return false,
        }
    }
    has_normal
}

/// TOML 解析的共用样板：解析失败时返回带 `issue_code` / `message` 的 [`ResourceIssue`]。
pub(crate) fn parse_toml_resource<T: serde::de::DeserializeOwned>(
    content: &str,
    issue_code: &str,
    message: &str,
) -> Result<T, ResourceIssue> {
    toml::from_str(content).map_err(|_| ResourceIssue::error(issue_code, message))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    #[test]
    fn bounded_reader_rejects_large_and_invalid_files() {
        let temp = tempfile::tempdir().expect("tempdir");
        let large = temp.path().join("large");
        fs::write(&large, b"1234").expect("write");
        assert!(matches!(
            read_utf8_bounded(&large, 3),
            Err(ResourceFileError::TooLarge { .. })
        ));
        let invalid = temp.path().join("invalid");
        fs::write(&invalid, [0xff]).expect("write");
        assert!(matches!(
            read_utf8_bounded(&invalid, 3),
            Err(ResourceFileError::InvalidUtf8)
        ));
    }

    #[cfg(unix)]
    #[test]
    fn bounded_reader_rejects_symlink_outside_root() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().expect("root");
        let outside = tempfile::tempdir().expect("outside");
        fs::write(outside.path().join("secret"), "secret").expect("outside file");
        symlink(outside.path().join("secret"), root.path().join("linked")).expect("symlink");
        assert!(matches!(
            read_utf8_bounded_within(&root.path().join("linked"), root.path(), 1024),
            Err(ResourceFileError::OutsideRoot)
        ));
    }

    #[test]
    fn identifier_allows_dot_only_when_requested() {
        for id in ["abc", "ABC123", "a-b_c"] {
            assert!(
                is_valid_identifier(id, true),
                "expected {id:?} valid with dot"
            );
            assert!(
                is_valid_identifier(id, false),
                "expected {id:?} valid without dot"
            );
        }
        for id in ["a.b", "a.b-c_d", "x.y.z"] {
            assert!(
                is_valid_identifier(id, true),
                "expected {id:?} valid with dot"
            );
        }
        assert!(!is_valid_identifier("a.b", false));
        for id in ["", "a b", "a/b", "aé"] {
            assert!(
                !is_valid_identifier(id, true),
                "expected {id:?} invalid with dot"
            );
        }
    }

    #[test]
    fn relative_reference_accepts_only_safe_normal_paths() {
        for reference in [
            "foo",
            "foo/bar",
            "./foo",
            "foo/./bar",
            "foo/bar.txt",
            "foo/.",
        ] {
            assert!(
                is_safe_relative_reference(reference),
                "expected {reference:?} safe"
            );
        }
        for reference in [
            "",
            "   ",
            ".",
            "./",
            "..",
            "foo/../bar",
            "/abs",
            "C:foo",
            "C:/foo",
        ] {
            assert!(
                !is_safe_relative_reference(reference),
                "expected {reference:?} unsafe"
            );
        }
    }

    #[test]
    fn toml_resource_parse_reports_issue_on_error() {
        let parsed: BTreeMap<String, String> =
            parse_toml_resource("key = \"value\"", "test_code", "boom").expect("valid TOML");
        assert_eq!(parsed.get("key").map(String::as_str), Some("value"));

        let issue = parse_toml_resource::<BTreeMap<String, String>>(
            "key = [unclosed",
            "test_parse",
            "bad TOML",
        )
        .expect_err("invalid TOML must fail");
        assert_eq!(issue.code, "test_parse");
        assert_eq!(issue.message, "bad TOML");
    }
}
