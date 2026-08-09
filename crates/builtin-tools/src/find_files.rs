//! `find_files` 工具（P4-7）。
//!
//! glob 匹配、类型过滤、ignore、最大深度/结果、稳定排序。

use agent_domain::{ContentPart, TextContent, WorkspaceId};
use async_trait::async_trait;
use globset::GlobSet;
use ignore::WalkBuilder;
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

use crate::common::opt_u64;
use crate::common::require_str;
use crate::common::workspace_roots;
use crate::common::BuiltinToolError;

const DEFAULT_MAX_RESULTS: u64 = 200;
const MAX_OUTPUT_BYTES: u64 = 256 * 1024;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum FileTypeFilter {
    #[default]
    File,
    Dir,
    Any,
}

/// `find_files` 工具。
#[derive(Clone)]
pub struct FindFilesTool {
    workspaces: WorkspaceService,
}

impl FindFilesTool {
    pub fn new(workspaces: WorkspaceService) -> Self {
        Self { workspaces }
    }
}

#[async_trait]
impl AgentTool for FindFilesTool {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "find_files".into(),
            description: "Find workspace files by glob pattern, respecting ignore rules, with type and depth filters.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "pattern": { "type": "string" },
                    "file_type": { "type": "string", "enum": ["file", "dir", "any"] },
                    "max_depth": { "type": "integer", "minimum": 0 },
                    "max_results": { "type": "integer", "minimum": 1 }
                },
                "required": ["pattern"]
            }),
            capability: ToolCapability::ReadOnly,
            read_only: true,
            supports_concurrency: true,
            default_timeout_ms: Some(10_000),
            max_output_bytes: MAX_OUTPUT_BYTES,
            allowed_in_untrusted_workspace: true,
        }
    }

    async fn execute(
        &self,
        request: ToolRequest,
        context: ToolExecutionContext,
        _sink: &dyn ToolEventSink,
        cancel: CancellationToken,
    ) -> Result<ToolResult, ToolError> {
        let service = self.workspaces.clone();
        let workspace_id = context.workspace_id;
        let input = request.input;
        let result =
            tokio::task::spawn_blocking(move || find(&service, &workspace_id, &input, &cancel))
                .await
                .map_err(|error| ToolError {
                    kind: tool_api::ToolErrorKind::Internal,
                    message: format!("find_files worker failed: {error}"),
                    retryable: false,
                    retry_after_ms: None,
                })?;
        match result {
            Ok(result) => Ok(result),
            Err(FindFilesError::Cancelled) => Err(ToolError::cancelled("find_files cancelled")),
            Err(error) => Err(BuiltinToolError::from(error).into()),
        }
    }
}

fn find(
    service: &WorkspaceService,
    workspace_id: &WorkspaceId,
    input: &Value,
    cancel: &CancellationToken,
) -> Result<ToolResult, FindFilesError> {
    if cancel.is_cancelled() {
        return Err(FindFilesError::Cancelled);
    }
    let pattern = require_str(input, "pattern")?;
    let file_type = crate::common::opt_str(input, "file_type")
        .map(|s| match s.as_str() {
            "dir" => FileTypeFilter::Dir,
            "any" => FileTypeFilter::Any,
            _ => FileTypeFilter::File,
        })
        .unwrap_or_default();
    let max_depth = opt_u64(input, "max_depth").map(|d| d as usize);
    let max_results = opt_u64(input, "max_results").unwrap_or(DEFAULT_MAX_RESULTS) as usize;

    let roots = workspace_roots(service, workspace_id)?;
    let glob_set = build_glob_set(&pattern)?;

    let mut found: Vec<String> = Vec::new();
    let mut truncated = false;
    let mut visited = 0usize;

    for root in &roots {
        let mut builder = WalkBuilder::new(root);
        builder.ignore(true).git_ignore(true).git_exclude(true);
        if let Some(depth) = max_depth {
            builder.max_depth(Some(depth));
        }
        let walker = builder.build();
        for entry in walker {
            visited += 1;
            if visited % 64 == 0 && cancel.is_cancelled() {
                return Err(FindFilesError::Cancelled);
            }
            if found.len() >= max_results {
                truncated = true;
                break;
            }
            let entry = entry?;
            let path = entry.path();
            let rel = match path.strip_prefix(root) {
                Ok(rel) => rel,
                Err(_) => continue,
            };
            if rel.as_os_str().is_empty() {
                continue;
            }
            let ftype = entry.file_type();
            let keep = match file_type {
                FileTypeFilter::File => ftype.map(|t| t.is_file()).unwrap_or(false),
                FileTypeFilter::Dir => ftype.map(|t| t.is_dir()).unwrap_or(false),
                FileTypeFilter::Any => true,
            };
            if !keep {
                continue;
            }
            if glob_set.is_match(rel) || glob_set.is_match(path) {
                found.push(rel.display().to_string());
            }
        }
        if truncated {
            break;
        }
    }

    // 稳定排序：路径字典序。
    found.sort();

    let body = found.join("\n");
    let metadata = json!({
        "pattern": pattern,
        "count": found.len(),
        "truncated": truncated,
    });
    Ok(ToolResult {
        content: vec![ContentPart::Text(TextContent { text: body })],
        artifacts: Vec::new(),
        metadata,
        truncated,
        success: true,
        error: None,
    })
}

fn build_glob_set(pattern: &str) -> Result<GlobSet, FindFilesError> {
    let mut builder = globset::GlobSetBuilder::new();
    for piece in pattern.split(',') {
        let piece = piece.trim();
        if !piece.is_empty() {
            builder.add(
                globset::Glob::new(piece)
                    .map_err(|e| FindFilesError::Common(BuiltinToolError::Other(e.to_string())))?,
            );
        }
    }
    builder
        .build()
        .map_err(|e| FindFilesError::Common(BuiltinToolError::Other(e.to_string())))
}

#[derive(Debug, thiserror::Error)]
pub enum FindFilesError {
    #[error(transparent)]
    Common(#[from] BuiltinToolError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Ignore(#[from] ignore::Error),
    #[error("find cancelled")]
    Cancelled,
}

impl From<FindFilesError> for BuiltinToolError {
    fn from(error: FindFilesError) -> Self {
        match error {
            FindFilesError::Common(common) => common,
            FindFilesError::Io(io) => BuiltinToolError::Io(io),
            FindFilesError::Ignore(e) => BuiltinToolError::Other(e.to_string()),
            FindFilesError::Cancelled => BuiltinToolError::Other("find_files cancelled".into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_domain::Timestamp;
    use agent_domain::WorkspaceId;
    use std::fs;
    use std::sync::atomic::AtomicU64;
    use std::sync::atomic::Ordering;

    static NEXT: AtomicU64 = AtomicU64::new(1);

    fn temp_root(name: &str) -> tempfile::TempDir {
        tempfile::Builder::new()
            .prefix(&format!(
                "pawork-find-{}-{}-{name}-",
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
            .add(
                id.clone(),
                "demo",
                [root.clone()],
                Timestamp::from_unix_millis(1),
            )
            .expect("add");
        (service, id, root, ws_dir)
    }

    #[test]
    fn glob_matches_files_sorted() {
        let (service, id, root, _ws_dir) = make_service();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/b.rs"), "").unwrap();
        fs::write(root.join("src/a.rs"), "").unwrap();
        fs::write(root.join("readme.md"), "").unwrap();
        let res = find(
            &service,
            &id,
            &json!({"pattern": "**/*.rs"}),
            &CancellationToken::new(),
        )
        .expect("find");
        let text = match &res.content[0] {
            ContentPart::Text(t) => &t.text,
            _ => panic!("text"),
        };
        let lines: Vec<&str> = text.lines().collect();
        // 路径分隔符随平台（Windows 为反斜杠）。
        let sep = std::path::MAIN_SEPARATOR;
        let a = format!("src{sep}a.rs");
        let b = format!("src{sep}b.rs");
        assert_eq!(lines, vec![a.as_str(), b.as_str()]);
    }

    #[test]
    fn max_results_truncates() {
        let (service, id, root, _ws_dir) = make_service();
        for i in 0..10 {
            fs::write(root.join(format!("f{i}.txt")), "").unwrap();
        }
        let res = find(
            &service,
            &id,
            &json!({"pattern": "*.txt", "max_results": 3}),
            &CancellationToken::new(),
        )
        .expect("find");
        assert!(res.truncated);
        let text = match &res.content[0] {
            ContentPart::Text(t) => &t.text,
            _ => panic!("text"),
        };
        assert_eq!(text.lines().count(), 3);
    }

    #[test]
    fn dir_filter() {
        let (service, id, root, _ws_dir) = make_service();
        fs::create_dir_all(root.join("sub")).unwrap();
        fs::write(root.join("sub/x.rs"), "").unwrap();
        let res = find(
            &service,
            &id,
            &json!({"pattern": "**/*", "file_type": "dir"}),
            &CancellationToken::new(),
        )
        .expect("find");
        let text = match &res.content[0] {
            ContentPart::Text(t) => &t.text,
            _ => panic!("text"),
        };
        assert!(text.contains("sub"));
        assert!(!text.contains("x.rs"));
    }

    #[test]
    fn cancelled_find_stops_before_traversal() {
        let (service, id, _root, _ws_dir) = make_service();
        let cancel = CancellationToken::new();
        cancel.cancel();
        let error = find(&service, &id, &json!({"pattern": "**/*"}), &cancel).unwrap_err();
        assert!(matches!(error, FindFilesError::Cancelled));
    }
}
