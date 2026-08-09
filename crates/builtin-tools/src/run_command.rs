//! `run_command` 工具（P4-5）。
//!
//! 非 PTY 执行：流式 stdout/stderr、cwd、timeout、env 白名单、cancel、exit code。

use std::time::Duration;

use agent_domain::{ContentPart, TextContent, WorkspaceId};
use async_trait::async_trait;
use process_runtime::{CommandSpec, ProcessEvent, ProcessRuntime};
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
#[cfg(not(windows))]
const ENV_ALLOWLIST: &[&str] = &["PATH", "HOME", "LANG", "LC_ALL", "TERM", "TMPDIR"];
#[cfg(windows)]
const ENV_ALLOWLIST: &[&str] = &[
    "PATH",
    "HOME",
    "LANG",
    "LC_ALL",
    "TERM",
    "SYSTEMROOT",
    "TEMP",
    "TMP",
    "USERPROFILE",
    "COMSPEC",
    "PATHEXT",
];

const DEFAULT_TIMEOUT_MS: u64 = 30_000;
const MAX_OUTPUT_BYTES: u64 = 8 * 1024 * 1024;

/// `run_command` 工具。
#[derive(Clone)]
pub struct RunCommandTool {
    workspaces: WorkspaceService,
    runtime: ProcessRuntime,
    extra_env_allowlist: Vec<String>,
}

impl RunCommandTool {
    pub fn new(workspaces: WorkspaceService) -> Self {
        Self {
            workspaces,
            runtime: ProcessRuntime::new(),
            extra_env_allowlist: Vec::new(),
        }
    }

    pub fn with_runtime(workspaces: WorkspaceService, runtime: ProcessRuntime) -> Self {
        Self {
            workspaces,
            runtime,
            extra_env_allowlist: Vec::new(),
        }
    }

    /// 追加由配置层明确允许继承的环境变量名。
    pub fn with_extra_env_allowlist<I, S>(mut self, names: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.extra_env_allowlist = names.into_iter().map(Into::into).collect();
        self
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
                    "max_output_bytes": { "type": "integer", "minimum": 1, "maximum": MAX_OUTPUT_BYTES },
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
            &self.extra_env_allowlist,
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
    extra_env_allowlist: &[String],
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
    spec.max_output_bytes = opt_u64(input, "max_output_bytes")
        .unwrap_or(MAX_OUTPUT_BYTES)
        .min(MAX_OUTPUT_BYTES);
    spec.env_clear = true;
    // 仅透传白名单变量 + 显式 env。
    for name in ENV_ALLOWLIST {
        if let Ok(value) = std::env::var(name) {
            spec.env.push(((*name).to_string(), value));
        }
    }
    for name in extra_env_allowlist {
        if ENV_ALLOWLIST.contains(&name.as_str()) {
            continue;
        }
        if let Ok(value) = std::env::var(name) {
            spec.env.push((name.clone(), value));
        }
    }
    if let Some(map) = env_map {
        for (k, v) in map {
            if let Some(s) = v.as_str() {
                spec.env.push((k.clone(), s.to_string()));
            }
        }
    }

    // 真流式执行：进程运行期间立即向 sink 发出增量，同时保留有界最终结果。
    let (mut events, _handle) = runtime
        .spawn_stream(spec, cancel)
        .await
        .map_err(|e| RunCommandError::Process(e.to_string()))?;
    let mut stdout_bytes = Vec::new();
    let mut stderr_bytes = Vec::new();
    let mut exit_code = None;
    let mut truncated = false;
    while let Some(event) = events.recv().await {
        match event {
            ProcessEvent::Stdout(chunk) => {
                stdout_bytes.extend_from_slice(&chunk);
                sink.emit(ToolStreamEvent::OutputDelta {
                    channel: ToolOutputChannel::Stdout,
                    delta: String::from_utf8_lossy(&chunk).into_owned(),
                })
                .await
                .map_err(|error| RunCommandError::Process(error.to_string()))?;
            }
            ProcessEvent::Stderr(chunk) => {
                stderr_bytes.extend_from_slice(&chunk);
                sink.emit(ToolStreamEvent::OutputDelta {
                    channel: ToolOutputChannel::Stderr,
                    delta: String::from_utf8_lossy(&chunk).into_owned(),
                })
                .await
                .map_err(|error| RunCommandError::Process(error.to_string()))?;
            }
            ProcessEvent::Exit {
                code,
                truncated: was_truncated,
            } => {
                exit_code = code;
                truncated = was_truncated;
                break;
            }
        }
    }
    let stdout = String::from_utf8_lossy(&stdout_bytes).into_owned();
    let stderr = String::from_utf8_lossy(&stderr_bytes).into_owned();
    let success = exit_code == Some(0);
    let metadata = json!({
        "exit_code": exit_code,
        "stdout_bytes": stdout_bytes.len(),
        "stderr_bytes": stderr_bytes.len(),
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
    // command 字符串保持 shell 语义：unix 走 `sh -c`、Windows 走
    // `cmd /d /s /c`，避免手工词法分析。
    Ok(shell_argv(command))
}

/// 把 command 字符串包装为平台 shell 的 argv。
#[cfg(not(windows))]
fn shell_argv(command: String) -> (String, Vec<String>) {
    ("sh".to_string(), vec!["-c".to_string(), command])
}

/// 把 command 字符串包装为平台 shell 的 argv（Windows：`cmd /d /s /c`）。
#[cfg(windows)]
fn shell_argv(command: String) -> (String, Vec<String>) {
    (
        "cmd".to_string(),
        vec![
            "/d".to_string(),
            "/s".to_string(),
            "/c".to_string(),
            command,
        ],
    )
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
            &[],
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
        #[cfg(windows)]
        let command = "exit 7";
        #[cfg(not(windows))]
        let command = "sh -c \"exit 7\"";
        let res = run(
            &service,
            ProcessRuntime::new(),
            &id,
            &json!({"command": command}),
            &[],
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
        // Windows 无 sleep，用 ping 制造长时进程。
        #[cfg(windows)]
        let command = "ping -n 30 127.0.0.1";
        #[cfg(not(windows))]
        let command = "sleep 10";
        let res = run(
            &service,
            ProcessRuntime::new(),
            &id,
            &json!({"command": command, "timeout_ms": 200}),
            &[],
            &sink,
            CancellationToken::new(),
        )
        .await
        .expect("run");
        // 超时：进程被杀，无正常退出码 -> 非成功。
        assert!(!res.success);
    }

    #[tokio::test]
    async fn emits_output_before_process_exits() {
        let (service, id, _root) = make_service();
        let sink = RecordingToolSink::default();
        let observed = sink.clone();
        #[cfg(windows)]
        let command = "powershell -NoProfile -Command \"Write-Output first; Start-Sleep -Milliseconds 800; Write-Output second\"";
        #[cfg(not(windows))]
        let command = "printf first; sleep 1; printf second";
        let task = tokio::spawn(async move {
            run(
                &service,
                ProcessRuntime::new(),
                &id,
                &json!({"command": command}),
                &[],
                &sink,
                CancellationToken::new(),
            )
            .await
        });
        tokio::time::sleep(Duration::from_millis(250)).await;
        assert!(!task.is_finished(), "process should still be running");
        assert!(observed.events().iter().any(|event| matches!(
            event,
            ToolStreamEvent::OutputDelta {
                channel: ToolOutputChannel::Stdout,
                delta,
            } if delta.contains("first")
        )));
        assert!(task.await.expect("join").expect("run").success);
    }

    #[test]
    fn platform_environment_allowlist_contains_runtime_basics() {
        assert!(ENV_ALLOWLIST.contains(&"PATH"));
        #[cfg(windows)]
        for name in [
            "SYSTEMROOT",
            "TEMP",
            "TMP",
            "USERPROFILE",
            "COMSPEC",
            "PATHEXT",
        ] {
            assert!(ENV_ALLOWLIST.contains(&name), "missing {name}");
        }
    }
}
