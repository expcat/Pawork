//! `search_text` 工具（P4-6）。
//!
//! 固定串/正则匹配、文件过滤(glob)、ignore 规则、结果限制、上下文行、Unicode。

use std::path::Path;
use std::sync::Arc;

use agent_domain::{ContentPart, TextContent, WorkspaceId};
use async_trait::async_trait;
use globset::GlobSet;
use ignore::WalkBuilder;
use regex::Regex;
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

use crate::common::opt_bool;
use crate::common::opt_u64;
use crate::common::require_str;
use crate::common::workspace_roots;
use crate::common::BuiltinToolError;

const DEFAULT_MAX_RESULTS: u64 = 100;
const DEFAULT_CONTEXT_LINES: u64 = 2;
const MAX_OUTPUT_BYTES: u64 = 256 * 1024;

/// `search_text` 工具。
#[derive(Clone)]
pub struct SearchTextTool {
    workspaces: WorkspaceService,
}

impl SearchTextTool {
    pub fn new(workspaces: WorkspaceService) -> Self {
        Self { workspaces }
    }
}

#[async_trait]
impl AgentTool for SearchTextTool {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "search_text".into(),
            description: "Search text (fixed string or regex) across workspace files, respecting ignore rules, with context lines.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "pattern": { "type": "string" },
                    "is_regex": { "type": "boolean" },
                    "glob": { "type": "string" },
                    "max_results": { "type": "integer", "minimum": 1 },
                    "context_lines": { "type": "integer", "minimum": 0 },
                    "case_sensitive": { "type": "boolean" }
                },
                "required": ["pattern"]
            }),
            capability: ToolCapability::ReadOnly,
            read_only: true,
            supports_concurrency: true,
            default_timeout_ms: Some(15_000),
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
            tokio::task::spawn_blocking(move || search(&service, &workspace_id, &input, &cancel))
                .await
                .map_err(|error| ToolError {
                    kind: tool_api::ToolErrorKind::Internal,
                    message: format!("search_text worker failed: {error}"),
                    retryable: false,
                    retry_after_ms: None,
                })?;
        match result {
            Ok(result) => Ok(result),
            Err(SearchTextError::Cancelled) => Err(ToolError::cancelled("search_text cancelled")),
            Err(error) => Err(BuiltinToolError::from(error).into()),
        }
    }
}

fn search(
    service: &WorkspaceService,
    workspace_id: &WorkspaceId,
    input: &Value,
    cancel: &CancellationToken,
) -> Result<ToolResult, SearchTextError> {
    if cancel.is_cancelled() {
        return Err(SearchTextError::Cancelled);
    }
    let pattern = require_str(input, "pattern")?;
    let is_regex = opt_bool(input, "is_regex").unwrap_or(false);
    let glob = crate::common::opt_str(input, "glob");
    let max_results = opt_u64(input, "max_results").unwrap_or(DEFAULT_MAX_RESULTS) as usize;
    let context_lines = opt_u64(input, "context_lines").unwrap_or(DEFAULT_CONTEXT_LINES) as usize;
    let case_sensitive = opt_bool(input, "case_sensitive").unwrap_or(true);

    let roots = workspace_roots(service, workspace_id)?;
    if roots.is_empty() {
        return Err(SearchTextError::Common(BuiltinToolError::Other(
            "workspace has no roots".into(),
        )));
    }

    let glob_set = match glob.as_deref() {
        Some(pattern) if !pattern.is_empty() => Some(build_glob_set(pattern)?),
        _ => None,
    };

    let matcher = build_matcher(&pattern, is_regex, case_sensitive)?;

    let mut matches = Vec::new();
    let mut budget = MAX_OUTPUT_BYTES as usize;
    let mut truncated = false;
    let mut visited = 0usize;

    for root in &roots {
        let walker = WalkBuilder::new(root)
            .hidden(false)
            .ignore(true)
            .git_ignore(true)
            .git_exclude(true)
            .build();
        for entry in walker {
            visited += 1;
            if visited % 64 == 0 && cancel.is_cancelled() {
                return Err(SearchTextError::Cancelled);
            }
            if matches.len() >= max_results {
                truncated = true;
                break;
            }
            let entry = entry?;
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let rel = match path.strip_prefix(root) {
                Ok(rel) => rel,
                Err(_) => continue,
            };
            if let Some(set) = &glob_set {
                if !set.is_match(rel) && !set.is_match(path) {
                    continue;
                }
            }
            if cancel.is_cancelled() {
                return Err(SearchTextError::Cancelled);
            }
            if let Ok(text) = std::fs::read_to_string(path) {
                if let Some(emitted) =
                    scan_file(rel, &text, &matcher, context_lines, &mut budget, cancel)?
                {
                    matches.push(emitted);
                    if matches.len() >= max_results {
                        truncated = true;
                        break;
                    }
                }
            }
        }
        if truncated {
            break;
        }
    }

    let body = matches.join("\n");
    let metadata = json!({
        "pattern": pattern,
        "is_regex": is_regex,
        "matches": matches.len(),
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

/// 构造正则或固定串匹配器（返回共享匹配器）。
enum Matcher {
    Regex(Regex),
    Fixed(String),
}

impl Matcher {
    fn find_at(&self, line: &str) -> Option<usize> {
        match self {
            Matcher::Regex(re) => re.find(line).map(|m| m.start()),
            Matcher::Fixed(needle) => line.find(needle),
        }
    }
}

fn build_matcher(
    pattern: &str,
    is_regex: bool,
    case_sensitive: bool,
) -> Result<Arc<Matcher>, SearchTextError> {
    let matcher = if is_regex {
        let mut builder = regex::RegexBuilder::new(pattern);
        builder.case_insensitive(!case_sensitive);
        Matcher::Regex(
            builder
                .build()
                .map_err(|e| SearchTextError::InvalidInput(format!("invalid regex: {e}")))?,
        )
    } else {
        let needle = if case_sensitive {
            pattern.to_string()
        } else {
            pattern.to_lowercase()
        };
        Matcher::Fixed(needle)
    };
    Ok(Arc::new(matcher))
}

fn build_glob_set(pattern: &str) -> Result<GlobSet, SearchTextError> {
    let mut builder = globset::GlobSetBuilder::new();
    for piece in pattern.split(',') {
        let piece = piece.trim();
        if !piece.is_empty() {
            builder.add(
                globset::Glob::new(piece)
                    .map_err(|e| SearchTextError::InvalidInput(format!("invalid glob: {e}")))?,
            );
        }
    }
    builder
        .build()
        .map_err(|e| SearchTextError::InvalidInput(format!("invalid glob: {e}")))
}

/// 扫描单个文件，返回格式化输出（路径:行号:内容 + 上下文）。
fn scan_file(
    rel: &Path,
    text: &str,
    matcher: &Matcher,
    context_lines: usize,
    budget: &mut usize,
    cancel: &CancellationToken,
) -> Result<Option<String>, SearchTextError> {
    let lines: Vec<&str> = text.lines().collect();
    let rel_str = rel.display().to_string();
    let mut out = String::new();
    let mut matched_any = false;
    for (idx, line) in lines.iter().enumerate() {
        if idx % 256 == 0 && cancel.is_cancelled() {
            return Err(SearchTextError::Cancelled);
        }
        let hay = match matcher {
            Matcher::Fixed(needle) if !needle.is_empty() => line.to_lowercase(),
            _ => line.to_string(),
        };
        // 固定串大小写不敏感时已转小写，比较用小写 needle。
        let hit = match matcher {
            Matcher::Fixed(needle) => {
                if needle.is_empty() {
                    false
                } else {
                    hay.find(needle.as_str()).is_some()
                }
            }
            Matcher::Regex(_) => matcher.find_at(line).is_some(),
        };
        if !hit {
            continue;
        }
        matched_any = true;
        let start = idx.saturating_sub(context_lines);
        let end = (idx + context_lines).min(lines.len().saturating_sub(1));
        for (c, line_text) in lines.iter().enumerate().take(end + 1).skip(start) {
            let marker = if c == idx { "" } else { "  > " };
            let entry = format!("{rel_str}:{}:{}{line}\n", c + 1, marker, line = line_text);
            if entry.len() > *budget {
                return Ok(Some(out));
            }
            *budget -= entry.len();
            out.push_str(&entry);
        }
    }
    Ok(if matched_any { Some(out) } else { None })
}

#[derive(Debug, thiserror::Error)]
pub enum SearchTextError {
    #[error(transparent)]
    Common(#[from] BuiltinToolError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Ignore(#[from] ignore::Error),
    #[error("invalid input: {0}")]
    InvalidInput(String),
    #[error("search cancelled")]
    Cancelled,
}

impl From<SearchTextError> for BuiltinToolError {
    fn from(error: SearchTextError) -> Self {
        match error {
            SearchTextError::Common(common) => common,
            SearchTextError::Io(io) => BuiltinToolError::Io(io),
            SearchTextError::Ignore(e) => BuiltinToolError::Other(e.to_string()),
            SearchTextError::InvalidInput(detail) => BuiltinToolError::InvalidField {
                field: "pattern",
                detail,
            },
            SearchTextError::Cancelled => BuiltinToolError::Other("search_text cancelled".into()),
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
                "pawork-search-{}-{}-{name}-",
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
    fn fixed_string_match_with_context() {
        let (service, id, root, _ws_dir) = make_service();
        fs::write(root.join("a.rs"), "alpha\nbeta\ngamma\nbeta\n").unwrap();
        let res = search(
            &service,
            &id,
            &json!({"pattern": "beta", "context_lines": 1}),
            &CancellationToken::new(),
        )
        .expect("search");
        assert!(res.success);
        let text = match &res.content[0] {
            ContentPart::Text(t) => &t.text,
            _ => panic!("text"),
        };
        assert!(text.contains("a.rs:2:beta"));
        assert!(text.contains("a.rs:1:"));
        assert!(text.contains("a.rs:3:"));
    }

    #[test]
    fn regex_match() {
        let (service, id, root, _ws_dir) = make_service();
        fs::write(root.join("b.txt"), "foo123bar\nqux\n").unwrap();
        let res = search(
            &service,
            &id,
            &json!({"pattern": "[0-9]+", "is_regex": true, "context_lines": 0}),
            &CancellationToken::new(),
        )
        .expect("search");
        let text = match &res.content[0] {
            ContentPart::Text(t) => &t.text,
            _ => panic!("text"),
        };
        assert!(text.contains("b.txt:1:foo123bar"));
        assert!(!text.contains("qux"));
    }

    #[test]
    fn glob_filter_applies() {
        let (service, id, root, _ws_dir) = make_service();
        fs::write(root.join("a.rs"), "target\n").unwrap();
        fs::write(root.join("b.txt"), "target\n").unwrap();
        let res = search(
            &service,
            &id,
            &json!({"pattern": "target", "glob": "*.rs", "context_lines": 0}),
            &CancellationToken::new(),
        )
        .expect("search");
        let text = match &res.content[0] {
            ContentPart::Text(t) => &t.text,
            _ => panic!("text"),
        };
        assert!(text.contains("a.rs"));
        assert!(!text.contains("b.txt"));
    }

    #[test]
    fn invalid_regex_returns_invalid_input() {
        let (service, id, _root, _ws_dir) = make_service();
        let err = search(
            &service,
            &id,
            &json!({"pattern": "[", "is_regex": true}),
            &CancellationToken::new(),
        )
        .unwrap_err();
        let error: ToolError = BuiltinToolError::from(err).into();
        assert_eq!(error.kind, tool_api::ToolErrorKind::InvalidInput);
    }

    #[test]
    fn cancelled_search_stops_before_traversal() {
        let (service, id, _root, _ws_dir) = make_service();
        let cancel = CancellationToken::new();
        cancel.cancel();
        let error = search(&service, &id, &json!({"pattern": "x"}), &cancel).unwrap_err();
        assert!(matches!(error, SearchTextError::Cancelled));
    }
}
