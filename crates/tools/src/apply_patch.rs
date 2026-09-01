//! `apply_patch` 工具。
//!
//! 多文件 create/update/delete/rename、dry run、原子提交、部分失败回滚、路径安全。
//! 回滚用执行前本地备份（已存在则读字节，不存在则标记为新建），不依赖 checkpoint。

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use pawork_domain::AgentTool;
use pawork_domain::ToolError;
use pawork_domain::ToolEventSink;
use pawork_domain::ToolExecutionContext;
use pawork_domain::ToolRequest;
use pawork_domain::ToolResult;
use pawork_domain::{
    CancellationToken, ContentPart, TextContent, ToolCapability, ToolDescriptor, ToolHosting,
    ToolKind, WorkspaceId,
};
use pawork_workspace::WorkspaceService;
use serde::Serialize;
use serde_json::{json, Value};

use crate::common::atomic_write;
use crate::common::opt_bool;
use crate::common::resolve_write_rel;
use crate::common::workspace_roots;
use crate::common::BuiltinToolError;

/// `apply_patch` 工具。
#[derive(Clone)]
pub struct ApplyPatchTool {
    workspaces: WorkspaceService,
}

impl ApplyPatchTool {
    pub fn new(workspaces: WorkspaceService) -> Self {
        Self { workspaces }
    }
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
enum OpKind {
    Create,
    Update,
    Delete,
    Rename,
}

#[derive(Clone, Debug)]
struct Op {
    kind: OpKind,
    path: String,
    content: Option<String>,
    to: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct PlannedChange {
    op: OpKind,
    path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    to: Option<String>,
    bytes: usize,
}

enum PathBackup {
    Existing(Vec<u8>),
    Absent,
}

#[async_trait]
impl AgentTool for ApplyPatchTool {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "apply_patch".into(),
            description: "Apply a multi-file patch (create/update/delete/rename) atomically with dry-run and rollback on partial failure.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "ops": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "op": { "type": "string", "enum": ["create", "update", "delete", "rename"] },
                                "path": { "type": "string" },
                                "content": { "type": "string" },
                                "to": { "type": "string" }
                            },
                            "required": ["op", "path"]
                        }
                    },
                    "dry_run": { "type": "boolean" }
                },
                "required": ["ops"]
            }),
            capability: ToolCapability::WorkspaceWrite,
            kind: ToolKind::ClientFunction,
            hosting: ToolHosting::Local,
            capabilities: Vec::new(),
            requires_approval: false,
            read_only: false,
            supports_concurrency: false,
            default_timeout_ms: Some(15_000),
            max_output_bytes: 64 * 1024,
            allowed_in_untrusted_workspace: false,
        }
    }

    async fn execute(
        &self,
        request: ToolRequest,
        context: ToolExecutionContext,
        _sink: &dyn ToolEventSink,
        _cancel: CancellationToken,
    ) -> Result<ToolResult, ToolError> {
        match apply(&self.workspaces, &context.workspace_id, &request.input) {
            Ok(result) => Ok(result),
            Err(error) => Err(BuiltinToolError::from(error).into()),
        }
    }
}

fn apply(
    service: &WorkspaceService,
    workspace_id: &WorkspaceId,
    input: &Value,
) -> Result<ToolResult, ApplyPatchError> {
    let dry_run = opt_bool(input, "dry_run").unwrap_or(false);
    let ops = parse_ops(input)?;
    if ops.is_empty() {
        return Err(ApplyPatchError::Common(BuiltinToolError::Other(
            "no ops provided".into(),
        )));
    }
    let roots = workspace_roots(service, workspace_id)?;

    let mut planned: Vec<(Op, PathBuf, Option<PathBuf>)> = Vec::new();
    for op in &ops {
        let abs = resolve_write_rel(&roots, &op.path)?;
        let to_abs = match &op.to {
            Some(to) => Some(resolve_write_rel(&roots, to)?),
            None => None,
        };
        planned.push((op.clone(), abs, to_abs));
    }

    if dry_run {
        let changes = planned
            .iter()
            .map(|(op, _abs, _to)| PlannedChange {
                op: op.kind,
                path: op.path.clone(),
                to: op.to.clone(),
                bytes: op.content.as_ref().map(|c| c.len()).unwrap_or(0),
            })
            .collect::<Vec<_>>();
        return Ok(preview_result(changes, true));
    }

    let backups = snapshot_involved_paths(&planned)?;

    let mut applied_changes: Vec<PlannedChange> = Vec::new();
    for (op, abs, to_abs) in planned {
        match exec_op(&op, &abs, to_abs.as_deref()) {
            Ok(()) => {
                applied_changes.push(PlannedChange {
                    op: op.kind,
                    path: op.path.clone(),
                    to: op.to.clone(),
                    bytes: op.content.as_ref().map(|c| c.len()).unwrap_or(0),
                });
            }
            Err(err) => {
                let message = match restore_backups(&backups) {
                    Ok(()) => err.to_string(),
                    Err(rollback) => format!("{err}; local rollback failed: {rollback}"),
                };
                return Err(ApplyPatchError::Partial {
                    failed_op: op.path.clone(),
                    message,
                    applied: applied_changes,
                });
            }
        }
    }

    Ok(applied_result(applied_changes, false))
}

fn snapshot_involved_paths(
    planned: &[(Op, PathBuf, Option<PathBuf>)],
) -> Result<HashMap<PathBuf, PathBackup>, ApplyPatchError> {
    let mut backups = HashMap::new();
    for (_op, abs, to_abs) in planned {
        remember_backup(&mut backups, abs)?;
        if let Some(to) = to_abs {
            remember_backup(&mut backups, to)?;
        }
    }
    Ok(backups)
}

fn remember_backup(
    backups: &mut HashMap<PathBuf, PathBackup>,
    path: &Path,
) -> Result<(), ApplyPatchError> {
    if backups.contains_key(path) {
        return Ok(());
    }
    let backup = if path.exists() {
        PathBackup::Existing(fs::read(path)?)
    } else {
        PathBackup::Absent
    };
    backups.insert(path.to_path_buf(), backup);
    Ok(())
}

fn restore_backups(backups: &HashMap<PathBuf, PathBackup>) -> Result<(), std::io::Error> {
    for (path, backup) in backups {
        match backup {
            PathBackup::Existing(bytes) => atomic_write(path, bytes)?,
            PathBackup::Absent => {
                if path.exists() {
                    fs::remove_file(path)?;
                }
            }
        }
    }
    Ok(())
}

fn parse_ops(input: &Value) -> Result<Vec<Op>, ApplyPatchError> {
    let arr = input
        .get("ops")
        .and_then(|v| v.as_array())
        .ok_or(BuiltinToolError::MissingField("ops"))?;
    let mut ops = Vec::new();
    for item in arr {
        let op_str = item.get("op").and_then(|v| v.as_str()).ok_or_else(|| {
            BuiltinToolError::InvalidField {
                field: "op",
                detail: "missing".into(),
            }
        })?;
        let kind = match op_str {
            "create" => OpKind::Create,
            "update" => OpKind::Update,
            "delete" => OpKind::Delete,
            "rename" => OpKind::Rename,
            other => {
                return Err(ApplyPatchError::Common(BuiltinToolError::InvalidField {
                    field: "op",
                    detail: format!("unknown op `{other}`"),
                }));
            }
        };
        ops.push(Op {
            kind,
            path: require_str_in(item, "path")?,
            content: item
                .get("content")
                .and_then(|v| v.as_str())
                .map(String::from),
            to: item.get("to").and_then(|v| v.as_str()).map(String::from),
        });
    }
    Ok(ops)
}

fn require_str_in(item: &Value, key: &'static str) -> Result<String, ApplyPatchError> {
    item.get(key)
        .and_then(|v| v.as_str())
        .map(str::to_owned)
        .ok_or(ApplyPatchError::Common(BuiltinToolError::MissingField(key)))
}

fn exec_op(op: &Op, abs: &Path, to: Option<&Path>) -> Result<(), std::io::Error> {
    match op.kind {
        OpKind::Create | OpKind::Update => {
            let content = op.content.as_deref().unwrap_or("");
            atomic_write(abs, content.as_bytes())
        }
        OpKind::Delete => {
            if abs.exists() {
                fs::remove_file(abs)
            } else {
                Ok(())
            }
        }
        OpKind::Rename => {
            let to = to.ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::InvalidInput, "rename requires `to`")
            })?;
            if let Some(parent) = to.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::rename(abs, to)
        }
    }
}

fn preview_result(changes: Vec<PlannedChange>, dry_run: bool) -> ToolResult {
    let metadata = json!({
        "dry_run": dry_run,
        "changes": serde_json::to_value(&changes).unwrap_or(Value::Null),
    });
    ToolResult {
        content: vec![ContentPart::Text(TextContent {
            text: format!("dry run: {} planned change(s)", changes.len()),
        })],
        artifacts: Vec::new(),
        metadata,
        truncated: false,
        success: true,
        error: None,
    }
}

fn applied_result(changes: Vec<PlannedChange>, dry_run: bool) -> ToolResult {
    let metadata = json!({
        "dry_run": dry_run,
        "changes": serde_json::to_value(&changes).unwrap_or(Value::Null),
    });
    ToolResult {
        content: vec![ContentPart::Text(TextContent {
            text: format!("applied {} change(s)", changes.len()),
        })],
        artifacts: Vec::new(),
        metadata,
        truncated: false,
        success: true,
        error: None,
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ApplyPatchError {
    #[error(transparent)]
    Common(#[from] BuiltinToolError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("partial failure at `{failed_op}`: {message}")]
    Partial {
        failed_op: String,
        message: String,
        applied: Vec<PlannedChange>,
    },
}

impl From<ApplyPatchError> for BuiltinToolError {
    fn from(error: ApplyPatchError) -> Self {
        match error {
            ApplyPatchError::Common(c) => c,
            ApplyPatchError::Io(io) => BuiltinToolError::Io(io),
            ApplyPatchError::Partial { message, .. } => BuiltinToolError::Other(message),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pawork_domain::WorkspaceId;
    use pawork_policy::PathSafetyError;
    use proptest::prelude::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(1);

    fn temp_root(name: &str) -> tempfile::TempDir {
        tempfile::Builder::new()
            .prefix(&format!(
                "pawork-applypatch-{}-{}-{name}-",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ))
            .tempdir()
            .expect("create temp dir")
    }

    fn make_service() -> (
        WorkspaceService,
        WorkspaceId,
        std::path::PathBuf,
        tempfile::TempDir,
    ) {
        let ws_dir = temp_root("ws");
        let root = ws_dir.path().to_path_buf();
        let service = WorkspaceService::new();
        let id = WorkspaceId::from("ws-1");
        service
            .add(id.clone(), "demo", [root.clone()])
            .expect("add");
        (service, id, root, ws_dir)
    }

    #[test]
    fn multi_file_create() {
        let (service, id, root, _ws_dir) = make_service();
        let input = json!({"ops": [
            {"op": "create", "path": "a.txt", "content": "AAA"},
            {"op": "create", "path": "b.txt", "content": "BBB"}
        ]});
        apply(&service, &id, &input).expect("apply");
        assert_eq!(fs::read_to_string(root.join("a.txt")).unwrap(), "AAA");
        assert_eq!(fs::read_to_string(root.join("b.txt")).unwrap(), "BBB");
    }

    #[test]
    fn dry_run_does_not_write() {
        let (service, id, root, _ws_dir) = make_service();
        let input = json!({"dry_run": true, "ops": [
            {"op": "create", "path": "c.txt", "content": "CCC"}
        ]});
        let res = apply(&service, &id, &input).expect("apply");
        assert_eq!(res.metadata["dry_run"], true);
        assert!(!root.join("c.txt").exists());
    }

    #[test]
    fn delete_and_rename() {
        let (service, id, root, _ws_dir) = make_service();
        fs::write(root.join("old.txt"), "data").unwrap();
        let input = json!({"ops": [
            {"op": "rename", "path": "old.txt", "to": "new.txt"},
            {"op": "delete", "path": "gone.txt"}
        ]});
        apply(&service, &id, &input).expect("apply");
        assert!(!root.join("old.txt").exists());
        assert_eq!(fs::read_to_string(root.join("new.txt")).unwrap(), "data");
    }

    #[test]
    fn partial_failure_rolls_back() {
        let (service, id, root, _ws_dir) = make_service();
        // 第二个 op rename 一个不存在的文件 -> 失败；第一个 create 应被回滚删除。
        let input = json!({"ops": [
            {"op": "create", "path": "created.txt", "content": "X"},
            {"op": "rename", "path": "nope.txt", "to": "elsewhere.txt"}
        ]});
        let err = apply(&service, &id, &input).unwrap_err();
        assert!(matches!(err, ApplyPatchError::Partial { .. }));
        assert!(!root.join("created.txt").exists(), "回滚应删除已创建文件");
    }

    fn assert_existing_file_restored(first_op: Value) {
        let (service, id, root, _ws_dir) = make_service();
        fs::write(root.join("target.txt"), "original").unwrap();
        let input = json!({"ops": [
            first_op,
            {"op": "rename", "path": "missing.txt", "to": "elsewhere.txt"}
        ]});
        let err = apply(&service, &id, &input).unwrap_err();
        assert!(matches!(err, ApplyPatchError::Partial { .. }));
        assert_eq!(
            fs::read_to_string(root.join("target.txt")).unwrap(),
            "original"
        );
    }

    #[test]
    fn create_over_existing_is_restored_after_partial_failure() {
        assert_existing_file_restored(
            json!({"op": "create", "path": "target.txt", "content": "replacement"}),
        );
    }

    #[test]
    fn update_is_restored_after_partial_failure() {
        assert_existing_file_restored(
            json!({"op": "update", "path": "target.txt", "content": "replacement"}),
        );
    }

    #[test]
    fn delete_is_restored_after_partial_failure() {
        assert_existing_file_restored(json!({"op": "delete", "path": "target.txt"}));
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(16))]

        #[test]
        fn partial_failure_rollback_is_byte_exact(
            original in proptest::collection::vec(any::<u8>(), 0..256),
            replacement in proptest::collection::vec(any::<u8>(), 0..256),
        ) {
            let (service, id, root, _ws_dir) = make_service();
            fs::write(root.join("target.bin"), &original).unwrap();
            let replacement_text = String::from_utf8_lossy(&replacement).into_owned();
            let input = json!({"ops": [
                {"op": "update", "path": "target.bin", "content": replacement_text},
                {"op": "rename", "path": "missing.bin", "to": "elsewhere.bin"}
            ]});
            let error = apply(&service, &id, &input).unwrap_err();
            assert!(matches!(error, ApplyPatchError::Partial { .. }));
            prop_assert_eq!(fs::read(root.join("target.bin")).unwrap(), original);
        }
    }

    #[test]
    fn rejects_traversal_in_op_path() {
        let (service, id, _root, _ws_dir) = make_service();
        let input = json!({"ops": [
            {"op": "create", "path": "../escape.txt", "content": "X"}
        ]});
        let err = apply(&service, &id, &input).unwrap_err();
        assert!(matches!(
            err,
            ApplyPatchError::Common(BuiltinToolError::PolicyPath(PathSafetyError::Traversal(_)))
        ));
        let error: ToolError = BuiltinToolError::from(err).into();
        assert_eq!(error.kind, pawork_domain::ToolErrorKind::PermissionDenied);
    }
}
