//! `list_directory` 工具。
//!
//! 类型/大小/mtime 输出、symlink 信息、分页。

use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use pawork_api::AgentTool;
use pawork_api::ToolError;
use pawork_api::ToolEventSink;
use pawork_api::ToolExecutionContext;
use pawork_api::ToolRequest;
use pawork_api::ToolResult;
use pawork_domain::{
    CancellationToken, ContentPart, TextContent, ToolCapability, ToolDescriptor, ToolHosting,
    ToolKind, WorkspaceId,
};
use async_trait::async_trait;
use serde::Serialize;
use serde_json::{json, Value};
use pawork_workspace::WorkspaceService;

use crate::common::opt_u64;
use crate::common::require_str;
use crate::common::resolve_write_rel;
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
            kind: ToolKind::ClientFunction,
            hosting: ToolHosting::Local,
            capabilities: Vec::new(),
            requires_approval: false,
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
    let absolute = resolve_write_rel(&roots, &path)?;
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
            safe_symlink_target(&entry.path(), &roots)
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

/// 将 symlink 目标相对化到某个 workspace root；越 root 则省略，避免回传宿主绝对路径。
fn safe_symlink_target(entry_path: &Path, roots: &[PathBuf]) -> Option<String> {
    let raw = std::fs::read_link(entry_path).ok()?;
    let parent = entry_path.parent().unwrap_or(entry_path);
    let joined = if raw.is_absolute() {
        raw.clone()
    } else {
        parent.join(&raw)
    };
    match pawork_policy::canonicalize_platform(&joined) {
        Ok(canon) => {
            for root in roots {
                let Ok(canon_root) = pawork_policy::canonicalize_platform(root) else {
                    continue;
                };
                if let Some(rel) = pawork_policy::relative_to_root(&canon, &canon_root) {
                    return Some(rel.to_string_lossy().replace('\\', "/"));
                }
            }
            None
        }
        Err(_) => {
            if raw.is_absolute() {
                None
            } else {
                Some(raw.to_string_lossy().replace('\\', "/"))
            }
        }
    }
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
    use pawork_domain::WorkspaceId;
    use std::fs;
    #[cfg(unix)]
    use std::os::unix::fs::symlink;
    use std::sync::atomic::AtomicU64;
    use std::sync::atomic::Ordering;

    static NEXT: AtomicU64 = AtomicU64::new(1);

    fn temp_root(name: &str) -> tempfile::TempDir {
        tempfile::Builder::new()
            .prefix(&format!(
                "pawork-listdir-{}-{}-{name}-",
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
    fn lists_entries_with_type_and_symlink() {
        let (service, id, root, _ws_dir) = make_service();
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
            assert!(text.contains("link.txt -> a.txt"), "{text}");
            assert!(!text.contains(&root.display().to_string()), "{text}");
        }
        assert!(res.metadata.get("absolute").is_none());
    }

    #[test]
    fn pagination_works() {
        let (service, id, root, _ws_dir) = make_service();
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
        let (service, id, root, _ws_dir) = make_service();
        symlink("missing-target", root.join("broken-link")).unwrap();
        let res = list_dir(&service, &id, &json!({"path": "."})).expect("list");
        assert_eq!(res.metadata["total"], 1);
        assert_eq!(res.metadata["entries"][0]["kind"], "broken_symlink");
        assert_eq!(res.metadata["entries"][0]["is_symlink"], true);
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlink_escape_dir_and_redacts_host_absolute_target() {
        let (service, id, root, _ws_dir) = make_service();
        std::os::unix::fs::symlink("/etc", root.join("etc-link")).unwrap();
        std::os::unix::fs::symlink("/etc/passwd", root.join("passwd-link")).unwrap();

        let err = list_dir(&service, &id, &json!({"path": "etc-link"})).unwrap_err();
        assert!(
            matches!(
                err,
                ListDirError::Common(BuiltinToolError::PolicyPath(
                    pawork_policy::PathSafetyError::SymlinkEscape
                ))
            ),
            "{err:?}"
        );

        let res = list_dir(&service, &id, &json!({"path": "."})).expect("list");
        let text = match &res.content[0] {
            ContentPart::Text(t) => t.text.as_str(),
            _ => panic!("text"),
        };
        let meta = res.metadata.to_string();
        assert!(!text.contains("/etc/passwd"), "body leaked host path: {text}");
        assert!(!meta.contains("/etc/passwd"), "json leaked host path: {meta}");
        assert!(!text.contains("/etc"), "body leaked host dir: {text}");
        assert!(
            !meta.contains("/etc"),
            "json leaked host dir: {meta}"
        );
    }

    #[test]
    fn non_directory_path_errors() {
        let (service, id, root, _ws_dir) = make_service();
        fs::write(root.join("file.txt"), "x").unwrap();
        let err = list_dir(&service, &id, &json!({"path": "file.txt"})).unwrap_err();
        assert!(matches!(
            err,
            ListDirError::Common(BuiltinToolError::Other(_))
        ));
    }
}
