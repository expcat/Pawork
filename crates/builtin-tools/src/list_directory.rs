//! `list_directory` 工具（P4-8）。
//!
//! 类型/大小/mtime 输出、symlink 信息、分页。

use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::time::UNIX_EPOCH;

use agent_domain::{ContentPart, TextContent, WorkspaceId};
use async_trait::async_trait;
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

use crate::common::opt_u64;
use crate::common::require_str;
use crate::common::resolve_rel;
use crate::common::workspace_roots;
use crate::common::BuiltinToolError;

const DEFAULT_LIMIT: u64 = 500;

#[derive(Clone, Debug, Serialize)]
struct Entry {
    name: String,
    kind: String,
    size: u64,
    mtime_ms: u64,
    is_symlink: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    symlink_target: Option<String>,
}

#[derive(Clone, Debug)]
struct RankedEntry(Entry);

impl PartialEq for RankedEntry {
    fn eq(&self, other: &Self) -> bool {
        entry_cmp(&self.0, &other.0) == Ordering::Equal
    }
}

impl Eq for RankedEntry {}

impl PartialOrd for RankedEntry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for RankedEntry {
    fn cmp(&self, other: &Self) -> Ordering {
        entry_cmp(&self.0, &other.0)
    }
}

/// `list_directory` 工具。
#[derive(Clone)]
pub struct ListDirectoryTool {
    workspaces: WorkspaceService,
}

impl ListDirectoryTool {
    pub fn new(workspaces: WorkspaceService) -> Self {
        Self { workspaces }
    }
}

#[async_trait]
impl AgentTool for ListDirectoryTool {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "list_directory".into(),
            description:
                "List directory entries with type, size, mtime and symlink info, with pagination."
                    .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "limit": { "type": "integer", "minimum": 1 },
                    "offset": { "type": "integer", "minimum": 0 }
                },
                "required": ["path"]
            }),
            capability: ToolCapability::ReadOnly,
            read_only: true,
            supports_concurrency: true,
            default_timeout_ms: Some(10_000),
            max_output_bytes: 128 * 1024,
            allowed_in_untrusted_workspace: true,
        }
    }

    async fn execute(
        &self,
        request: ToolRequest,
        context: ToolExecutionContext,
        _sink: &dyn ToolEventSink,
        _cancel: CancellationToken,
    ) -> Result<ToolResult, ToolError> {
        match list_dir(&self.workspaces, &context.workspace_id, &request.input) {
            Ok(result) => Ok(result),
            Err(error) => Err(BuiltinToolError::from(error).into()),
        }
    }
}

fn list_dir(
    service: &WorkspaceService,
    workspace_id: &WorkspaceId,
    input: &Value,
) -> Result<ToolResult, ListDirError> {
    let path = require_str(input, "path")?;
    let limit = opt_u64(input, "limit").unwrap_or(DEFAULT_LIMIT).max(1) as usize;
    let offset = opt_u64(input, "offset").unwrap_or(0) as usize;

    let roots = workspace_roots(service, workspace_id)?;
    let absolute = resolve_rel(&roots, &path)?;
    if !absolute.is_dir() {
        return Err(ListDirError::Common(BuiltinToolError::Other(format!(
            "{path} is not a directory"
        ))));
    }

    // 只保留当前页结束位置之前的最小项；仍单次扫描得到准确 total，
    // 但大目录的内存从 O(total) 收敛为 O(offset + limit)。
    let keep = offset.saturating_add(limit);
    let mut candidates = BinaryHeap::with_capacity(keep.min(4096));
    let mut total = 0usize;
    for entry in std::fs::read_dir(&absolute)? {
        let entry = entry?;
        total += 1;
        let lmeta = std::fs::symlink_metadata(entry.path())?;
        let is_symlink = lmeta.file_type().is_symlink();
        let symlink_target = if is_symlink {
            std::fs::read_link(entry.path())
                .map(|p| p.display().to_string())
                .ok()
        } else {
            None
        };
        let followed = if is_symlink {
            std::fs::metadata(entry.path()).ok()
        } else {
            None
        };
        let (kind, meta) = if is_symlink {
            match followed {
                Some(meta) => ("symlink".to_string(), meta),
                None => ("broken_symlink".to_string(), lmeta),
            }
        } else if lmeta.is_dir() {
            ("dir".to_string(), lmeta)
        } else {
            ("file".to_string(), lmeta)
        };
        let mtime_ms = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let item = RankedEntry(Entry {
            name: entry.file_name().to_string_lossy().into_owned(),
            kind,
            size: meta.len(),
            mtime_ms,
            is_symlink,
            symlink_target,
        });
        if keep == 0 {
            continue;
        }
        if candidates.len() < keep {
            candidates.push(item);
        } else if candidates
            .peek()
            .is_some_and(|worst| entry_cmp(&item.0, &worst.0).is_lt())
        {
            candidates.pop();
            candidates.push(item);
        }
    }

    // 稳定排序：目录优先，再按名字字典序。
    let mut entries: Vec<Entry> = candidates.into_iter().map(|item| item.0).collect();
    entries.sort_by(entry_cmp);
    let truncated = keep < total;
    let page: Vec<Entry> = entries.into_iter().skip(offset).take(limit).collect();

    let lines: Vec<String> = page
        .iter()
        .map(|e| {
            let target = e
                .symlink_target
                .as_ref()
                .map(|t| format!(" -> {t}"))
                .unwrap_or_default();
            format!("{:>8}  {}  {}{}", e.size, e.kind, e.name, target)
        })
        .collect();

    let metadata = json!({
        "path": path,
        "total": total,
        "offset": offset,
        "limit": limit,
        "truncated": truncated,
        "entries": serde_json::to_value(&page).unwrap_or(Value::Null),
    });
    Ok(ToolResult {
        content: vec![ContentPart::Text(TextContent {
            text: lines.join("\n"),
        })],
        artifacts: Vec::new(),
        metadata,
        truncated,
        success: true,
        error: None,
    })
}

fn dir_rank(kind: &str) -> u8 {
    match kind {
        "dir" => 0,
        _ => 1,
    }
}

fn entry_cmp(left: &Entry, right: &Entry) -> Ordering {
    dir_rank(&left.kind)
        .cmp(&dir_rank(&right.kind))
        .then_with(|| left.name.cmp(&right.name))
        .then_with(|| left.kind.cmp(&right.kind))
}

#[derive(Debug, thiserror::Error)]
pub enum ListDirError {
    #[error(transparent)]
    Common(#[from] BuiltinToolError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

impl From<ListDirError> for BuiltinToolError {
    fn from(error: ListDirError) -> Self {
        match error {
            ListDirError::Common(common) => common,
            ListDirError::Io(io) => BuiltinToolError::Io(io),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_domain::Timestamp;
    use agent_domain::WorkspaceId;
    use std::fs;
    #[cfg(unix)]
    use std::os::unix::fs::symlink;
    use std::sync::atomic::AtomicU64;
    use std::sync::atomic::Ordering;

    static NEXT: AtomicU64 = AtomicU64::new(1);

    fn temp_root(name: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "pawork-listdir-{}-{}-{name}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).expect("mkdir");
        path
    }

    fn make_service() -> (WorkspaceService, WorkspaceId, std::path::PathBuf) {
        let root = temp_root("ws");
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
        (service, id, root)
    }

    #[test]
    fn lists_entries_with_type_and_symlink() {
        let (service, id, root) = make_service();
        fs::create_dir_all(root.join("sub")).unwrap();
        fs::write(root.join("a.txt"), b"hello").unwrap();
        #[cfg(unix)]
        symlink(root.join("a.txt"), root.join("link.txt")).unwrap();

        let res = list_dir(&service, &id, &json!({"path": "."})).expect("list");
        let text = match &res.content[0] {
            ContentPart::Text(t) => &t.text,
            _ => panic!("text"),
        };
        // 目录排在文件前。
        let lines: Vec<&str> = text.lines().collect();
        assert!(lines[0].contains("sub"));
        assert!(text.contains("a.txt"));
        // symlink 仅在 unix 测试中创建（见上）。
        #[cfg(unix)]
        {
            assert!(text.contains("symlink"));
            assert!(text.contains("link.txt -> "));
        }
    }

    #[test]
    fn pagination_works() {
        let (service, id, root) = make_service();
        for i in 0..5 {
            fs::write(root.join(format!("f{i}")), "").unwrap();
        }
        let res = list_dir(
            &service,
            &id,
            &json!({"path": ".", "offset": 0, "limit": 2}),
        )
        .expect("list");
        assert!(res.truncated);
        let text = match &res.content[0] {
            ContentPart::Text(t) => &t.text,
            _ => panic!("text"),
        };
        assert_eq!(text.lines().count(), 2);
        assert_eq!(res.metadata["total"], 5);
    }

    #[cfg(unix)]
    #[test]
    fn dangling_symlink_is_reported_without_failing_listing() {
        let (service, id, root) = make_service();
        symlink("missing-target", root.join("broken-link")).unwrap();
        let res = list_dir(&service, &id, &json!({"path": "."})).expect("list");
        assert_eq!(res.metadata["total"], 1);
        assert_eq!(res.metadata["entries"][0]["kind"], "broken_symlink");
        assert_eq!(res.metadata["entries"][0]["is_symlink"], true);
    }

    #[test]
    fn non_directory_path_errors() {
        let (service, id, root) = make_service();
        fs::write(root.join("file.txt"), "x").unwrap();
        let err = list_dir(&service, &id, &json!({"path": "file.txt"})).unwrap_err();
        assert!(matches!(
            err,
            ListDirError::Common(BuiltinToolError::Other(_))
        ));
    }
}
