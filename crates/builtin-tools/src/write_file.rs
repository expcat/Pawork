//! `write_file` 工具（P4-2）。
//!
//! 原子写（tmp+sync+rename）、建父目录、保留已有文件权限、写入前 checkpoint。

use agent_domain::{ContentPart, TextContent, WorkspaceId};
use async_trait::async_trait;
use checkpoint_service::CheckpointService;
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
use crate::common::require_str;
use crate::common::resolve_rel;
use crate::common::workspace_roots;
use crate::common::BuiltinToolError;

/// `write_file` 工具。
#[derive(Clone)]
pub struct WriteFileTool {
    workspaces: WorkspaceService,
    checkpoints: CheckpointService,
}

impl WriteFileTool {
    pub fn new(workspaces: WorkspaceService, checkpoints: CheckpointService) -> Self {
        Self {
            workspaces,
            checkpoints,
        }
    }
}

#[async_trait]
impl AgentTool for WriteFileTool {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "write_file".into(),
            description: "Atomically write a workspace-relative file, creating parent directories. Overwrites require approval (enforced by the scheduler/policy).".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "content": { "type": "string" }
                },
                "required": ["path", "content"]
            }),
            capability: ToolCapability::WorkspaceWrite,
            read_only: false,
            supports_concurrency: false,
            default_timeout_ms: Some(10_000),
            max_output_bytes: 16 * 1024,
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
        let workspaces = self.workspaces.clone();
        let checkpoints = self.checkpoints.clone();
        match write(
            &workspaces,
            &checkpoints,
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

async fn write(
    service: &WorkspaceService,
    checkpoints: &CheckpointService,
    workspace_id: &WorkspaceId,
    run_id: &agent_domain::RunId,
    tool_call_id: &agent_domain::ToolCallId,
    input: &Value,
) -> Result<ToolResult, WriteFileError> {
    let path = require_str(input, "path")?;
    let content = require_str(input, "content")?;
    let roots = workspace_roots(service, workspace_id)?;
    let absolute = resolve_rel(&roots, &path)?;

    // 写入前 checkpoint：保存当前内容以便回滚。
    checkpoints
        .snapshot_before_write(run_id.as_ref(), &call_key(tool_call_id), &roots, &path)
        .await
        .map_err(|e| WriteFileError::Checkpoint(e.to_string()))?;

    atomic_write(&absolute, content.as_bytes())?;

    let metadata = json!({
        "path": path,
        "absolute": absolute.display().to_string(),
        "bytes": content.len(),
    });
    Ok(ToolResult {
        content: vec![ContentPart::Text(TextContent {
            text: format!("wrote {} bytes to {path}", content.len()),
        })],
        artifacts: Vec::new(),
        metadata,
        truncated: false,
        success: true,
        error: None,
    })
}

#[derive(Debug, thiserror::Error)]
pub enum WriteFileError {
    #[error(transparent)]
    Common(#[from] BuiltinToolError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("checkpoint error: {0}")]
    Checkpoint(String),
}

impl From<WriteFileError> for BuiltinToolError {
    fn from(error: WriteFileError) -> Self {
        match error {
            WriteFileError::Common(common) => common,
            WriteFileError::Io(io) => BuiltinToolError::Io(io),
            WriteFileError::Checkpoint(msg) => BuiltinToolError::Checkpoint(msg),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::resolve_rel;
    use agent_domain::Timestamp;
    use agent_domain::WorkspaceId;
    use artifact_store::ArtifactStore;
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(1);

    fn temp_root(name: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "pawork-writefile-{}-{}-{name}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).expect("mkdir");
        path
    }

    struct Env {
        service: WorkspaceService,
        checkpoints: CheckpointService,
        id: WorkspaceId,
        root: std::path::PathBuf,
        _store: ArtifactStore,
    }

    async fn make_env() -> Env {
        let root = temp_root("ws");
        let store_root = temp_root("store");
        let store = ArtifactStore::open(&store_root).await.expect("open store");
        let checkpoints = CheckpointService::new(store.clone());
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
        Env {
            service,
            checkpoints,
            id,
            root,
            _store: store,
        }
    }

    fn write_input(path: &str, content: &str) -> Value {
        json!({"path": path, "content": content})
    }

    #[tokio::test]
    async fn creates_and_overwrites_atomically() {
        let env = make_env().await;
        let rid = agent_domain::RunId::from("run-1");
        let tid = agent_domain::ToolCallId::from("call-1");
        write(
            &env.service,
            &env.checkpoints,
            &env.id,
            &rid,
            &tid,
            &write_input("a.txt", "v1"),
        )
        .await
        .expect("write");
        assert_eq!(fs::read_to_string(env.root.join("a.txt")).unwrap(), "v1");
        write(
            &env.service,
            &env.checkpoints,
            &env.id,
            &rid,
            &tid,
            &write_input("a.txt", "v2"),
        )
        .await
        .expect("write");
        assert_eq!(fs::read_to_string(env.root.join("a.txt")).unwrap(), "v2");
    }

    #[tokio::test]
    async fn creates_parent_directories() {
        let env = make_env().await;
        let rid = agent_domain::RunId::from("run-1");
        let tid = agent_domain::ToolCallId::from("call-1");
        write(
            &env.service,
            &env.checkpoints,
            &env.id,
            &rid,
            &tid,
            &write_input("nested/dir/b.txt", "hi"),
        )
        .await
        .expect("write");
        assert_eq!(
            fs::read_to_string(env.root.join("nested/dir/b.txt")).unwrap(),
            "hi"
        );
    }

    #[tokio::test]
    async fn rollback_restores_original_content() {
        let env = make_env().await;
        fs::write(env.root.join("c.txt"), "original").unwrap();
        let rid = agent_domain::RunId::from("run-1");
        let tid = agent_domain::ToolCallId::from("call-1");
        write(
            &env.service,
            &env.checkpoints,
            &env.id,
            &rid,
            &tid,
            &write_input("c.txt", "changed"),
        )
        .await
        .expect("write");
        assert_eq!(
            fs::read_to_string(env.root.join("c.txt")).unwrap(),
            "changed"
        );
        // 回滚恢复原内容。
        env.checkpoints
            .rollback_tool_call(rid.as_ref(), tid.as_ref())
            .await
            .expect("rollback");
        assert_eq!(
            fs::read_to_string(env.root.join("c.txt")).unwrap(),
            "original"
        );
    }

    #[test]
    fn rejects_traversal() {
        // 仅验证路径解析层（无需 async）。
        let roots = vec![temp_root("ws2")];
        let err = resolve_rel(&roots, "../escape.txt").unwrap_err();
        assert!(matches!(err, BuiltinToolError::Path(_)));
    }
}
