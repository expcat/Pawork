//! 内置工具共享模块：输入解析、工作区路径解析、错误映射。

use std::path::PathBuf;

use pawork_api::{ToolError, ToolErrorKind};
use pawork_domain::WorkspaceId;
use pawork_workspace::{
    resolve_relative_path, WorkspaceError, WorkspacePathError, WorkspaceService,
};
use serde_json::Value;

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
    Io(#[from] std::io::Error),
    #[error("workspace error: {0}")]
    Workspace(WorkspaceError),
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
