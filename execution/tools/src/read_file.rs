//! `read_file` 工具。
//!
//! 只读读取工作区相对文件：行号、offset/limit、编码检测与二进制检测；
//! 路径基于 `workspace_id + relative_path`，经 policy 内核校验。

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
use chardetng::EncodingDetector;
use serde_json::{json, Value};
use tokio::io::AsyncReadExt;
use pawork_workspace::WorkspaceService;

use crate::common::opt_u64;
use crate::common::require_str;
use crate::common::resolve_write_rel;
use crate::common::workspace_roots;
use crate::common::BuiltinToolError;

const DEFAULT_LIMIT: u64 = 2000;
const MAX_OUTPUT_BYTES: u64 = 256 * 1024;
const MAX_READ_BYTES: u64 = 4 * 1024 * 1024;

/// `read_file` 工具。
#[derive(Clone)]
pub struct ReadFileTool {
    workspaces: WorkspaceService,
}

impl ReadFileTool {
    pub fn new(workspaces: WorkspaceService) -> Self {
        Self { workspaces }
    }
}

#[async_trait]
impl AgentTool for ReadFileTool {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "read_file".into(),
            description: "Read a workspace-relative file with line numbers, offset/limit, encoding and binary detection.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "offset": { "type": "integer", "minimum": 1 },
                    "limit": { "type": "integer", "minimum": 1 }
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
        match read(
            &self.workspaces,
            &context.workspace_id,
            &request.input,
            cancel,
        )
        .await
        {
            Ok(output) => Ok(output),
            Err(ReadFileError::Cancelled) => Err(ToolError::cancelled("read_file cancelled")),
            Err(error) => Err(BuiltinToolError::from(error).into()),
        }
    }
}

async fn read(
    service: &WorkspaceService,
    workspace_id: &WorkspaceId,
    input: &Value,
    cancel: CancellationToken,
) -> Result<ToolResult, ReadFileError> {
    let path = require_str(input, "path")?;
    let offset = opt_u64(input, "offset").unwrap_or(1).max(1) as usize;
    let limit = opt_u64(input, "limit").unwrap_or(DEFAULT_LIMIT).max(1) as usize;

    let roots = workspace_roots(service, workspace_id)?;
    let absolute = resolve_write_rel(&roots, &path)?;
    let metadata = tokio::fs::metadata(&absolute).await?;
    let file = tokio::fs::File::open(&absolute).await?;
    let mut limited = file.take(MAX_READ_BYTES + 1);
    let mut bytes = Vec::with_capacity(
        metadata
            .len()
            .min(MAX_READ_BYTES + 1)
            .try_into()
            .unwrap_or(0),
    );
    tokio::select! {
        _ = cancel.cancelled() => return Err(ReadFileError::Cancelled),
        result = limited.read_to_end(&mut bytes) => { result?; }
    }
    let read_truncated = bytes.len() as u64 > MAX_READ_BYTES;
    if read_truncated {
        bytes.truncate(MAX_READ_BYTES as usize);
    }

    if is_binary(&bytes) {
        return Ok(binary_result(&path, metadata.len()));
    }

    let text = decode(&bytes);
    let total_lines = text.lines().count();
    let (rendered, output_truncated) = render_lines(&text, offset, limit);
    let truncated = read_truncated || output_truncated;

    let metadata = json!({
        "path": path,
        "bytes": metadata.len(),
        "bytes_read": bytes.len(),
        "read_truncated": read_truncated,
        "lines_total": total_lines,
        "offset": offset,
        "limit": limit,
        "binary": false,
    });
    Ok(ToolResult {
        content: vec![ContentPart::Text(TextContent { text: rendered })],
        artifacts: Vec::new(),
        metadata,
        truncated,
        success: true,
        error: None,
    })
}

/// 输出带行号的行片段，返回 (正文, 是否被行数或字节上限截断)。
fn render_lines(text: &str, offset: usize, limit: usize) -> (String, bool) {
    let mut out = String::new();
    let mut budget = MAX_OUTPUT_BYTES as usize;
    let mut truncated = false;
    for (idx, line) in text.lines().enumerate() {
        let line_no = idx + 1;
        if line_no < offset {
            continue;
        }
        if line_no >= offset.saturating_add(limit) {
            truncated = true;
            break;
        }
        let entry = format!("{line_no:>6}\t{line}\n");
        if entry.len() > budget {
            truncated = true;
            break;
        }
        budget -= entry.len();
        out.push_str(&entry);
    }
    (out, truncated)
}

/// 二进制检测：NUL 字节或大量非文本控制字节即判定为二进制。
fn is_binary(bytes: &[u8]) -> bool {
    if bytes.is_empty() {
        return false;
    }
    if bytes.contains(&0u8) {
        return true;
    }
    let mut textish = 0usize;
    let mut ctrl = 0usize;
    let sample = bytes.len().min(1024);
    for &b in &bytes[..sample] {
        if b == 0x09 || b == 0x0a || b == 0x0d || (0x20..=0x7e).contains(&b) || b >= 0x80 {
            textish += 1;
        } else {
            ctrl += 1;
        }
    }
    ctrl > 0 && ctrl * 10 > textish
}

/// 检测编码并解码为 UTF-8（损失式兜底并标注）。
fn decode(bytes: &[u8]) -> String {
    let mut detector = EncodingDetector::new();
    detector.feed(bytes, true);
    let encoding = detector.guess(None, true);
    let (cow, _used, had_errors) = encoding.decode(bytes);
    let mut text = cow.into_owned();
    if had_errors {
        text.push_str("\n\n[warning: contained bytes that were not valid in detected encoding]\n");
    }
    text
}

fn binary_result(path: &str, size: u64) -> ToolResult {
    let metadata = json!({
        "path": path,
        "bytes": size,
        "binary": true,
    });
    let text = format!(
        "{path}: binary file ({size} bytes); content omitted.\nUse a dedicated tool to inspect binary content."
    );
    ToolResult {
        content: vec![ContentPart::Text(TextContent { text })],
        artifacts: Vec::new(),
        metadata,
        truncated: false,
        success: true,
        error: None,
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ReadFileError {
    #[error(transparent)]
    Common(#[from] BuiltinToolError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("read cancelled")]
    Cancelled,
}

impl From<ReadFileError> for BuiltinToolError {
    fn from(error: ReadFileError) -> Self {
        match error {
            ReadFileError::Common(common) => common,
            ReadFileError::Io(io) => BuiltinToolError::Io(io),
            ReadFileError::Cancelled => BuiltinToolError::Other("read_file cancelled".into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pawork_api::ToolErrorKind;
    use pawork_domain::WorkspaceId;
    use pawork_policy::PathSafetyError;
    use std::fs;
    use std::sync::atomic::AtomicU64;
    use std::sync::atomic::Ordering;

    static NEXT: AtomicU64 = AtomicU64::new(1);

    fn temp_root(name: &str) -> tempfile::TempDir {
        tempfile::Builder::new()
            .prefix(&format!(
                "pawork-readfile-{}-{}-{name}-",
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

    async fn run_read(service: &WorkspaceService, id: &WorkspaceId, input: Value) -> ToolResult {
        read(service, id, &input, CancellationToken::new())
            .await
            .expect("read ok")
    }

    fn assert_no_host_absolute(res: &ToolResult, root: &std::path::Path) {
        let meta = res.metadata.to_string();
        let body = text_of(res);
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

    #[tokio::test]
    async fn line_numbers_offset_and_limit() {
        let (service, id, root, _ws_dir) = make_service();
        fs::write(root.join("a.txt"), "one\ntwo\nthree\nfour\nfive\n").unwrap();
        let res = run_read(
            &service,
            &id,
            json!({"path": "a.txt", "offset": 2, "limit": 2}),
        )
        .await;
        assert!(res.success);
        assert!(res.truncated);
        let text = text_of(&res);
        assert!(text.contains("     2\ttwo"));
        assert!(text.contains("     3\tthree"));
        assert!(!text.contains("four"));
        assert_no_host_absolute(&res, &root);
    }

    #[tokio::test]
    async fn binary_file_is_detected_and_omitted() {
        let (service, id, root, _ws_dir) = make_service();
        let mut bytes = vec![0u8; 64];
        bytes[0] = 1;
        fs::write(root.join("bin.dat"), &bytes).unwrap();
        let res = run_read(&service, &id, json!({"path": "bin.dat"})).await;
        assert_eq!(res.metadata["binary"], true);
        assert!(text_of(&res).contains("binary file"));
        assert_no_host_absolute(&res, &root);
    }

    #[tokio::test]
    async fn rejects_absolute_and_traversal_paths() {
        let (service, id, root, _ws_dir) = make_service();
        fs::write(root.join("ok.txt"), "hi").unwrap();
        let abs = root.join("ok.txt");
        let err = read(
            &service,
            &id,
            &json!({"path": abs.display().to_string()}),
            CancellationToken::new(),
        )
        .await
        .unwrap_err();
        assert!(matches!(
            err,
            ReadFileError::Common(BuiltinToolError::PolicyPath(PathSafetyError::AbsolutePath))
        ));
        let error: ToolError = BuiltinToolError::from(err).into();
        assert_eq!(error.kind, ToolErrorKind::PermissionDenied);

        let err = read(
            &service,
            &id,
            &json!({"path": "../escape.txt"}),
            CancellationToken::new(),
        )
        .await
        .unwrap_err();
        assert!(matches!(
            err,
            ReadFileError::Common(BuiltinToolError::PolicyPath(PathSafetyError::Traversal(_)))
        ));
        let error: ToolError = BuiltinToolError::from(err).into();
        assert_eq!(error.kind, ToolErrorKind::PermissionDenied);
    }

    #[tokio::test]
    async fn missing_file_returns_not_found_kind() {
        let (service, id, _root, _ws_dir) = make_service();
        let error: ToolError = BuiltinToolError::from(
            read(
                &service,
                &id,
                &json!({"path": "nope.txt"}),
                CancellationToken::new(),
            )
            .await
            .unwrap_err(),
        )
        .into();
        assert_eq!(error.kind, ToolErrorKind::NotFound);
    }

    #[tokio::test]
    async fn large_reads_are_bounded() {
        let (service, id, root, _ws_dir) = make_service();
        fs::write(
            root.join("large.txt"),
            vec![b'a'; MAX_READ_BYTES as usize + 1024],
        )
        .unwrap();
        let result = run_read(&service, &id, json!({"path": "large.txt"})).await;
        assert_eq!(result.metadata["read_truncated"], true);
        assert_eq!(result.metadata["bytes_read"], MAX_READ_BYTES);
        assert!(result.truncated);
        assert_no_host_absolute(&result, &root);
    }

    fn text_of(res: &ToolResult) -> String {
        match &res.content[0] {
            ContentPart::Text(t) => t.text.clone(),
            _ => String::new(),
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn rejects_symlink_escape_and_git_internals() {
        let (service, id, root, _ws_dir) = make_service();
        let outside = temp_root("outside");
        fs::write(outside.path().join("secret.txt"), "top-secret\n").unwrap();
        std::os::unix::fs::symlink(outside.path().join("secret.txt"), root.join("auth-link"))
            .unwrap();
        std::os::unix::fs::symlink("/etc", root.join("etc-link")).unwrap();
        fs::create_dir_all(root.join(".git")).unwrap();
        fs::write(root.join(".git/config"), "[core]\n").unwrap();

        let err = read(
            &service,
            &id,
            &json!({"path": "auth-link"}),
            CancellationToken::new(),
        )
        .await
        .unwrap_err();
        assert!(
            matches!(
                err,
                ReadFileError::Common(BuiltinToolError::PolicyPath(PathSafetyError::SymlinkEscape))
            ),
            "{err:?}"
        );

        let err = read(
            &service,
            &id,
            &json!({"path": "etc-link"}),
            CancellationToken::new(),
        )
        .await
        .unwrap_err();
        assert!(
            matches!(
                err,
                ReadFileError::Common(BuiltinToolError::PolicyPath(PathSafetyError::SymlinkEscape))
            ),
            "{err:?}"
        );

        let err = read(
            &service,
            &id,
            &json!({"path": ".git/config"}),
            CancellationToken::new(),
        )
        .await
        .unwrap_err();
        assert!(
            matches!(
                err,
                ReadFileError::Common(BuiltinToolError::PolicyPath(PathSafetyError::GitInternals))
            ),
            "{err:?}"
        );
        let error: ToolError = BuiltinToolError::from(err).into();
        assert_eq!(error.kind, ToolErrorKind::PermissionDenied);
    }
}
