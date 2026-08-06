//! `edit_file` 工具（P4-3）。
//!
//! 精确替换、多段编辑、上下文校验、模糊匹配、冲突报告（结构化 diff）。

use std::fs;
use std::io::Write;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;

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

use crate::common::call_key;
use crate::common::opt_bool;
use crate::common::require_str;
use crate::common::resolve_rel;
use crate::common::workspace_roots;
use crate::common::BuiltinToolError;

/// `edit_file` 工具。
#[derive(Clone)]
pub struct EditFileTool {
    workspaces: WorkspaceService,
    checkpoints: CheckpointService,
}

impl EditFileTool {
    pub fn new(workspaces: WorkspaceService, checkpoints: CheckpointService) -> Self {
        Self {
            workspaces,
            checkpoints,
        }
    }
}

#[async_trait]
impl AgentTool for EditFileTool {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "edit_file".into(),
            description: "Edit a workspace-relative file via precise replacements (single or multi-segment), with fuzzy whitespace matching and structured conflict reports.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "old_string": { "type": "string" },
                    "new_string": { "type": "string" },
                    "allow_fuzzy": { "type": "boolean" },
                    "edits": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "old_string": { "type": "string" },
                                "new_string": { "type": "string" }
                            },
                            "required": ["old_string", "new_string"]
                        }
                    }
                },
                "required": ["path"]
            }),
            capability: ToolCapability::WorkspaceWrite,
            read_only: false,
            supports_concurrency: false,
            default_timeout_ms: Some(10_000),
            max_output_bytes: 32 * 1024,
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
        match edit(
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

#[derive(Clone, Debug, Serialize)]
struct AppliedEdit {
    old_string: String,
    new_string: String,
    occurrences: usize,
}

#[derive(Clone, Debug, Serialize)]
struct EditReport {
    path: String,
    applied: Vec<AppliedEdit>,
    bytes: usize,
}

#[derive(Debug, thiserror::Error)]
pub enum EditFileError {
    #[error(transparent)]
    Common(#[from] BuiltinToolError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("checkpoint error: {0}")]
    Checkpoint(String),
    #[error("edit conflict: {0}")]
    Conflict(String),
    #[error("old_string not found in {path}")]
    NotFound { path: String },
}

impl From<EditFileError> for BuiltinToolError {
    fn from(error: EditFileError) -> Self {
        match error {
            EditFileError::Common(c) => c,
            EditFileError::Io(io) => BuiltinToolError::Io(io),
            EditFileError::Checkpoint(m) => BuiltinToolError::Checkpoint(m),
            EditFileError::Conflict(m) => BuiltinToolError::Other(m),
            EditFileError::NotFound { .. } => BuiltinToolError::Other(error.to_string()),
        }
    }
}

async fn edit(
    service: &WorkspaceService,
    checkpoints: &CheckpointService,
    workspace_id: &WorkspaceId,
    run_id: &agent_domain::RunId,
    tool_call_id: &agent_domain::ToolCallId,
    input: &Value,
) -> Result<ToolResult, EditFileError> {
    let path = require_str(input, "path")?;
    let allow_fuzzy = opt_bool(input, "allow_fuzzy").unwrap_or(false);

    let mut segments: Vec<(String, String)> = Vec::new();
    if let Some(arr) = input.get("edits").and_then(|v| v.as_array()) {
        for item in arr {
            segments.push((
                item.get("old_string")
                    .and_then(|v| v.as_str())
                    .ok_or(BuiltinToolError::MissingField("old_string"))?
                    .to_string(),
                item.get("new_string")
                    .and_then(|v| v.as_str())
                    .ok_or(BuiltinToolError::MissingField("new_string"))?
                    .to_string(),
            ));
        }
    } else {
        segments.push((
            require_str(input, "old_string")?,
            require_str(input, "new_string")?,
        ));
    }

    if segments.is_empty() {
        return Err(EditFileError::Common(BuiltinToolError::Other(
            "no edits provided".into(),
        )));
    }
    for (old, new) in &segments {
        if old == new {
            return Err(EditFileError::Conflict(format!(
                "old_string equals new_string in {path}"
            )));
        }
    }

    let roots = workspace_roots(service, workspace_id)?;
    let absolute = resolve_rel(&roots, &path)?;
    let original = fs::read_to_string(&absolute).map_err(EditFileError::Io)?;

    // 全部替换先在内存中预演；任一段冲突则不落盘（原子）。
    let mut content = original.clone();
    let mut applied = Vec::new();
    for (old, new) in &segments {
        let count = count_and_replace(&mut content, old, new, allow_fuzzy, &path)?;
        applied.push(AppliedEdit {
            old_string: old.clone(),
            new_string: new.clone(),
            occurrences: count,
        });
    }

    if content == original {
        return Err(EditFileError::NotFound { path: path.clone() });
    }

    // 写入前 checkpoint（一次）。
    checkpoints
        .snapshot_before_write(run_id.as_ref(), &call_key(tool_call_id), &roots, &path)
        .await
        .map_err(|e| EditFileError::Checkpoint(e.to_string()))?;

    atomic_write(&absolute, content.as_bytes())?;

    let report = EditReport {
        path: path.clone(),
        applied,
        bytes: content.len(),
    };
    let metadata = serde_json::to_value(&report).unwrap_or(Value::Null);
    Ok(ToolResult {
        content: vec![ContentPart::Text(TextContent {
            text: format!("edited {path}: {} segment(s)", report.applied.len()),
        })],
        artifacts: Vec::new(),
        metadata,
        truncated: false,
        success: true,
        error: None,
    })
}

/// 统计并执行单段替换，返回匹配次数；0 次匹配视为 NotFound，多次（不唯一）视为冲突。
fn count_and_replace(
    content: &mut String,
    old: &str,
    new: &str,
    allow_fuzzy: bool,
    path: &str,
) -> Result<usize, EditFileError> {
    let occurrences = if allow_fuzzy {
        count_fuzzy(content, old)
    } else {
        content.matches(old).count()
    };
    if occurrences == 0 {
        // 未匹配：该段无法应用；多段原子语义要求整体失败（尚未写盘）。
        return Err(EditFileError::NotFound {
            path: path.to_string(),
        });
    }
    if occurrences > 1 {
        return Err(EditFileError::Conflict(format!(
            "old_string is not unique: found {occurrences} occurrences"
        )));
    }
    if allow_fuzzy {
        replace_fuzzy(content, old, new);
    } else {
        *content = content.replacen(old, new, 1);
    }
    Ok(1)
}

/// 规范化空白：压缩连续空白、去除首尾空白，便于模糊匹配。
fn normalize_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// 模糊统计：按行块匹配，规范化后比较是否相等，返回匹配窗口数。
fn count_fuzzy(content: &str, old: &str) -> usize {
    let norm_old = normalize_ws(old);
    if norm_old.is_empty() {
        return 0;
    }
    let lines: Vec<&str> = content.lines().collect();
    let old_lines: Vec<&str> = old.lines().collect();
    let n = old_lines.len();
    if n == 0 || lines.len() < n {
        return 0;
    }
    let mut count = 0;
    for window in lines.windows(n) {
        let joined = window.join("\n");
        if normalize_ws(&joined) == norm_old {
            count += 1;
        }
    }
    count
}

fn replace_fuzzy(content: &mut String, old: &str, new: &str) {
    let lines: Vec<&str> = content.lines().collect();
    let old_lines: Vec<&str> = old.lines().collect();
    let n = old_lines.len();
    let norm_old = normalize_ws(old);
    if n == 0 {
        return;
    }
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        if i + n <= lines.len() && normalize_ws(&lines[i..i + n].join("\n")) == norm_old {
            out.push(new.to_string());
            i += n;
        } else {
            out.push(lines[i].to_string());
            i += 1;
        }
    }
    *content = out.join("\n");
}

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

fn atomic_write(path: &std::path::Path, content: &[u8]) -> Result<(), EditFileError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_file_name(format!(
        ".tmp-{}-{}",
        std::process::id(),
        TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| -> std::io::Result<()> {
        let mut file = fs::File::create(&tmp)?;
        file.write_all(content)?;
        file.sync_all()?;
        fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result.map_err(EditFileError::Io)
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_domain::Timestamp;
    use agent_domain::WorkspaceId;
    use artifact_store::ArtifactStore;
    use std::sync::atomic::AtomicU64;

    static NEXT: AtomicU64 = AtomicU64::new(1);

    fn temp_root(name: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "pawork-editfile-{}-{}-{name}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).expect("mkdir");
        path
    }

    async fn make_env() -> (
        WorkspaceService,
        CheckpointService,
        WorkspaceId,
        std::path::PathBuf,
    ) {
        let root = temp_root("ws");
        let store_root = temp_root("store");
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
        (service, checkpoints, id, root)
    }

    fn single_edit(path: &str, old: &str, new: &str) -> Value {
        json!({"path": path, "old_string": old, "new_string": new})
    }

    #[tokio::test]
    async fn precise_single_replacement() {
        let (service, checkpoints, id, root) = make_env().await;
        fs::write(root.join("a.txt"), "alpha\nbeta\n").unwrap();
        let rid = agent_domain::RunId::from("r1");
        let tid = agent_domain::ToolCallId::from("t1");
        edit(
            &service,
            &checkpoints,
            &id,
            &rid,
            &tid,
            &single_edit("a.txt", "alpha", "ALPHA"),
        )
        .await
        .expect("edit");
        assert_eq!(
            fs::read_to_string(root.join("a.txt")).unwrap(),
            "ALPHA\nbeta\n"
        );
    }

    #[tokio::test]
    async fn non_unique_reports_conflict() {
        let (service, checkpoints, id, root) = make_env().await;
        fs::write(root.join("a.txt"), "dup\ndup\n").unwrap();
        let rid = agent_domain::RunId::from("r1");
        let tid = agent_domain::ToolCallId::from("t1");
        let err = edit(
            &service,
            &checkpoints,
            &id,
            &rid,
            &tid,
            &single_edit("a.txt", "dup", "x"),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, EditFileError::Conflict(_)));
        // 未落盘。
        assert_eq!(
            fs::read_to_string(root.join("a.txt")).unwrap(),
            "dup\ndup\n"
        );
    }

    #[tokio::test]
    async fn multi_segment_atomic() {
        let (service, checkpoints, id, root) = make_env().await;
        fs::write(root.join("a.txt"), "foo\nbar\nbaz\n").unwrap();
        let rid = agent_domain::RunId::from("r1");
        let tid = agent_domain::ToolCallId::from("t1");
        let input = json!({
        "path": "a.txt",
        "edits": [
        {"old_string": "foo", "new_string": "FOO"},
        {"old_string": "bar", "new_string": "BAR"}
        ]
        });
        edit(&service, &checkpoints, &id, &rid, &tid, &input)
            .await
            .expect("edit");
        assert_eq!(
            fs::read_to_string(root.join("a.txt")).unwrap(),
            "FOO\nBAR\nbaz\n"
        );
    }

    #[tokio::test]
    async fn multi_segment_partial_failure_rolls_back() {
        let (service, checkpoints, id, root) = make_env().await;
        fs::write(root.join("a.txt"), "foo\nbar\n").unwrap();
        let rid = agent_domain::RunId::from("r1");
        let tid = agent_domain::ToolCallId::from("t1");
        let input = json!({
        "path": "a.txt",
        "edits": [
        {"old_string": "foo", "new_string": "FOO"},
        {"old_string": "missing", "new_string": "X"}
        ]
        });
        let err = edit(&service, &checkpoints, &id, &rid, &tid, &input)
            .await
            .unwrap_err();
        // 第二段未找到 -> 整体不落盘（预演失败，未写文件）。
        assert!(matches!(err, EditFileError::NotFound { .. }));
        assert_eq!(
            fs::read_to_string(root.join("a.txt")).unwrap(),
            "foo\nbar\n"
        );
    }

    #[tokio::test]
    async fn fuzzy_match_normalizes_whitespace() {
        let (service, checkpoints, id, root) = make_env().await;
        fs::write(root.join("a.txt"), "  alpha    beta  \n").unwrap();
        let rid = agent_domain::RunId::from("r1");
        let tid = agent_domain::ToolCallId::from("t1");
        edit(
 &service,
 &checkpoints,
 &id,
 &rid,
 &tid,
 &json!({"path": "a.txt", "old_string": "alpha beta", "new_string": "REPLACED", "allow_fuzzy": true}),
        )
        .await
        .expect("fuzzy edit");
        assert!(fs::read_to_string(root.join("a.txt"))
            .unwrap()
            .contains("REPLACED"));
    }
}
