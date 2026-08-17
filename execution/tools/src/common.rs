//! 内置工具共享模块：输入解析、工作区路径解析、错误映射。

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use pawork_api::{ToolError, ToolErrorKind};
use pawork_domain::WorkspaceId;
use pawork_policy::{resolve_workspace_path, PathSafetyError};
use pawork_workspace::{
    resolve_relative_path, WorkspaceError, WorkspacePathError, WorkspaceService,
};
use serde_json::Value;

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// 内置工具统一错误：可转换为 [`ToolError`]。
#[derive(Debug, thiserror::Error)]
pub enum BuiltinToolError {
    #[error("missing required input field `{0}`")]
    MissingField(&'static str),
    #[error("field `{field}` has invalid type: {detail}")]
    InvalidField { field: &'static str, detail: String },
    #[error(transparent)]
    Path(#[from] WorkspacePathError),
    #[error(transparent)]
    PolicyPath(PathSafetyError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("workspace error: {0}")]
    Workspace(WorkspaceError),
    #[error("process error: {0}")]
    Process(String),
    #[error("{0}")]
    Other(String),
}

impl From<BuiltinToolError> for ToolError {
    fn from(error: BuiltinToolError) -> Self {
        let (kind, message) = match &error {
            BuiltinToolError::MissingField(field) => {
                (ToolErrorKind::InvalidInput, format!("missing `{field}`"))
            }
            BuiltinToolError::InvalidField { field, detail } => (
                ToolErrorKind::InvalidInput,
                format!("invalid `{field}`: {detail}"),
            ),
            BuiltinToolError::Path(path) => match path {
                WorkspacePathError::Empty => (ToolErrorKind::InvalidInput, path.to_string()),
                WorkspacePathError::NoRoot => (ToolErrorKind::NotFound, path.to_string()),
                WorkspacePathError::AbsolutePath
                | WorkspacePathError::Traversal(_)
                | WorkspacePathError::ReservedDeviceName(_) => {
                    (ToolErrorKind::PermissionDenied, path.to_string())
                }
            },
            BuiltinToolError::PolicyPath(path) => match path {
                PathSafetyError::Empty => (ToolErrorKind::InvalidInput, path.to_string()),
                PathSafetyError::NoRoot => (ToolErrorKind::NotFound, path.to_string()),
                PathSafetyError::AbsolutePath
                | PathSafetyError::Traversal(_)
                | PathSafetyError::SymlinkEscape
                | PathSafetyError::GitInternals
                | PathSafetyError::NonRegular => {
                    (ToolErrorKind::PermissionDenied, path.to_string())
                }
                PathSafetyError::Io(io) => {
                    if io.kind() == std::io::ErrorKind::NotFound {
                        (ToolErrorKind::NotFound, io.to_string())
                    } else {
                        (ToolErrorKind::ExecutionFailed, io.to_string())
                    }
                }
            },
            BuiltinToolError::Io(io) => {
                if io.kind() == std::io::ErrorKind::NotFound {
                    (ToolErrorKind::NotFound, io.to_string())
                } else {
                    (ToolErrorKind::ExecutionFailed, io.to_string())
                }
            }
            BuiltinToolError::Workspace(ws) => match ws {
                WorkspaceError::NotFound(_) => (ToolErrorKind::NotFound, ws.to_string()),
                _ => (ToolErrorKind::ExecutionFailed, ws.to_string()),
            },
            BuiltinToolError::Process(msg) => (ToolErrorKind::ExecutionFailed, msg.clone()),
            BuiltinToolError::Other(msg) => (ToolErrorKind::ExecutionFailed, msg.clone()),
        };
        ToolError {
            kind,
            message,
            retryable: false,
            retry_after_ms: None,
        }
    }
}

/// 取必填字符串字段。
pub fn require_str(input: &Value, key: &'static str) -> Result<String, BuiltinToolError> {
    input
        .get(key)
        .and_then(|v| v.as_str())
        .map(str::to_owned)
        .ok_or(BuiltinToolError::MissingField(key))
}

/// 取可选字符串字段。
pub fn opt_str(input: &Value, key: &str) -> Option<String> {
    input.get(key).and_then(|v| v.as_str()).map(str::to_owned)
}

/// 取可选 u64 字段。
pub fn opt_u64(input: &Value, key: &str) -> Option<u64> {
    input.get(key).and_then(|v| v.as_u64())
}

/// 取可选 bool 字段。
pub fn opt_bool(input: &Value, key: &str) -> Option<bool> {
    input.get(key).and_then(|v| v.as_bool())
}

/// 解析 workspace_id 对应的工作区根路径列表。
pub fn workspace_roots(
    service: &WorkspaceService,
    id: &WorkspaceId,
) -> Result<Vec<PathBuf>, BuiltinToolError> {
    let workspace = service
        .get(id)
        .map_err(BuiltinToolError::Workspace)?
        .ok_or_else(|| BuiltinToolError::Workspace(WorkspaceError::NotFound(id.to_string())))?;
    Ok(workspace.roots.clone())
}

/// 把工作区相对路径安全解析为绝对路径。
pub fn resolve_rel(roots: &[PathBuf], relative: &str) -> Result<PathBuf, BuiltinToolError> {
    resolve_relative_path(roots, relative)
        .map(|resolved| resolved.absolute)
        .map_err(BuiltinToolError::Path)
}

/// 写工具路径解析：走 policy 安全内核（越界 / symlink / `.git` / 非普通文件）。
pub fn resolve_write_rel(roots: &[PathBuf], relative: &str) -> Result<PathBuf, BuiltinToolError> {
    resolve_workspace_path(roots, relative)
        .map(|resolved| resolved.absolute)
        .map_err(BuiltinToolError::PolicyPath)
}

/// 内置写工具共用的同目录原子写；覆盖时保留 Unix mode。
pub fn atomic_write(path: &Path, content: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    #[cfg(unix)]
    let existing_mode = {
        use std::os::unix::fs::PermissionsExt;
        path.metadata()
            .ok()
            .map(|metadata| metadata.permissions().mode())
    };
    let temp = path.with_file_name(format!(
        ".pawork-tmp-{}-{}",
        std::process::id(),
        TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| {
        let mut file = std::fs::File::create(&temp)?;
        file.write_all(content)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result?;
    #[cfg(unix)]
    if let Some(mode) = existing_mode {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode));
    }
    Ok(())
}
