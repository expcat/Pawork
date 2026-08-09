//! `apply_patch` 工具（P4-4）。
//!
//! 多文件 create/update/delete/rename、dry run、原子提交、部分失败回滚、路径安全。

use std::fs;
use std::path::Path;

use agent_domain::{ContentPart, TextContent, WorkspaceId};
use async_trait::async_trait;
use checkpoint_service::CheckpointService;
use serde::Serialize;
use serde_json::{json, Value};
use tool_api::AgentTool;
use tool_api::CancellationToken;
use tool_api::ToolCapability;
use tool_api::ToolDescriptor;
use tool_api::ToolError;
use tool_api::ToolEventSink;
use tool_api::ToolExecutionContext;
use tool_api::ToolRequest;
use tool_api::ToolResult;
use workspace_service::WorkspaceService;

use crate::common::atomic_write;
use crate::common::call_key;
use crate::common::opt_bool;
use crate::common::resolve_rel;
use crate::common::workspace_roots;
use crate::common::BuiltinToolError;

/// `apply_patch` 工具。
#[derive(Clone)]
pub struct ApplyPatchTool {
    workspaces: WorkspaceService,
    checkpoints: CheckpointService,
}

impl ApplyPatchTool {
    pub fn new(workspaces: WorkspaceService, checkpoints: CheckpointService) -> Self {
        Self {
            workspaces,
            checkpoints,
        }
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
                                "to": { "type": "string" },
                                "old_string": { "type": "string" }
                            },
                            "required": ["op", "path"]
                        }
                    },
                    "dry_run": { "type": "boolean" }
                },
                "required": ["ops"]
            }),
            capability: ToolCapability::WorkspaceWrite,
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
        match apply(
            &self.workspaces,
            &self.checkpoints,
            &context.workspace_id,
            &context.run_id,
            &request.tool_call_id,
            &request.input,
        )
        .await
        {
            Ok(result) => Ok(result),
            Err(error) => Err(BuiltinToolError::from(error).into()),
        }
    }
}

async fn apply(
    service: &WorkspaceService,
    checkpoints: &CheckpointService,
    workspace_id: &WorkspaceId,
    run_id: &agent_domain::RunId,
    tool_call_id: &agent_domain::ToolCallId,
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

    // 解析每个 op 涉及的路径（含 to）为绝对，并收集计划。
    let mut planned: Vec<(Op, std::path::PathBuf, Option<std::path::PathBuf>)> = Vec::new();
    for op in &ops {
        let abs = resolve_rel(&roots, &op.path)?;
        let to_abs = match &op.to {
            Some(to) => Some(resolve_rel(&roots, to)?),
            None => None,
        };
        planned.push((op.clone(), abs, to_abs));
    }

    // dry_run：只返回计划，不落盘。
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

    // 写前 snapshot：对每个将被改/删的路径快照（用于回滚）。
    let call_key = call_key(tool_call_id);
    for (op, _abs, _to) in &planned {
        match op.kind {
            OpKind::Create => {
                checkpoints
                    .snapshot_before_write(run_id.as_ref(), &call_key, &roots, &op.path)
                    .await
                    .map_err(|e| ApplyPatchError::Checkpoint(e.to_string()))?;
            }
            OpKind::Update | OpKind::Delete => {
                checkpoints
                    .snapshot_before_write(run_id.as_ref(), &call_key, &roots, &op.path)
                    .await
                    .map_err(|e| ApplyPatchError::Checkpoint(e.to_string()))?;
            }
            OpKind::Rename => {
                checkpoints
                    .snapshot_before_write(run_id.as_ref(), &call_key, &roots, &op.path)
                    .await
                    .map_err(|e| ApplyPatchError::Checkpoint(e.to_string()))?;
                if let Some(to) = op.to.as_deref() {
                    checkpoints
                        .snapshot_before_write(run_id.as_ref(), &call_key, &roots, to)
                        .await
                        .map_err(|e| ApplyPatchError::Checkpoint(e.to_string()))?;
                }
            }
        }
    }

    // 原子执行：记录已完成的操作，失败时回滚已执行部分。
    let mut applied_changes: Vec<PlannedChange> = Vec::new();
    for (op, abs, to_abs) in planned {
        let result = exec_op(&op, &abs, to_abs.as_deref());
        match result {
            Ok(()) => {
                applied_changes.push(PlannedChange {
                    op: op.kind,
                    path: op.path.clone(),
                    to: op.to.clone(),
                    bytes: op.content.as_ref().map(|c| c.len()).unwrap_or(0),
                });
            }
            Err(err) => {
                // 统一由 checkpoint 恢复原内容；create-over-existing 也不得被误删。
                let message = match checkpoints
                    .rollback_tool_call(run_id.as_ref(), &call_key)
                    .await
                {
                    Ok(_) => err.to_string(),
                    Err(rollback) => format!("{err}; checkpoint rollback failed: {rollback}"),
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
    #[error("checkpoint error: {0}")]
    Checkpoint(String),
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
            ApplyPatchError::Checkpoint(m) => BuiltinToolError::Checkpoint(m),
            ApplyPatchError::Partial { message, .. } => BuiltinToolError::Other(message),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_domain::Timestamp;
    use agent_domain::WorkspaceId;
    use artifact_store::ArtifactStore;
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

    async fn make_env() -> (
        WorkspaceService,
        CheckpointService,
        WorkspaceId,
        std::path::PathBuf,
        tempfile::TempDir,
        tempfile::TempDir,
    ) {
        let ws_dir = temp_root("ws");
        let store_dir = temp_root("store");
        let root = ws_dir.path().to_path_buf();
        let store_root = store_dir.path().to_path_buf();
        let store = ArtifactStore::open(&store_root).await.expect("open store");
        let checkpoints = CheckpointService::new(store);
        let service = WorkspaceService::new();
        let id = WorkspaceId::from("ws-1");
        service
            .add(
                id.clone(),
                "demo",
                [root.clone()],
                Timestamp::from_unix_millis(1),
            )
            .expect("add");
        (service, checkpoints, id, root, ws_dir, store_dir)
    }

    fn rid() -> agent_domain::RunId {
        agent_domain::RunId::from("r1")
    }
    fn tid() -> agent_domain::ToolCallId {
        agent_domain::ToolCallId::from("t1")
    }

    #[tokio::test]
    async fn multi_file_create() {
        let (service, checkpoints, id, root, _ws_dir, _store_dir) = make_env().await;
        let input = json!({"ops": [
        {"op": "create", "path": "a.txt", "content": "AAA"},
        {"op": "create", "path": "b.txt", "content": "BBB"}
        ]});
        apply(&service, &checkpoints, &id, &rid(), &tid(), &input)
            .await
            .expect("apply");
        assert_eq!(fs::read_to_string(root.join("a.txt")).unwrap(), "AAA");
        assert_eq!(fs::read_to_string(root.join("b.txt")).unwrap(), "BBB");
    }

    #[tokio::test]
    async fn dry_run_does_not_write() {
        let (service, checkpoints, id, root, _ws_dir, _store_dir) = make_env().await;
        let input = json!({"dry_run": true, "ops": [
        {"op": "create", "path": "c.txt", "content": "CCC"}
        ]});
        let res = apply(&service, &checkpoints, &id, &rid(), &tid(), &input)
            .await
            .expect("apply");
        assert_eq!(res.metadata["dry_run"], true);
        assert!(!root.join("c.txt").exists());
    }

    #[tokio::test]
    async fn delete_and_rename() {
        let (service, checkpoints, id, root, _ws_dir, _store_dir) = make_env().await;
        fs::write(root.join("old.txt"), "data").unwrap();
        let input = json!({"ops": [
        {"op": "rename", "path": "old.txt", "to": "new.txt"},
        {"op": "delete", "path": "gone.txt"}
        ]});
        apply(&service, &checkpoints, &id, &rid(), &tid(), &input)
            .await
            .expect("apply");
        assert!(!root.join("old.txt").exists());
        assert_eq!(fs::read_to_string(root.join("new.txt")).unwrap(), "data");
    }

    #[tokio::test]
    async fn partial_failure_rolls_back() {
        let (service, checkpoints, id, root, _ws_dir, _store_dir) = make_env().await;
        // 第二个 op rename 一个不存在的文件 -> 失败；第一个 create 应被回滚删除。
        let input = json!({"ops": [
        {"op": "create", "path": "created.txt", "content": "X"},
        {"op": "rename", "path": "nope.txt", "to": "elsewhere.txt"}
        ]});
        let err = apply(&service, &checkpoints, &id, &rid(), &tid(), &input)
            .await
            .unwrap_err();
        assert!(matches!(err, ApplyPatchError::Partial { .. }));
        assert!(!root.join("created.txt").exists(), "回滚应删除已创建文件");
    }

    async fn assert_existing_file_restored(first_op: Value) {
        let (service, checkpoints, id, root, _ws_dir, _store_dir) = make_env().await;
        fs::write(root.join("target.txt"), "original").unwrap();
        let input = json!({"ops": [
            first_op,
            {"op": "rename", "path": "missing.txt", "to": "elsewhere.txt"}
        ]});
        let err = apply(&service, &checkpoints, &id, &rid(), &tid(), &input)
            .await
            .unwrap_err();
        assert!(matches!(err, ApplyPatchError::Partial { .. }));
        assert_eq!(
            fs::read_to_string(root.join("target.txt")).unwrap(),
            "original"
        );
    }

    #[tokio::test]
    async fn create_over_existing_is_restored_after_partial_failure() {
        assert_existing_file_restored(
            json!({"op": "create", "path": "target.txt", "content": "replacement"}),
        )
        .await;
    }

    #[tokio::test]
    async fn update_is_restored_after_partial_failure() {
        assert_existing_file_restored(
            json!({"op": "update", "path": "target.txt", "content": "replacement"}),
        )
        .await;
    }

    #[tokio::test]
    async fn delete_is_restored_after_partial_failure() {
        assert_existing_file_restored(json!({"op": "delete", "path": "target.txt"})).await;
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(16))]

        #[test]
        fn partial_failure_rollback_is_byte_exact(
            original in proptest::collection::vec(any::<u8>(), 0..256),
            replacement in proptest::collection::vec(any::<u8>(), 0..256),
        ) {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("runtime");
            runtime.block_on(async {
                let (service, checkpoints, id, root, _ws_dir, _store_dir) = make_env().await;
                fs::write(root.join("target.bin"), &original).unwrap();
                let replacement_text = String::from_utf8_lossy(&replacement).into_owned();
                let input = json!({"ops": [
                    {"op": "update", "path": "target.bin", "content": replacement_text},
                    {"op": "rename", "path": "missing.bin", "to": "elsewhere.bin"}
                ]});
                let error = apply(&service, &checkpoints, &id, &rid(), &tid(), &input)
                    .await
                    .unwrap_err();
                assert!(matches!(error, ApplyPatchError::Partial { .. }));
                prop_assert_eq!(fs::read(root.join("target.bin")).unwrap(), original);
                Ok(())
            })?;
        }
    }

    #[tokio::test]
    async fn rejects_traversal_in_op_path() {
        let (service, checkpoints, id, _root, _ws_dir, _store_dir) = make_env().await;
        let input = json!({"ops": [
        {"op": "create", "path": "../escape.txt", "content": "X"}
        ]});
        let err = apply(&service, &checkpoints, &id, &rid(), &tid(), &input)
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            ApplyPatchError::Common(BuiltinToolError::Path(_))
        ));
    }
}
