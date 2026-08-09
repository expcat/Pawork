use std::path::PathBuf;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum ResourceLoadError {
    #[error("workspace service failed: {0}")]
    Workspace(#[from] workspace_service::WorkspaceError),
    #[error("workspace was not found: {0}")]
    WorkspaceNotFound(String),
    #[error("workspace root index {root_index} is out of range (root count: {root_count})")]
    RootIndexOutOfRange {
        root_index: usize,
        root_count: usize,
    },
    #[error("path must be workspace-relative and may not contain '..': {0}")]
    InvalidRelativePath(PathBuf),
    #[error("resource path resolves outside the workspace root: {0}")]
    PathEscapesWorkspace(PathBuf),
    #[error("resource watcher could not start: {0}")]
    Watcher(String),
    #[error("initial resource load failed: {0}")]
    InitialLoad(String),
}

#[derive(Debug, Error)]
pub(crate) enum ResourceFileError {
    #[error("resource does not exist")]
    NotFound,
    #[error("resource exceeds the {limit}-byte limit ({actual} bytes)")]
    TooLarge { limit: u64, actual: u64 },
    #[error("resource is not valid UTF-8")]
    InvalidUtf8,
    #[error("resource is not a regular file")]
    NotRegularFile,
    #[error("resource resolves outside its configured root")]
    OutsideRoot,
    #[error("resource I/O failed: {0}")]
    Io(#[from] std::io::Error),
}

impl ResourceFileError {
    pub(crate) const fn code(&self) -> &'static str {
        match self {
            Self::NotFound => "resource_not_found",
            Self::TooLarge { .. } => "resource_too_large",
            Self::InvalidUtf8 => "resource_invalid_utf8",
            Self::NotRegularFile => "resource_not_regular_file",
            Self::OutsideRoot => "resource_outside_root",
            Self::Io(_) => "resource_io_error",
        }
    }
}
