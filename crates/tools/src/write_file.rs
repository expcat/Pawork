//! `write_file` 工具。
//!
//! 原子写（tmp+sync+rename）、建父目录、保留已有文件权限。路径走 policy 安全内核。

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
use serde_json::{json, Value};

use crate::common::atomic_write;
use crate::common::require_str;
use crate::common::resolve_write_rel;
use crate::common::workspace_roots;
use crate::common::BuiltinToolError;

/// `write_file` 工具。
#[derive(Clone)]
pub struct WriteFileTool {
    workspaces: WorkspaceService,
}

impl WriteFileTool {
    pub fn new(workspaces: WorkspaceService) -> Self {
        Self { workspaces }
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
            kind: ToolKind::ClientFunction,
            hosting: ToolHosting::Local,
            capabilities: Vec::new(),
            requires_approval: false,
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
        match write(&self.workspaces, &context.workspace_id, &request.input) {
            Ok(result) => Ok(result),
            Err(error) => Err(BuiltinToolError::from(error).into()),
        }
    }
}

fn write(
    service: &WorkspaceService,
    workspace_id: &WorkspaceId,
    input: &Value,
) -> Result<ToolResult, WriteFileError> {
    let path = require_str(input, "path")?;
    let content = require_str(input, "content")?;
    let roots = workspace_roots(service, workspace_id)?;
    let absolute = resolve_write_rel(&roots, &path)?;

    atomic_write(&absolute, content.as_bytes())?;

    let metadata = json!({
        "path": path,
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
}

impl From<WriteFileError> for BuiltinToolError {
    fn from(error: WriteFileError) -> Self {
        match error {
            WriteFileError::Common(common) => common,
            WriteFileError::Io(io) => BuiltinToolError::Io(io),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pawork_domain::ToolErrorKind;
    use pawork_domain::WorkspaceId;
    use pawork_policy::PathSafetyError;
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(1);

    fn temp_root(name: &str) -> tempfile::TempDir {
        tempfile::Builder::new()
            .prefix(&format!(
                "pawork-writefile-{}-{}-{name}-",
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

    fn write_input(path: &str, content: &str) -> Value {
        json!({"path": path, "content": content})
    }

    fn assert_no_host_absolute(res: &ToolResult, root: &std::path::Path) {
        let meta = res.metadata.to_string();
        let body = match &res.content[0] {
            ContentPart::Text(t) => t.text.clone(),
            _ => String::new(),
        };
        let root_str = root.display().to_string();
        assert!(
            res.metadata.get("absolute").is_none(),
            "metadata must not contain absolute path"
        );
        assert!(
            !meta.contains(&root_str),
            "metadata leaked host path: {meta}"
        );
        assert!(!body.contains(&root_str), "body leaked host path: {body}");
    }

    #[test]
    fn creates_and_overwrites_atomically() {
        let (service, id, root, _ws_dir) = make_service();
        let first = write(&service, &id, &write_input("a.txt", "v1")).expect("write");
        assert_eq!(fs::read_to_string(root.join("a.txt")).unwrap(), "v1");
        assert_no_host_absolute(&first, &root);
        write(&service, &id, &write_input("a.txt", "v2")).expect("write");
        assert_eq!(fs::read_to_string(root.join("a.txt")).unwrap(), "v2");
    }

    #[test]
    fn creates_parent_directories() {
        let (service, id, root, _ws_dir) = make_service();
        write(&service, &id, &write_input("nested/dir/b.txt", "hi")).expect("write");
        assert_eq!(
            fs::read_to_string(root.join("nested/dir/b.txt")).unwrap(),
            "hi"
        );
    }

    #[test]
    fn overwrite_replaces_file_content() {
        let (service, id, root, _ws_dir) = make_service();
        fs::write(root.join("c.txt"), "original").unwrap();
        write(&service, &id, &write_input("c.txt", "changed")).expect("write");
        assert_eq!(fs::read_to_string(root.join("c.txt")).unwrap(), "changed");
    }

    #[test]
    fn rejects_absolute_and_traversal_paths() {
        let (service, id, root, _ws_dir) = make_service();
        fs::write(root.join("ok.txt"), "hi").unwrap();
        let abs = root.join("ok.txt");
        let err = write(&service, &id, &write_input(&abs.display().to_string(), "x")).unwrap_err();
        assert!(matches!(
            err,
            WriteFileError::Common(BuiltinToolError::PolicyPath(PathSafetyError::AbsolutePath))
        ));
        let error: ToolError = BuiltinToolError::from(err).into();
        assert_eq!(error.kind, ToolErrorKind::PermissionDenied);

        let err = write(&service, &id, &write_input("../escape.txt", "x")).unwrap_err();
        assert!(matches!(
            err,
            WriteFileError::Common(BuiltinToolError::PolicyPath(PathSafetyError::Traversal(_)))
        ));
        let error: ToolError = BuiltinToolError::from(err).into();
        assert_eq!(error.kind, ToolErrorKind::PermissionDenied);
    }
}
