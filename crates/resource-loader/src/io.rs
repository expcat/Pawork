use std::{
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
};

use crate::error::ResourceFileError;

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
    let canonical_root = dunce::canonicalize(root).map_err(ResourceFileError::Io)?;
    let canonical_path = match dunce::canonicalize(path) {
        Ok(path) => path,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(ResourceFileError::NotFound)
        }
        Err(error) => return Err(ResourceFileError::Io(error)),
    };
    if !canonical_path.starts_with(canonical_root) {
        return Err(ResourceFileError::OutsideRoot);
    }
    Ok(canonical_path)
}

pub(crate) fn path_key(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

pub(crate) fn workspace_relative_key(path: &Path, root: &Path) -> String {
    path.strip_prefix(root).map_or_else(
        |_| {
            path_key(
                path.file_name()
                    .map_or_else(|| Path::new("resource"), Path::new),
            )
        },
        path_key,
    )
}

#[cfg(test)]
mod tests {
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
}
