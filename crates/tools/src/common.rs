//! 内置工具共享模块：输入解析、工作区路径解析、错误映射。

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use pawork_domain::WorkspaceId;
use pawork_domain::{ToolError, ToolErrorKind};
use pawork_policy::{resolve_workspace_path, PathSafetyError};
use pawork_workspace::{WorkspaceError, WorkspacePathError, WorkspaceService};
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
                | WorkspacePathError::ReservedDeviceName(_)
                | WorkspacePathError::SymlinkEscape
                | WorkspacePathError::GitInternals
                | WorkspacePathError::NonRegular => {
                    (ToolErrorKind::PermissionDenied, path.to_string())
                }
                WorkspacePathError::Io(io) => {
                    if io.kind() == std::io::ErrorKind::NotFound {
                        (ToolErrorKind::NotFound, io.to_string())
                    } else {
                        (ToolErrorKind::ExecutionFailed, io.to_string())
                    }
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
pub fn opt_str(input: &Value, key: &'static str) -> Result<Option<String>, BuiltinToolError> {
    opt_typed(input, key, "string", |v| v.as_str().map(str::to_owned))
}

/// 取可选 u64 字段。
pub fn opt_u64(input: &Value, key: &'static str) -> Result<Option<u64>, BuiltinToolError> {
    opt_typed(input, key, "integer", Value::as_u64)
}

/// 取可选 bool 字段。
pub fn opt_bool(input: &Value, key: &'static str) -> Result<Option<bool>, BuiltinToolError> {
    opt_typed(input, key, "boolean", Value::as_bool)
}

/// 可选字段统一取值：缺省或显式 `null` 视为未提供；存在但类型不符报
/// [`InvalidField`](BuiltinToolError::InvalidField)，给模型纠错信号。
fn opt_typed<T>(
    input: &Value,
    key: &'static str,
    expected: &'static str,
    cast: impl FnOnce(&Value) -> Option<T>,
) -> Result<Option<T>, BuiltinToolError> {
    match input.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => cast(value)
            .map(Some)
            .ok_or_else(|| BuiltinToolError::InvalidField {
                field: key,
                detail: format!("expected {expected}, got {value}"),
            }),
    }
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

/// 读写共用路径解析：走 policy 安全内核（越界 / symlink / `.git` / 非普通文件）。
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
    let (temp, mut file) = create_temp_file(path, &TEMP_COUNTER)?;
    let result = (|| {
        file.write_all(content)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temp, path)
    })();
    if result.is_err() {
        // 临时文件由本次调用独占创建，失败时只清理这一个路径。
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

/// 同目录独占创建临时文件（R-01）：create_new 拒绝跟随已存在路径——
/// 目标目录中被预置或竞速创建的同名 symlink 不会被打开写入，只会换名
/// 重试；有界次数后放弃。调用方据此保证只清理自己确实创建的文件。
fn create_temp_file(path: &Path, counter: &AtomicU64) -> std::io::Result<(PathBuf, std::fs::File)> {
    const MAX_ATTEMPTS: u32 = 16;
    let mut attempt = 0u32;
    loop {
        let candidate = path.with_file_name(format!(
            ".pawork-tmp-{}-{}",
            std::process::id(),
            counter.fetch_add(1, Ordering::Relaxed)
        ));
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(file) => return Ok((candidate, file)),
            Err(error)
                if error.kind() == std::io::ErrorKind::AlreadyExists
                    && attempt + 1 < MAX_ATTEMPTS =>
            {
                attempt += 1;
            }
            Err(error) => return Err(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn optional_fields_parse_or_default_to_none() {
        let input = json!({"name": "a", "count": 3, "flag": true});
        assert_eq!(opt_str(&input, "name").unwrap().as_deref(), Some("a"));
        assert_eq!(opt_u64(&input, "count").unwrap(), Some(3));
        assert_eq!(opt_bool(&input, "flag").unwrap(), Some(true));
        assert_eq!(opt_str(&input, "missing").unwrap(), None);
        assert_eq!(opt_u64(&input, "missing").unwrap(), None);
        // 显式 null 视为未提供。
        let nulled = json!({"count": null});
        assert_eq!(opt_u64(&nulled, "count").unwrap(), None);
    }

    #[test]
    fn optional_fields_reject_type_mismatch() {
        let input = json!({"count": "3"});
        let error = opt_u64(&input, "count").unwrap_err();
        assert!(matches!(
            error,
            BuiltinToolError::InvalidField { field: "count", .. }
        ));
        let mapped: ToolError = error.into();
        assert_eq!(mapped.kind, ToolErrorKind::InvalidInput);
    }
    /// R-01：预置同名 symlink 不能被独占创建跟随；哨兵文件内容不变，
    /// 临时文件改名落到其它候选名。
    #[cfg(unix)]
    #[test]
    fn create_temp_file_refuses_planted_symlink() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target.txt");
        let sentinel_dir = tempfile::tempdir().unwrap();
        let sentinel = sentinel_dir.path().join("sentinel.txt");
        std::fs::write(&sentinel, b"sentinel").unwrap();

        let counter = AtomicU64::new(0);
        let planted = target.with_file_name(format!(".pawork-tmp-{}-0", std::process::id()));
        symlink(&sentinel, &planted).unwrap();

        let (temp, file) = create_temp_file(&target, &counter).unwrap();
        drop(file);
        assert_ne!(temp, planted, "必须换一个未占用的候选名");
        assert_eq!(
            std::fs::read(&sentinel).unwrap(),
            b"sentinel",
            "预置链接指向的外部哨兵不得被改写"
        );
        assert!(
            planted.symlink_metadata().unwrap().file_type().is_symlink(),
            "预置链接本身不得被覆盖或删除"
        );
        let _ = std::fs::remove_file(&temp);
    }

    /// R-01：atomic_write 全路径——预置一批同名 symlink（少于重试上限），
    /// 写入仍成功且哨兵不变。
    #[cfg(unix)]
    #[test]
    fn atomic_write_succeeds_despite_planted_symlinks() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("out.txt");
        let sentinel_dir = tempfile::tempdir().unwrap();
        let sentinel = sentinel_dir.path().join("sentinel.txt");
        std::fs::write(&sentinel, b"sentinel").unwrap();

        // 预置一小段候选名（少于 MAX_ATTEMPTS，写入必然在段内或段后成功）。
        let start = TEMP_COUNTER.load(Ordering::Relaxed);
        for n in start..start + 8 {
            let name = target.with_file_name(format!(".pawork-tmp-{}-{n}", std::process::id()));
            symlink(&sentinel, &name).unwrap();
        }

        atomic_write(&target, b"new content").unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"new content");
        assert_eq!(std::fs::read(&sentinel).unwrap(), b"sentinel");
    }
}
