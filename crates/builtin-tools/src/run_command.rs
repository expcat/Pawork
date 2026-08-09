//! `run_command` 工具（P4-5）。
//!
//! 非 PTY 执行：经 SandboxSelector 选择隔离后端，并保留流式输出、timeout、
//! cancel、资源限制与进程树清理语义。

use std::time::Duration;

use agent_domain::{ContentPart, TextContent, WorkspaceId};
use async_trait::async_trait;
use process_runtime::{CommandSpec, ProcessEvent, ProcessRuntime};
use sandbox_runtime::{
    FilesystemPolicy, NetworkMode, ResourceLimits, SandboxPolicy, SandboxProcessSpec,
    SandboxSelector,
};
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
const MAX_TIMEOUT_MS: u64 = 10 * 60_000;
const MAX_OUTPUT_BYTES: u64 = 8 * 1024 * 1024;
const DEFAULT_CPU_SECONDS: u64 = 60;
const MAX_CPU_SECONDS: u64 = 10 * 60;
const DEFAULT_MEMORY_MB: u64 = 2 * 1024;
const MAX_MEMORY_MB: u64 = 8 * 1024;
const DEFAULT_OPEN_FDS: u64 = 1_024;
const MAX_OPEN_FDS: u64 = 4_096;
const DEFAULT_MAX_PROCS: u32 = 64;
const MAX_MAX_PROCS: u32 = 256;

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
                    "timeout_ms": { "type": "integer", "minimum": 100, "maximum": MAX_TIMEOUT_MS },
                    "max_output_bytes": { "type": "integer", "minimum": 1, "maximum": MAX_OUTPUT_BYTES },
                    "cpu_seconds": { "type": "integer", "minimum": 1, "maximum": MAX_CPU_SECONDS, "default": DEFAULT_CPU_SECONDS },
                    "memory_mb": { "type": "integer", "minimum": 1, "maximum": MAX_MEMORY_MB, "default": DEFAULT_MEMORY_MB },
                    "open_fds": { "type": "integer", "minimum": 3, "maximum": MAX_OPEN_FDS, "default": DEFAULT_OPEN_FDS },
                    "max_procs": { "type": "integer", "minimum": 1, "maximum": MAX_MAX_PROCS, "default": DEFAULT_MAX_PROCS },
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
    let roots = workspace_roots(service, workspace_id)?;
    let cwd = match crate::common::opt_str(input, "cwd") {
        Some(rel) => Some(resolve_rel(&roots, &rel)?),
        None => roots.first().cloned(),
    };
    let timeout_ms = opt_u64(input, "timeout_ms")
        .unwrap_or(DEFAULT_TIMEOUT_MS)
        .clamp(100, MAX_TIMEOUT_MS);
    let env_map = input.get("env").and_then(|v| v.as_object());

    let mut spec = CommandSpec::new(program).args(args);
    spec.timeout = Some(Duration::from_millis(timeout_ms));
    spec.cwd = cwd;
    spec.max_output_bytes = opt_u64(input, "max_output_bytes")
        .unwrap_or(MAX_OUTPUT_BYTES)
        .clamp(1, MAX_OUTPUT_BYTES);
    let max_output_bytes = spec.max_output_bytes;
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

    let mut env_allowlist = ENV_ALLOWLIST
        .iter()
        .map(|name| (*name).to_string())
        .chain(extra_env_allowlist.iter().cloned())
        .collect::<Vec<_>>();
    if let Some(map) = env_map {
        env_allowlist.extend(map.keys().cloned());
    }
    env_allowlist.sort_unstable();
    env_allowlist.dedup();

    // `run_command` 只声明 Process capability；模型输入不能据此自行取得 Network
    // capability。保留旧字段仅用于审计，真正策略始终 fail-closed。
    let network_requested = input
        .get("needs_network")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let cpu_seconds = opt_u64(input, "cpu_seconds")
        .unwrap_or(DEFAULT_CPU_SECONDS)
        .clamp(1, MAX_CPU_SECONDS);
    let memory_mb = opt_u64(input, "memory_mb")
        .unwrap_or(DEFAULT_MEMORY_MB)
        .clamp(1, MAX_MEMORY_MB);
    let open_fds = opt_u64(input, "open_fds")
        .unwrap_or(DEFAULT_OPEN_FDS)
        .clamp(3, MAX_OPEN_FDS);
    let max_procs = opt_u64(input, "max_procs")
        .unwrap_or(u64::from(DEFAULT_MAX_PROCS))
        .clamp(1, u64::from(MAX_MAX_PROCS)) as u32;
    let policy = SandboxPolicy {
        filesystem: FilesystemPolicy {
            read_roots: roots.clone(),
            write_roots: roots.clone(),
            deny: default_secret_paths(),
        },
        network_mode: NetworkMode::Enforce,
        allow_spawn: true,
        max_procs: Some(max_procs),
        env_clear: true,
        env_allowlist,
        env_denylist: vec!["*TOKEN*".into(), "*KEY*".into(), "*SECRET*".into()],
        resources: ResourceLimits {
            cpu_seconds: Some(cpu_seconds),
            memory_mb: Some(memory_mb),
            open_fds: Some(open_fds),
            wall_time_ms: Some(timeout_ms),
            max_output_bytes: Some(max_output_bytes),
        },
        ..Default::default()
    };

    // 真流式执行：选择器优先硬隔离，缺失时结构化回退；进程运行期间立即向 sink
    // 发出增量，同时保留有界最终结果。
    let selector = SandboxSelector::with_runtime(runtime);
    let (backend, selection) = selector.pick();
    let mut process = backend
        .spawn(
            SandboxProcessSpec {
                command: spec,
                workspace_roots: roots,
                needs_network: false,
            },
            policy,
            cancel,
        )
        .await
        .map_err(|e| RunCommandError::Sandbox(e.to_string()))?;
    let events = &mut process.events;
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
        "sandbox": {
            "backend": selection.id,
            "isolation": selection.isolation.as_str(),
            "fallback": selection.fallback,
            "note": selection.note,
            "attempted": selection.attempted,
            "network": {
                "requested": network_requested,
                "granted": false,
                "mode": "enforce",
            },
            "limits": {
                "timeout_ms": timeout_ms,
                "cpu_seconds": cpu_seconds,
                "memory_mb": memory_mb,
                "open_fds": open_fds,
                "max_procs": max_procs,
                "max_output_bytes": max_output_bytes,
            },
        },
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
    #[error("sandbox error: {0}")]
    Sandbox(String),
}

impl From<RunCommandError> for BuiltinToolError {
    fn from(error: RunCommandError) -> Self {
        match error {
            RunCommandError::Common(common) => common,
            RunCommandError::Process(msg) => BuiltinToolError::Process(msg),
            RunCommandError::Sandbox(msg) => BuiltinToolError::Process(msg),
        }
    }
}

fn default_secret_paths() -> Vec<std::path::PathBuf> {
    let mut paths = Vec::new();
    if let Some(home) = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }) {
        let home = std::path::PathBuf::from(home);
        paths.extend([".ssh", ".aws", ".azure", ".kube"].map(|name| home.join(name)));
    }
    #[cfg(windows)]
    if let Some(appdata) = std::env::var_os("APPDATA") {
        paths.push(std::path::PathBuf::from(appdata).join("gcloud"));
    }
    paths
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
        assert!(res.metadata["sandbox"]["backend"].is_string());
        assert!(res.metadata["sandbox"]["isolation"].is_string());
        assert!(res.metadata["sandbox"]["attempted"].is_array());
        assert_eq!(
            res.metadata["sandbox"]["limits"]["max_procs"],
            DEFAULT_MAX_PROCS
        );
        assert_eq!(
            res.metadata["sandbox"]["limits"]["memory_mb"],
            DEFAULT_MEMORY_MB
        );
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
        let input = json!({
            "argv": [
                "powershell",
                "-NoProfile",
                "-Command",
                "[Console]::Out.Write('first'); [Console]::Out.Flush(); Start-Sleep -Milliseconds 800; [Console]::Out.Write('second')"
            ]
        });
        #[cfg(not(windows))]
        let input = json!({"argv": ["sh", "-c", "printf first; sleep 1; printf second"]});
        let task = tokio::spawn(async move {
            run(
                &service,
                ProcessRuntime::new(),
                &id,
                &input,
                &[],
                &sink,
                CancellationToken::new(),
            )
            .await
        });
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if observed.events().iter().any(|event| {
                    matches!(
                        event,
                        ToolStreamEvent::OutputDelta {
                            channel: ToolOutputChannel::Stdout,
                            delta,
                        } if delta.contains("first")
                    )
                }) {
                    break;
                }
                assert!(
                    !task.is_finished(),
                    "process exited before emitting its first output"
                );
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("first output should arrive before timeout");
        assert!(!task.is_finished(), "process should still be running");
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

    #[test]
    fn descriptor_does_not_offer_model_controlled_network_bypass() {
        let (service, _, _) = make_service();
        let descriptor = RunCommandTool::new(service).descriptor();
        let properties = descriptor.input_schema["properties"]
            .as_object()
            .expect("properties");
        assert!(!properties.contains_key("needs_network"));
        assert_eq!(properties["max_procs"]["maximum"], MAX_MAX_PROCS);
        assert_eq!(properties["memory_mb"]["maximum"], MAX_MEMORY_MB);
        assert_eq!(properties["timeout_ms"]["maximum"], MAX_TIMEOUT_MS);
    }

    #[tokio::test]
    async fn legacy_network_request_is_audited_but_not_granted_and_limits_are_clamped() {
        let (service, id, _root) = make_service();
        let result = run(
            &service,
            ProcessRuntime::new(),
            &id,
            &json!({
                "command": "echo bounded",
                "needs_network": true,
                "timeout_ms": u64::MAX,
                "cpu_seconds": u64::MAX,
                "memory_mb": u64::MAX,
                "open_fds": u64::MAX,
                "max_procs": u64::MAX,
            }),
            &[],
            &RecordingToolSink::default(),
            CancellationToken::new(),
        )
        .await
        .expect("run");
        let sandbox = &result.metadata["sandbox"];
        assert_eq!(sandbox["network"]["requested"], true);
        assert_eq!(sandbox["network"]["granted"], false);
        assert_eq!(sandbox["network"]["mode"], "enforce");
        assert_eq!(sandbox["limits"]["timeout_ms"], MAX_TIMEOUT_MS);
        assert_eq!(sandbox["limits"]["cpu_seconds"], MAX_CPU_SECONDS);
        assert_eq!(sandbox["limits"]["memory_mb"], MAX_MEMORY_MB);
        assert_eq!(sandbox["limits"]["open_fds"], MAX_OPEN_FDS);
        assert_eq!(sandbox["limits"]["max_procs"], MAX_MAX_PROCS);
    }

    #[tokio::test]
    async fn sandbox_strips_explicit_secret_environment() {
        let (service, id, _root) = make_service();
        let sink = RecordingToolSink::default();
        #[cfg(windows)]
        let input = json!({
            "argv": [
                "powershell",
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "[Console]::Out.WriteLine(\"VISIBLE=$env:PAWORK_TEST_VISIBLE SECRET=$env:PAWORK_TEST_SECRET\")"
            ],
            "env": {
                "PAWORK_TEST_VISIBLE": "visible-canary",
                "PAWORK_TEST_SECRET": "secret-canary"
            }
        });
        #[cfg(not(windows))]
        let input = json!({
            "argv": ["sh", "-c", "printf 'VISIBLE=%s SECRET=%s' \"$PAWORK_TEST_VISIBLE\" \"$PAWORK_TEST_SECRET\""],
            "env": {
                "PAWORK_TEST_VISIBLE": "visible-canary",
                "PAWORK_TEST_SECRET": "secret-canary"
            }
        });
        let result = run(
            &service,
            ProcessRuntime::new(),
            &id,
            &input,
            &[],
            &sink,
            CancellationToken::new(),
        )
        .await
        .expect("sandboxed run");
        let text = match &result.content[0] {
            ContentPart::Text(text) => &text.text,
            _ => panic!("expected text"),
        };
        assert!(
            text.contains("visible-canary"),
            "visible env missing: {text}"
        );
        assert!(!text.contains("secret-canary"), "secret leaked: {text}");
    }
}
