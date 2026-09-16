//! Agent-facing interactive PTY tool. Shares the GUI terminal registry and broadcast.
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use pawork_domain::{
    CancellationToken, ContentPart, CoreInstanceId, TextContent, ToolCapability, ToolDescriptor,
    ToolError, ToolErrorKind, ToolEventSink, ToolExecutionContext, ToolHosting, ToolKind,
    ToolRequest, ToolResult, WorkspaceId,
};
use pawork_exec::{
    OwnerSessionId, PtyCreateSpec, PtyService, PtySnapshot, PtyWindowSize, TerminalId,
};
use pawork_workspace::{WorkspacePathError, resolve_relative_path};
use serde_json::{Value, json};

use super::bus::{ActiveGuiRun, GuiEventBus, GuiRunRegistry};
use super::handlers::terminal::{
    cwd_label_for_terminal, decode_registered_terminal, register_terminal,
    spawn_terminal_output_forwarder, unregister_terminal,
};

const MAX_OUTPUT_BYTES: usize = 64 * 1024;
const WRITE_SNAPSHOT_WAIT: Duration = Duration::from_millis(100);
const MAX_COMMAND_BYTES: usize = 8 * 1024;

pub(super) struct TerminalTool {
    pub(super) pty: Arc<PtyService>,
    pub(super) terminals: Arc<Mutex<HashMap<String, String>>>,
    pub(super) runs: Arc<GuiRunRegistry>,
    pub(super) bus: Arc<GuiEventBus>,
    pub(super) instance: CoreInstanceId,
    pub(super) shell: Option<String>,
    pub(super) size: PtyWindowSize,
}

fn error(kind: ToolErrorKind, message: impl Into<String>) -> ToolError {
    ToolError {
        kind,
        message: message.into(),
        retryable: false,
        retry_after_ms: None,
    }
}

fn path_error(err: WorkspacePathError) -> ToolError {
    match err {
        WorkspacePathError::Empty => error(ToolErrorKind::InvalidInput, err.to_string()),
        WorkspacePathError::NoRoot => error(ToolErrorKind::NotFound, err.to_string()),
        WorkspacePathError::AbsolutePath
        | WorkspacePathError::Traversal(_)
        | WorkspacePathError::ReservedDeviceName(_)
        | WorkspacePathError::SymlinkEscape
        | WorkspacePathError::GitInternals
        | WorkspacePathError::NonRegular => error(ToolErrorKind::PermissionDenied, err.to_string()),
        WorkspacePathError::Io(io) if io.kind() == std::io::ErrorKind::NotFound => {
            error(ToolErrorKind::NotFound, io.to_string())
        }
        WorkspacePathError::Io(io) => error(ToolErrorKind::ExecutionFailed, io.to_string()),
    }
}

fn pty_error(err: pawork_exec::PtyError) -> ToolError {
    let kind = match err {
        pawork_exec::PtyError::NotFound(_) => ToolErrorKind::NotFound,
        pawork_exec::PtyError::Ownership(_, _) => ToolErrorKind::PermissionDenied,
        pawork_exec::PtyError::Closed(_) => ToolErrorKind::Conflict,
        _ => ToolErrorKind::ExecutionFailed,
    };
    error(kind, err.to_string())
}

fn require_str<'a>(input: &'a Value, field: &'static str) -> Result<&'a str, ToolError> {
    input
        .get(field)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| error(ToolErrorKind::InvalidInput, format!("missing {field}")))
}

fn validate_command(command: &str) -> Result<(), ToolError> {
    if command.len() > MAX_COMMAND_BYTES {
        return Err(error(ToolErrorKind::InvalidInput, "command exceeds 8KiB"));
    }
    if command.chars().any(|ch| ch.is_control() && ch != '\t') {
        return Err(error(
            ToolErrorKind::InvalidInput,
            "command must be a complete shell line; raw control characters are not accepted",
        ));
    }
    Ok(())
}

fn last_utf8(bytes: &[u8], max: usize) -> (String, bool) {
    if bytes.len() <= max {
        return (String::from_utf8_lossy(bytes).into_owned(), false);
    }
    let slice = &bytes[bytes.len() - max..];
    let start = slice
        .iter()
        .position(|byte| byte & 0b1100_0000 != 0b1000_0000)
        .unwrap_or(0);
    (String::from_utf8_lossy(&slice[start..]).into_owned(), true)
}

fn snapshot_payload(snapshot: &PtySnapshot) -> Value {
    let (output, truncated) = last_utf8(&snapshot.buffered, MAX_OUTPUT_BYTES);
    json!({
        "terminal_session_id": snapshot.terminal_id.as_str(),
        "state": format!("{:?}", snapshot.state).to_ascii_lowercase(),
        "exit_code": snapshot.exit_code,
        "exit_signal": snapshot.exit_signal,
        "cursor_start": snapshot.buffer_start,
        "cursor_end": snapshot.buffer_end,
        "truncated": truncated,
        "dropped_events": snapshot.dropped_events,
        "columns": snapshot.size.cols,
        "rows": snapshot.size.rows,
        "output": output,
    })
}

fn success_json(value: Value) -> ToolResult {
    ToolResult::success(vec![ContentPart::Text(TextContent {
        text: value.to_string(),
    })])
}

#[async_trait]
impl pawork_domain::AgentTool for TerminalTool {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "terminal".into(),
            description: "Control an interactive PTY in this task's workspace. Actions: list, create(cwd), read(terminal_session_id), write(terminal_session_id, command), interrupt(terminal_session_id), close(terminal_session_id). write submits one complete shell command and appends CR; it does not accept raw PTY bytes. This is an interactive unsandboxed process (same PTY list as the user Terminal panel), not run_command. It requires explicit approval.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["list", "create", "read", "write", "interrupt", "close"]
                    },
                    "terminal_session_id": { "type": "string" },
                    "command": { "type": "string" },
                    "cwd": { "type": "string" }
                },
                "required": ["action"],
                "additionalProperties": false
            }),
            capability: ToolCapability::Process,
            kind: ToolKind::ClientFunction,
            hosting: ToolHosting::Local,
            capabilities: Vec::new(),
            requires_approval: true,
            read_only: false,
            supports_concurrency: false,
            default_timeout_ms: Some(15_000),
            max_output_bytes: MAX_OUTPUT_BYTES as u64,
            allowed_in_untrusted_workspace: false,
        }
    }

    async fn execute(
        &self,
        request: ToolRequest,
        context: ToolExecutionContext,
        _sink: &dyn ToolEventSink,
        cancel: CancellationToken,
    ) -> Result<ToolResult, ToolError> {
        if cancel.is_cancelled() {
            return Err(ToolError::cancelled("terminal request cancelled"));
        }
        let run = self.authorized_run(&context)?;
        let action = require_str(&request.input, "action")?;
        match action {
            "list" => self.list(&run.workspace_id),
            "create" => {
                self.create(&run, request.input.get("cwd").and_then(Value::as_str))
                    .await
            }
            "read" => self.read(
                &run.workspace_id,
                require_str(&request.input, "terminal_session_id")?,
            ),
            "write" => {
                self.write(
                    &run.workspace_id,
                    require_str(&request.input, "terminal_session_id")?,
                    require_str(&request.input, "command")?,
                )
                .await
            }
            "interrupt" => {
                self.interrupt(
                    &run.workspace_id,
                    require_str(&request.input, "terminal_session_id")?,
                )
                .await
            }
            "close" => {
                self.close(&run, require_str(&request.input, "terminal_session_id")?)
                    .await
            }
            _ => Err(error(
                ToolErrorKind::InvalidInput,
                "unknown terminal action",
            )),
        }
    }
}

impl TerminalTool {
    fn authorized_run(&self, context: &ToolExecutionContext) -> Result<ActiveGuiRun, ToolError> {
        self.runs
            .active()
            .into_iter()
            .find(|run| run.run_id == context.run_id && run.workspace_id == context.workspace_id)
            .ok_or_else(|| {
                error(
                    ToolErrorKind::PermissionDenied,
                    "terminal requires an active GUI run in this workspace",
                )
            })
    }

    fn owned_terminal(
        &self,
        workspace_id: &WorkspaceId,
        terminal_session_id: &str,
    ) -> Result<(TerminalId, OwnerSessionId), ToolError> {
        let registration = self
            .terminals
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(terminal_session_id)
            .cloned()
            .ok_or_else(|| {
                error(
                    ToolErrorKind::NotFound,
                    format!("terminal {terminal_session_id} is not registered"),
                )
            })?;
        let (owner, _) = decode_registered_terminal(&registration);
        if owner != workspace_id.as_str() {
            return Err(error(
                ToolErrorKind::PermissionDenied,
                "terminal is not owned by this workspace",
            ));
        }
        Ok((
            TerminalId::new(terminal_session_id),
            OwnerSessionId::new(owner),
        ))
    }

    fn list(&self, workspace_id: &WorkspaceId) -> Result<ToolResult, ToolError> {
        let registered: Vec<(String, String)> = self
            .terminals
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .map(|(id, registration)| (id.clone(), registration.clone()))
            .collect();
        let entries: Vec<Value> = registered
            .into_iter()
            .filter_map(|(id, registration)| {
                let (owner, cwd) = decode_registered_terminal(&registration);
                if owner != workspace_id.as_str() {
                    return None;
                }
                let mut entry = json!({ "terminal_session_id": id });
                if let Some(cwd) = cwd {
                    entry["cwd"] = Value::String(cwd.to_string());
                }
                Some(entry)
            })
            .collect();
        Ok(success_json(json!({ "terminals": entries })))
    }

    async fn create(&self, run: &ActiveGuiRun, cwd: Option<&str>) -> Result<ToolResult, ToolError> {
        if run.workspace_roots.is_empty() {
            return Err(error(ToolErrorKind::NotFound, "workspace has no roots"));
        }
        let resolved =
            resolve_relative_path(&run.workspace_roots, cwd.unwrap_or(".")).map_err(path_error)?;
        let cwd_label = cwd_label_for_terminal(resolved.relative);
        let owner = OwnerSessionId::new(run.workspace_id.as_str());
        let spec = PtyCreateSpec {
            owner_session: owner.clone(),
            cwd: Some(resolved.absolute),
            shell: self.shell.clone(),
            size: self.size,
            env: vec![("TERM".into(), "xterm".into())],
            ..PtyCreateSpec::default()
        };
        let terminal_id = self.pty.create(spec).await.map_err(pty_error)?;
        register_terminal(&self.terminals, &terminal_id, &owner, &cwd_label);
        spawn_terminal_output_forwarder(
            &self.pty,
            Arc::clone(&self.bus),
            self.instance.clone(),
            terminal_id.clone(),
            owner,
        );
        self.bus.publish_diagnostic(
            self.instance.clone(),
            &run.session_id,
            "terminal.agent_created",
            json!({
                "terminal_session_id": terminal_id.as_str(),
                "workspace_id": run.workspace_id.as_str(),
                "cwd": cwd_label,
            }),
        );
        Ok(success_json(json!({
            "terminal_session_id": terminal_id.as_str(),
            "cwd": cwd_label,
            "sandboxed": false,
        })))
    }

    fn read(
        &self,
        workspace_id: &WorkspaceId,
        terminal_session_id: &str,
    ) -> Result<ToolResult, ToolError> {
        let (terminal_id, owner) = self.owned_terminal(workspace_id, terminal_session_id)?;
        Ok(success_json(snapshot_payload(
            &self.pty.snapshot(&terminal_id, &owner).map_err(pty_error)?,
        )))
    }

    async fn write(
        &self,
        workspace_id: &WorkspaceId,
        terminal_session_id: &str,
        command: &str,
    ) -> Result<ToolResult, ToolError> {
        let (terminal_id, owner) = self.owned_terminal(workspace_id, terminal_session_id)?;
        validate_command(command)?;
        let mut data = command.as_bytes().to_vec();
        data.push(b'\r');
        self.pty
            .write(&terminal_id, &owner, data)
            .await
            .map_err(pty_error)?;
        tokio::time::sleep(WRITE_SNAPSHOT_WAIT).await;
        let mut payload =
            snapshot_payload(&self.pty.snapshot(&terminal_id, &owner).map_err(pty_error)?);
        payload["note"] = json!("snapshot after ~100ms; command completion is not asserted");
        Ok(success_json(payload))
    }

    async fn interrupt(
        &self,
        workspace_id: &WorkspaceId,
        terminal_session_id: &str,
    ) -> Result<ToolResult, ToolError> {
        let (terminal_id, owner) = self.owned_terminal(workspace_id, terminal_session_id)?;
        self.pty
            .write(&terminal_id, &owner, vec![0x03])
            .await
            .map_err(pty_error)?;
        Ok(success_json(snapshot_payload(
            &self.pty.snapshot(&terminal_id, &owner).map_err(pty_error)?,
        )))
    }

    async fn close(
        &self,
        run: &ActiveGuiRun,
        terminal_session_id: &str,
    ) -> Result<ToolResult, ToolError> {
        let (terminal_id, owner) = self.owned_terminal(&run.workspace_id, terminal_session_id)?;
        self.pty
            .cleanup(&terminal_id, &owner)
            .await
            .map_err(pty_error)?;
        unregister_terminal(&self.terminals, terminal_session_id);
        self.bus.publish_diagnostic(
            self.instance.clone(),
            &run.session_id,
            "terminal.agent_closed",
            json!({ "terminal_session_id": terminal_session_id }),
        );
        Ok(success_json(json!({
            "terminal_session_id": terminal_session_id,
            "closed": true,
        })))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pawork_domain::{AgentTool, ErrorCategory, RunId, SessionId, ToolCallId, ToolStreamEvent};
    use pawork_engine::now_timestamp;
    use pawork_policy::ApprovalMode;
    use pawork_tools::{NoopToolEventSink, ToolRegistry, ToolScheduler, ToolSchedulerConfig};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct NoopSink;

    #[async_trait]
    impl ToolEventSink for NoopSink {
        async fn emit(&self, _event: ToolStreamEvent) -> Result<(), ToolError> {
            Ok(())
        }
    }

    fn unique(prefix: &str) -> String {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        format!(
            "{prefix}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        )
    }

    fn tool(root: PathBuf) -> (TerminalTool, WorkspaceId, RunId) {
        let workspace_id = WorkspaceId::from(unique("ws"));
        let run_id = RunId::from(unique("run"));
        let runs = Arc::new(GuiRunRegistry::new());
        runs.register(
            ActiveGuiRun {
                run_id: run_id.clone(),
                session_id: SessionId::from(unique("session")),
                workspace_id: workspace_id.clone(),
                workspace_roots: vec![root],
                started_at_ms: now_timestamp().as_unix_millis(),
            },
            CancellationToken::new(),
        );
        let tool = TerminalTool {
            pty: Arc::new(PtyService::new()),
            terminals: Arc::new(Mutex::new(HashMap::new())),
            runs,
            bus: Arc::new(GuiEventBus::new(16)),
            instance: CoreInstanceId::from("instance-tool"),
            shell: Some("/bin/sh".into()),
            size: PtyWindowSize::default(),
        };
        (tool, workspace_id, run_id)
    }

    fn context(workspace_id: &WorkspaceId, run_id: &RunId) -> ToolExecutionContext {
        ToolExecutionContext {
            workspace_id: workspace_id.clone(),
            run_id: run_id.clone(),
            working_directory: None,
        }
    }

    fn request(input: Value) -> ToolRequest {
        ToolRequest {
            tool_call_id: ToolCallId::from("call-terminal"),
            input,
        }
    }

    fn text_json(result: &ToolResult) -> Value {
        let ContentPart::Text(TextContent { text }) = &result.content[0] else {
            panic!("expected text result: {result:?}");
        };
        serde_json::from_str(text).expect("json result")
    }

    async fn wait_output(
        tool: &TerminalTool,
        ctx: &ToolExecutionContext,
        id: &str,
        needle: &str,
    ) -> String {
        let mut output = String::new();
        for _ in 0..40 {
            let read = text_json(
                &tool
                    .execute(
                        request(json!({"action": "read", "terminal_session_id": id})),
                        ctx.clone(),
                        &NoopSink,
                        CancellationToken::new(),
                    )
                    .await
                    .expect("read"),
            );
            output = read["output"].as_str().unwrap_or("").to_string();
            if output.contains(needle) {
                return output;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        output
    }

    #[tokio::test]
    async fn create_write_interrupt_close_executes_and_rejects_foreign_workspace() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (tool, workspace_id, run_id) = tool(dir.path().to_path_buf());
        let ctx = context(&workspace_id, &run_id);
        let created = text_json(
            &tool
                .execute(
                    request(json!({"action": "create", "cwd": "."})),
                    ctx.clone(),
                    &NoopSink,
                    CancellationToken::new(),
                )
                .await
                .expect("create"),
        );
        let id = created["terminal_session_id"]
            .as_str()
            .expect("id")
            .to_string();

        let left = unique("pw");
        let right = unique("tk");
        let token = format!("{left}{right}");
        let command = format!("printf '%s%s\\n' '{left}' '{right}'");
        assert!(!command.contains(&token));
        tool.execute(
            request(json!({
                "action": "write",
                "terminal_session_id": id,
                "command": command,
            })),
            ctx.clone(),
            &NoopSink,
            CancellationToken::new(),
        )
        .await
        .expect("write");
        let output = wait_output(&tool, &ctx, &id, &token).await;
        assert!(
            output.contains(&token),
            "PTY must execute the command, not just echo it: {output:?}"
        );

        tool.execute(
            request(json!({
                "action": "write",
                "terminal_session_id": id,
                "command": "sleep 30",
            })),
            ctx.clone(),
            &NoopSink,
            CancellationToken::new(),
        )
        .await
        .expect("sleep");
        tool.execute(
            request(json!({"action": "interrupt", "terminal_session_id": id})),
            ctx.clone(),
            &NoopSink,
            CancellationToken::new(),
        )
        .await
        .expect("interrupt");
        tool.execute(
            request(json!({"action":"write", "terminal_session_id":id,
                "command":"printf '%s%s' 'INTERRUPT-' 'OK'"})),
            ctx.clone(),
            &NoopSink,
            CancellationToken::new(),
        )
        .await
        .expect("write after interrupt");
        let interrupted = wait_output(&tool, &ctx, &id, "INTERRUPT-OK").await;
        assert!(
            interrupted.contains("INTERRUPT-OK"),
            "interrupt must stop sleep and let the shell execute: {interrupted:?}"
        );

        let foreign = ToolExecutionContext {
            workspace_id: WorkspaceId::from("ws-other"),
            run_id: RunId::from("foreign-run"),
            working_directory: None,
        };
        tool.runs.register(
            ActiveGuiRun {
                run_id: foreign.run_id.clone(),
                session_id: "foreign-session".into(),
                workspace_id: foreign.workspace_id.clone(),
                workspace_roots: vec![dir.path().to_path_buf()],
                started_at_ms: 1,
            },
            CancellationToken::new(),
        );
        let denied = tool
            .execute(
                request(json!({"action": "read", "terminal_session_id": id})),
                foreign,
                &NoopSink,
                CancellationToken::new(),
            )
            .await
            .expect_err("cross-workspace must deny");
        assert_eq!(denied.kind, ToolErrorKind::PermissionDenied);

        tool.execute(
            request(json!({"action": "close", "terminal_session_id": id})),
            ctx,
            &NoopSink,
            CancellationToken::new(),
        )
        .await
        .expect("close");
        assert!(tool.terminals.lock().unwrap().get(&id).is_none());
    }

    #[tokio::test]
    async fn rejects_escape_cancel_raw_write_and_scheduler_gates() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (tool, workspace_id, run_id) = tool(dir.path().to_path_buf());
        let ctx = context(&workspace_id, &run_id);

        let escaped = tool
            .execute(
                request(json!({"action": "create", "cwd": "../outside"})),
                ctx.clone(),
                &NoopSink,
                CancellationToken::new(),
            )
            .await
            .expect_err("path escape");
        assert_eq!(escaped.kind, ToolErrorKind::PermissionDenied);

        let cancel = CancellationToken::new();
        cancel.cancel();
        let cancelled = tool
            .execute(
                request(json!({"action": "create"})),
                ctx.clone(),
                &NoopSink,
                cancel,
            )
            .await
            .expect_err("cancelled");
        assert_eq!(cancelled.kind, ToolErrorKind::Cancelled);
        assert!(tool.terminals.lock().unwrap().is_empty());

        let created = text_json(
            &tool
                .execute(
                    request(json!({"action": "create"})),
                    ctx.clone(),
                    &NoopSink,
                    CancellationToken::new(),
                )
                .await
                .expect("create for write checks"),
        );
        let id = created["terminal_session_id"]
            .as_str()
            .expect("id")
            .to_string();
        let mut control = String::from("printf hi");
        control.push(char::from(3));
        let raw = tool
            .execute(
                request(json!({
                    "action": "write",
                    "terminal_session_id": id,
                    "command": control,
                })),
                ctx.clone(),
                &NoopSink,
                CancellationToken::new(),
            )
            .await
            .expect_err("raw control");
        assert_eq!(raw.kind, ToolErrorKind::InvalidInput);
        let oversized = tool
            .execute(
                request(json!({
                    "action": "write",
                    "terminal_session_id": id,
                    "command": "x".repeat(MAX_COMMAND_BYTES + 1),
                })),
                ctx,
                &NoopSink,
                CancellationToken::new(),
            )
            .await
            .expect_err("oversized");
        assert_eq!(oversized.kind, ToolErrorKind::InvalidInput);
        tool.execute(
            request(json!({"action": "close", "terminal_session_id": id})),
            context(&workspace_id, &run_id),
            &NoopSink,
            CancellationToken::new(),
        )
        .await
        .ok();

        let gated = TerminalTool {
            pty: Arc::new(PtyService::new()),
            terminals: Arc::new(Mutex::new(HashMap::new())),
            runs: Arc::new(GuiRunRegistry::new()),
            bus: Arc::new(GuiEventBus::new(8)),
            instance: CoreInstanceId::from("gate"),
            shell: Some("/bin/sh".into()),
            size: PtyWindowSize::default(),
        };
        for (mode, trusted) in [
            (ApprovalMode::ReadOnly, true),
            (ApprovalMode::AskForDangerous, false),
            (ApprovalMode::AskForDangerous, true),
        ] {
            let mut registry = ToolRegistry::new();
            registry
                .register(Arc::new(TerminalTool {
                    pty: gated.pty.clone(),
                    terminals: gated.terminals.clone(),
                    runs: gated.runs.clone(),
                    bus: gated.bus.clone(),
                    instance: gated.instance.clone(),
                    shell: gated.shell.clone(),
                    size: gated.size,
                }))
                .unwrap();
            let scheduler = ToolScheduler::new(
                registry,
                ToolSchedulerConfig {
                    approval_mode: mode,
                    workspace_trusted: trusted,
                    max_concurrent: 1,
                },
            );
            let result = scheduler
                .execute_named(
                    "terminal",
                    request(json!({"action": "create"})),
                    ToolExecutionContext {
                        workspace_id: "ws".into(),
                        run_id: "run".into(),
                        working_directory: None,
                    },
                    CancellationToken::new(),
                    None,
                    &NoopToolEventSink,
                )
                .await
                .unwrap();
            assert!(!result.success);
            assert_eq!(result.error.unwrap().category, ErrorCategory::Authorization);
            assert!(gated.terminals.lock().unwrap().is_empty());
        }
    }
}
