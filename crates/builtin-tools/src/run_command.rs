//! `run_command` 工具（P4-5）。
//!
//! 非 PTY 执行：流式 stdout/stderr、cwd、timeout、env 白名单、cancel、exit code。

use std::time::Duration;

use agent_domain::{ContentPart, TextContent, WorkspaceId};
use async_trait::async_trait;
use process_runtime::{CommandSpec, ProcessRuntime};
use serde_json::{json, Value};
use tool_api::AgentTool;
use tool_api::CancellationToken;
use tool_api::ToolCapability;
use tool_api::ToolDescriptor;
use tool_api::ToolError;
use tool_api::ToolEventSink;
use tool_api::ToolExecutionContext;
use tool_api::ToolOutputChannel;
use tool_api::ToolRequest;
use tool_api::ToolResult;
use tool_api::ToolStreamEvent;
use workspace_service::WorkspaceService;

use crate::common::opt_u64;
use crate::common::require_str;
use crate::common::resolve_rel;
use crate::common::workspace_roots;
use crate::common::BuiltinToolError;

/// 环境变量白名单：仅这些变量透传，避免泄漏 Secret。
const ENV_ALLOWLIST: &[&str] = &["PATH", "HOME", "LANG", "LC_ALL", "TERM"];

const DEFAULT_TIMEOUT_MS: u64 = 30_000;
const MAX_OUTPUT_BYTES: u64 = 8 * 1024 * 1024;

/// `run_command` 工具。
#[derive(Clone)]
pub struct RunCommandTool {
    workspaces: WorkspaceService,
    runtime: ProcessRuntime,
}

impl RunCommandTool {
    pub fn new(workspaces: WorkspaceService) -> Self {
        Self {
            workspaces,
            runtime: ProcessRuntime::new(),
        }
    }

    pub fn with_runtime(workspaces: WorkspaceService, runtime: ProcessRuntime) -> Self {
        Self {
            workspaces,
            runtime,
        }
    }
}

#[async_trait]
impl AgentTool for RunCommandTool {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "run_command".into(),
            description: "Run a non-PTY command with streaming output, cwd, timeout, env allowlist, cancel and exit code.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "command": { "type": "string" },
                    "argv": { "type": "array", "items": { "type": "string" } },
                    "cwd": { "type": "string" },
                    "timeout_ms": { "type": "integer", "minimum": 100 },
                    "env": { "type": "object" }
                }
            }),
            capability: ToolCapability::Process,
            read_only: false,
            supports_concurrency: false,
            default_timeout_ms: Some(DEFAULT_TIMEOUT_MS),
            max_output_bytes: MAX_OUTPUT_BYTES,
            allowed_in_untrusted_workspace: false,
        }
    }

    async fn execute(
        &self,
        request: ToolRequest,
        context: ToolExecutionContext,
        sink: &dyn ToolEventSink,
        cancel: CancellationToken,
    ) -> Result<ToolResult, ToolError> {
        match run(
            &self.workspaces,
            self.runtime,
            &context.workspace_id,
            &request.input,
            sink,
            cancel,
        )
        .await
        {
            Ok(result) => Ok(result),
            Err(error) => Err(BuiltinToolError::from(error).into()),
        }
    }
}

async fn run(
    service: &WorkspaceService,
    runtime: ProcessRuntime,
    workspace_id: &WorkspaceId,
    input: &Value,
    sink: &dyn ToolEventSink,
    cancel: CancellationToken,
) -> Result<ToolResult, RunCommandError> {
    let (program, args) = parse_command(input)?;
    let cwd = match crate::common::opt_str(input, "cwd") {
        Some(rel) => {
            let roots = workspace_roots(service, workspace_id)?;
            Some(resolve_rel(&roots, &rel)?)
        }
        None => None,
    };
    let timeout_ms = opt_u64(input, "timeout_ms").unwrap_or(DEFAULT_TIMEOUT_MS);
    let env_map = input.get("env").and_then(|v| v.as_object());

    let mut spec = CommandSpec::new(program).args(args);
    spec.timeout = Some(Duration::from_millis(timeout_ms));
    spec.cwd = cwd;
    spec.max_output_bytes = MAX_OUTPUT_BYTES;
    spec.env_clear = true;
    // 仅透传白名单变量 + 显式 env。
    for name in ENV_ALLOWLIST {
        if let Ok(value) = std::env::var(name) {
            spec.env.push(((*name).to_string(), value));
        }
    }
    if let Some(map) = env_map {
        for (k, v) in map {
            if let Some(s) = v.as_str() {
                spec.env.push((k.clone(), s.to_string()));
            }
        }
    }

    // 执行：run() 正确处理 timeout、cancel 与进程树终止。结果以事件回放保证流式可见。
    let output = runtime
        .run(spec, cancel.clone())
        .await
        .map_err(|e| RunCommandError::Process(e.to_string()))?;

    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    if !stdout.is_empty() {
        let _ = sink
            .emit(ToolStreamEvent::OutputDelta {
                channel: ToolOutputChannel::Stdout,
                delta: stdout.clone(),
            })
            .await;
    }
    if !stderr.is_empty() {
        let _ = sink
            .emit(ToolStreamEvent::OutputDelta {
                channel: ToolOutputChannel::Stderr,
                delta: stderr.clone(),
            })
            .await;
    }
    let exit_code = output.exit_code;
    let truncated = output.truncated;
    let success = exit_code == Some(0) && !output.timed_out && !output.killed;
    let metadata = json!({
        "exit_code": exit_code,
        "stdout_bytes": stdout.len(),
        "stderr_bytes": stderr.len(),
        "truncated": truncated,
        "success": success,
    });
    let mut text = stdout;
    if !stderr.is_empty() {
        text.push_str("\n[stderr]\n");
        text.push_str(&stderr);
    }
    if let Some(code) = exit_code {
        if code != 0 {
            text.push_str(&format!("\n[exit {code}]"));
        }
    }
    Ok(ToolResult {
        content: vec![ContentPart::Text(TextContent { text })],
        artifacts: Vec::new(),
        metadata,
        truncated,
        success,
        error: None,
    })
}

/// 解析命令：argv 数组优先，否则 command 字符串经 `sh -c` 执行。
fn parse_command(input: &Value) -> Result<(String, Vec<String>), RunCommandError> {
    if let Some(argv) = input.get("argv").and_then(|v| v.as_array()) {
        let mut iter = argv.iter();
        let program = iter
            .next()
            .and_then(|v| v.as_str())
            .ok_or(BuiltinToolError::MissingField("argv[0]"))?
            .to_string();
        let args: Vec<String> = iter.filter_map(|v| v.as_str().map(String::from)).collect();
        return Ok((program, args));
    }
    let command = require_str(input, "command")?;
    // command 字符串保持 shell 语义：通过 `sh -c` 执行，避免手工词法分析。
    Ok(("sh".to_string(), vec!["-c".to_string(), command]))
}

#[derive(Debug, thiserror::Error)]
pub enum RunCommandError {
    #[error(transparent)]
    Common(#[from] BuiltinToolError),
    #[error("process error: {0}")]
    Process(String),
}

impl From<RunCommandError> for BuiltinToolError {
    fn from(error: RunCommandError) -> Self {
        match error {
            RunCommandError::Common(common) => common,
            RunCommandError::Process(msg) => BuiltinToolError::Process(msg),
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
    use test_support::RecordingToolSink;
    use tool_api::ToolOutputChannel;
    use tool_api::ToolStreamEvent;

    static NEXT: AtomicU64 = AtomicU64::new(1);

    fn temp_root(name: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "pawork-runcmd-{}-{}-{name}",
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

    #[tokio::test]
    async fn captures_stdout_and_exit_code() {
        let (service, id, _root) = make_service();
        let sink = RecordingToolSink::default();
        let res = run(
            &service,
            ProcessRuntime::new(),
            &id,
            &json!({"command": "echo hello"}),
            &sink,
            CancellationToken::new(),
        )
        .await
        .expect("run");
        assert!(res.success);
        assert_eq!(res.metadata["exit_code"], 0);
        let text = match &res.content[0] {
            ContentPart::Text(t) => &t.text,
            _ => panic!("text"),
        };
        assert!(text.contains("hello"));
        assert!(sink.events().iter().any(|e| matches!(
            e,
            ToolStreamEvent::OutputDelta {
                channel: ToolOutputChannel::Stdout,
                ..
            }
        )));
    }

    #[tokio::test]
    async fn non_zero_exit_is_not_success() {
        let (service, id, _root) = make_service();
        let sink = RecordingToolSink::default();
        let res = run(
            &service,
            ProcessRuntime::new(),
            &id,
            &json!({"command": "sh -c \"exit 7\""}),
            &sink,
            CancellationToken::new(),
        )
        .await
        .expect("run");
        assert!(!res.success);
        assert_eq!(res.metadata["exit_code"], 7);
    }

    #[tokio::test]
    async fn timeout_produces_failure() {
        let (service, id, _root) = make_service();
        let sink = RecordingToolSink::default();
        let res = run(
            &service,
            ProcessRuntime::new(),
            &id,
            &json!({"command": "sleep 10", "timeout_ms": 200}),
            &sink,
            CancellationToken::new(),
        )
        .await
        .expect("run");
        // 超时：进程被杀，无正常退出码 -> 非成功。
        assert!(!res.success);
    }
}
