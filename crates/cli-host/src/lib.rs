//! `pawork` 唯一正式宿主的 CLI 装配层（P13-2）。
//!
//! [`CliHost`] 将 Core（[`AppService`] + [`EventHub`]）与 CLI 装配到同一进程：
//! `execute` 统一经命令信封（[`AppCommandEnvelope`]）与查询信封
//! （[`AppQueryEnvelope`]）路由，覆盖四种运行模式：
//!
//! - `run`：一次性执行——打开 workspace → 创建 session → 启动 run → 从
//!   Event Hub 订阅等待终态 → 流式输出；`--serve` 保持服务直到信号。
//! - `serve`：启动 Core 后等待信号；GUI Server 装配位保留为 [`GuiServerHost`]
//!   trait（P13-4 落地）。
//! - `shell`：交互 REPL（`/run` `/cancel` `/sessions` `/workspaces` `/approve`
//!   `/status` `/watch` `/connect`）。
//! - `service`：系统服务 `install` / `start` / `stop`（默认 dry-run，仅打印
//!   注册计划；`--apply` 才修改系统）。
//!
//! 退出策略：一次性模式在目标命令完成后，仅当无活跃 Run、无 GUI 连接、无后台
//! 任务时退出；`--serve` / `serve` / `shell` 模式按各自生命周期结束
//! （[ADR-026]）。
//!
//! [ADR-026]: ../../docs/adr/ADR-026-gui-disconnect-safe.md

use std::collections::BTreeMap;
use std::io::{self, BufRead};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use agent_domain::{
    ActorId, CommandId, ErrorContext, QueryId, RunId, SessionId, Timestamp, WorkspaceId,
};
use app_service::{AppService, ServiceOperation, ServiceRequest, ServiceResponse};
use cli_command::{Cli, Command, RemoteCommand, RunArgs, RunCommand, ServiceCommand};
use cli_renderer::{render, render_event, OutputFormat};
use core_api::{
    ActorIdentity, AppCommand, AppCommandEnvelope, AppQuery, AppQueryEnvelope, AppResponse,
    AppResponseEnvelope, ApprovalDecision, CommandSource, RunState, API_VERSION,
};
use serde_json::Value;
use subscription_hub::{EventHub, HubError};
use transport_remote_placeholder::{
    RemoteGuiTransportProvider, RemotePublishRequest, TransportEndpoint,
};

/// run 模式等待终态的超时。
const RUN_TERMINAL_TIMEOUT: Duration = Duration::from_secs(300);

/// CLI 执行结果：文本输出与进程退出码。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostOutcome {
    pub output: String,
    pub exit_code: i32,
}

/// GUI Server 装配位（serve 模式）：P13-4 实现本地 Transport 服务器后注入。
pub trait GuiServerHost: Send + Sync {
    /// 在 serve 模式下启动 GUI 协议服务器。
    fn start(&self, instance: &str) -> Result<(), String>;
    /// 停止 GUI 协议服务器。
    fn stop(&self) -> Result<(), String>;
}

/// CLI 宿主：持有同一进程内的 AppService 与 EventHub，按命令路由到
/// 四种运行模式或信封命令。
pub struct CliHost {
    service: Arc<AppService>,
    hub: Arc<EventHub>,
    instance: String,
    gui_server: Option<Arc<dyn GuiServerHost>>,
    remote_provider: Option<Arc<dyn RemoteGuiTransportProvider>>,
    next_command_id: AtomicU64,
}

impl CliHost {
    /// 仅以 AppService 装配（legacy 简单命令；run/watch 模式需要 EventPump，
    /// 请使用 [`CliHost::with_hub`] + `core-runtime`）。
    pub fn new(service: Arc<AppService>) -> Self {
        Self::with_hub(service, Arc::new(EventHub::new()))
    }

    /// 以 AppService + EventHub 装配（EventPump 由调用方经 core-runtime 运行）。
    pub fn with_hub(service: Arc<AppService>, hub: Arc<EventHub>) -> Self {
        let instance = service.status().instance;
        Self {
            service,
            hub,
            instance,
            gui_server: None,
            remote_provider: None,
            next_command_id: AtomicU64::new(0),
        }
    }

    /// 注入 GUI Server（serve 模式；未装配时 serve 仅等待信号）。
    pub fn attach_gui_server(&mut self, server: Arc<dyn GuiServerHost>) {
        self.gui_server = Some(server);
    }

    /// 注入 Remote Transport Provider（P13-6 占位 Adapter）；未装配时
    /// `remote` 命令返回结构化错误。
    pub fn attach_remote_provider(&mut self, provider: Arc<dyn RemoteGuiTransportProvider>) {
        self.remote_provider = Some(provider);
    }

    pub fn service(&self) -> &Arc<AppService> {
        &self.service
    }

    pub fn hub(&self) -> &Arc<EventHub> {
        &self.hub
    }

    /// 统一执行入口：按命令路由到四种运行模式或信封命令。
    pub async fn execute(&self, cli: Cli) -> HostOutcome {
        let format = if cli.json {
            OutputFormat::Json
        } else {
            OutputFormat::Text
        };
        match cli.command {
            Command::Serve(args) => self.serve(args.once, format).await,
            Command::Shell => self.shell(format).await,
            Command::Run(args) => match args.command {
                Some(command) => self.run_control(command, format),
                None => self.run(args, format).await,
            },
            Command::Watch => self.watch(format).await,
            Command::Status => self.legacy(ServiceOperation::Status, format),
            Command::Shutdown => self.legacy(ServiceOperation::Shutdown, format),
            Command::Doctor => self.legacy(ServiceOperation::Doctor, format),
            Command::Service(service) => self.service_mode(service.command, format),
            Command::Remote(remote) => self.remote_mode(remote.command, format).await,
            other => self.placeholder_for_command(other, format),
        }
    }

    // ---------- Remote Transport（P13-6 占位 Adapter） ----------

    /// 远程 GUI 端点生命周期：publish / unpublish。
    ///
    /// 无 Provider 时返回结构化错误；发布成功输出 endpoint 与状态，JSON 模式
    /// 携带 handle_id / endpoint 供 unpublish 使用。
    async fn remote_mode(&self, command: RemoteCommand, format: OutputFormat) -> HostOutcome {
        let Some(provider) = &self.remote_provider else {
            let response = ServiceResponse {
                ok: false,
                kind: "remote".into(),
                message: "no remote transport provider is attached (P13-6 placeholder)".into(),
                data: Value::Null,
            };
            return HostOutcome {
                output: render(&response, format),
                exit_code: 1,
            };
        };
        match command {
            RemoteCommand::Publish { name } => {
                let description = provider.describe();
                let request = RemotePublishRequest {
                    name: name.unwrap_or_else(|| self.instance.clone()),
                };
                match provider.publish(request).await {
                    Ok(handle) => {
                        let address = match &handle.endpoint {
                            TransportEndpoint::Remote { address, .. } => address.clone(),
                            other => format!("{other:?}"),
                        };
                        let response = ServiceResponse {
                            ok: true,
                            kind: "remote".into(),
                            message: format!(
                                "remote endpoint published via adapter '{}' (handle {}): {}",
                                description.adapter, handle.id, address
                            ),
                            data: serde_json::json!({
                                "action": "publish",
                                "adapter": description.adapter,
                                "handle_id": handle.id,
                                "endpoint": handle.endpoint,
                                "status": "published",
                            }),
                        };
                        HostOutcome {
                            output: render(&response, format),
                            exit_code: 0,
                        }
                    }
                    Err(error) => self.remote_failure("publish", &error.to_string(), format),
                }
            }
            RemoteCommand::Unpublish { handle } => match provider.unpublish(&handle).await {
                Ok(()) => {
                    let response = ServiceResponse {
                        ok: true,
                        kind: "remote".into(),
                        message: format!("remote endpoint unpublished (handle {handle})"),
                        data: serde_json::json!({
                            "action": "unpublish",
                            "handle_id": handle,
                            "status": "unpublished",
                        }),
                    };
                    HostOutcome {
                        output: render(&response, format),
                        exit_code: 0,
                    }
                }
                Err(error) => self.remote_failure("unpublish", &error.to_string(), format),
            },
        }
    }

    fn remote_failure(&self, action: &str, message: &str, format: OutputFormat) -> HostOutcome {
        let response = ServiceResponse {
            ok: false,
            kind: "remote".into(),
            message: format!("remote {action} failed: {message}"),
            data: serde_json::json!({
                "action": action,
                "error": message,
            }),
        };
        HostOutcome {
            output: render(&response, format),
            exit_code: 1,
        }
    }

    // ---------- 四种运行模式 ----------

    /// 服务模式：启动 Core（lifecycle Ready），等待信号；`--once` 立即退出。
    async fn serve(&self, once: bool, format: OutputFormat) -> HostOutcome {
        let ready = self.legacy(ServiceOperation::Serve, format);
        let mut outputs = vec![ready.output.clone()];
        if once {
            return HostOutcome {
                output: outputs.join("\n"),
                exit_code: ready.exit_code,
            };
        }
        if let Some(server) = &self.gui_server {
            if let Err(error) = server.start(&self.instance) {
                outputs.push(format!("gui server failed to start: {error}"));
                return HostOutcome {
                    output: outputs.join("\n"),
                    exit_code: 1,
                };
            }
        }
        match tokio::signal::ctrl_c().await {
            Ok(()) => {
                if let Some(server) = &self.gui_server {
                    let _ = server.stop();
                }
                self.dispatch(ServiceOperation::Shutdown);
                outputs.push("shutdown requested via signal".into());
                HostOutcome {
                    output: outputs.join("\n"),
                    exit_code: 0,
                }
            }
            Err(error) => HostOutcome {
                output: format!(
                    "{}\nfailed to listen for Ctrl-C: {error}",
                    outputs.join("\n")
                ),
                exit_code: 1,
            },
        }
    }

    /// 一次性模式：workspace → session → run start → Event Hub 等待终态 → 输出。
    async fn run(&self, args: RunArgs, format: OutputFormat) -> HostOutcome {
        let Some(prompt) = args.prompt.filter(|prompt| !prompt.trim().is_empty()) else {
            return self.envelope_outcome(
                "run",
                AppResponseEnvelope {
                    api_version: API_VERSION,
                    request_id: QueryId::from(self.next_id("run")),
                    responded_at: now_timestamp(),
                    response: AppResponse::Error(ErrorContext {
                        category: agent_domain::ErrorCategory::InvalidRequest,
                        message: "run command requires a non-empty --prompt".into(),
                        retryable: false,
                        retry_after_ms: None,
                        diagnostics: BTreeMap::new(),
                    }),
                },
                format,
            );
        };

        // 1) 打开 workspace（指定路径复用已有，否则新建；缺省用当前目录）。
        let workspace_id = match self.ensure_workspace(args.workspace.as_deref()) {
            Ok(workspace_id) => workspace_id,
            Err(message) => {
                return self.error_outcome("run", &message, format);
            }
        };

        // 2) 创建 session。
        let session_id = match self
            .dispatch_envelope(AppCommand::SessionCreate {
                workspace_id: workspace_id.clone(),
                title: Some("CLI run".into()),
            })
            .response
        {
            AppResponse::Data(value) => SessionId::from(
                value
                    .get("session_id")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
            ),
            AppResponse::Error(context) => {
                return self.error_outcome("run", &context.message, format);
            }
            other => {
                return self.error_outcome(
                    "run",
                    &format!("unexpected session create response: {other:?}"),
                    format,
                );
            }
        };

        // 3) 在 RunStart 之前订阅 Event Hub，避免错过终态事件。
        let mut subscription = self.hub.subscribe();

        // 4) 启动 run。
        let response = self.dispatch_envelope(AppCommand::RunStart {
            session_id: session_id.clone(),
            user_message: prompt,
            model: None,
        });
        if !matches!(response.response, AppResponse::Accepted { .. }) {
            return self.envelope_outcome("run", response, format);
        }
        let run_id = self
            .service
            .router()
            .last_started_run()
            .unwrap_or_else(|| RunId::from(""));

        // 5) 流式输出直到终态。
        let mut outputs = vec![];
        let terminal_state = loop {
            match tokio::time::timeout(RUN_TERMINAL_TIMEOUT, subscription.recv()).await {
                Ok(Ok(envelope)) => {
                    outputs.push(render_event(&envelope, format));
                    if let core_api::AppEvent::RunChanged {
                        run_id: event_run,
                        state,
                    } = &envelope.payload
                    {
                        if (run_id.as_str().is_empty() || event_run.as_str() == run_id.as_str())
                            && terminal(state)
                        {
                            break state.clone();
                        }
                    }
                }
                Ok(Err(
                    HubError::Lagged { .. } | HubError::Empty | HubError::ReplayUnavailable { .. },
                )) => continue,
                Ok(Err(HubError::Closed)) => {
                    outputs.push("event hub closed before run reached a terminal state".into());
                    return HostOutcome {
                        output: outputs.join("\n"),
                        exit_code: 1,
                    };
                }
                Err(_) => {
                    outputs.push("run timed out waiting for a terminal state".into());
                    return HostOutcome {
                        output: outputs.join("\n"),
                        exit_code: 1,
                    };
                }
            }
        };
        outputs.push(format!("run {run_id} finished: {terminal_state:?}"));

        // 6) `--serve` 保持服务直到信号。
        if args.serve {
            match tokio::signal::ctrl_c().await {
                Ok(()) => {
                    self.dispatch(ServiceOperation::Shutdown);
                    outputs.push("serve kept running after the run; shutdown requested".into());
                }
                Err(error) => {
                    outputs.push(format!("failed to listen for Ctrl-C: {error}"));
                    return HostOutcome {
                        output: outputs.join("\n"),
                        exit_code: 1,
                    };
                }
            }
        }

        // 退出策略：无活跃 Run + 无 GUI 连接 + 无后台任务才退出。
        let active = self.service.router().supervisor().stats().active;
        if active > 0 {
            outputs.push(format!(
                "{active} run(s) still active; keeping process alive"
            ));
        }
        HostOutcome {
            output: outputs.join("\n"),
            exit_code: 0,
        }
    }

    /// watch：从 Event Hub 订阅并流式渲染，直到 Ctrl-C。
    async fn watch(&self, format: OutputFormat) -> HostOutcome {
        let mut subscription = self.hub.subscribe();
        let mut outputs = Vec::new();
        loop {
            tokio::select! {
                signal = tokio::signal::ctrl_c() => {
                    if signal.is_err() {
                        return HostOutcome {
                            output: outputs.join("\n"),
                            exit_code: 1,
                        };
                    }
                    break;
                }
                event = subscription.recv() => match event {
                    Ok(envelope) => outputs.push(render_event(&envelope, format)),
                    Err(HubError::Lagged { .. } | HubError::Empty | HubError::ReplayUnavailable { .. }) => continue,
                    Err(HubError::Closed) => break,
                },
            }
        }
        HostOutcome {
            output: outputs.join("\n"),
            exit_code: 0,
        }
    }

    /// 交互模式：普通命令行 REPL。
    async fn shell(&self, format: OutputFormat) -> HostOutcome {
        let ready = self.legacy(ServiceOperation::Shell, format);
        let mut outputs = vec![ready.output.clone()];
        let mut watching = false;
        let mut watch_subscription: Option<subscription_hub::HubSubscription> = None;
        let mut current_session: Option<SessionId> = None;
        let mut quit_armed = false;

        for line in io::stdin().lock().lines() {
            // /watch 开启时先排空已到达的事件。
            if watching {
                if let Some(subscription) = watch_subscription.as_mut() {
                    while let Ok(envelope) = subscription.try_recv() {
                        outputs.push(render_event(&envelope, format));
                    }
                }
            }
            let Ok(line) = line else {
                break;
            };
            let command = line.trim();
            match command {
                "" => {}
                "/quit" | "/exit" => {
                    let active = self.service.router().supervisor().stats().active;
                    if active > 0 && !quit_armed {
                        outputs.push(format!(
                            "{active} run(s) still active; type /quit again to force exit"
                        ));
                        quit_armed = true;
                        continue;
                    }
                    break;
                }
                "/help" => outputs.push(SHELL_HELP.into()),
                "/status" => outputs.push(render(&self.dispatch(ServiceOperation::Status), format)),
                "/workspaces" => outputs.push(self.query_text(
                    "workspaces",
                    AppQuery::WorkspaceList,
                    "workspaces",
                    format,
                )),
                "/sessions" => outputs.push(self.snapshot_section_text("sessions", format)),
                "/watch" => {
                    watching = !watching;
                    watch_subscription = if watching {
                        Some(self.hub.subscribe())
                    } else {
                        None
                    };
                    outputs.push(if watching {
                        "event watch on (toggle with /watch)".into()
                    } else {
                        "event watch off".into()
                    });
                }
                "/connect" => outputs.push(
                    "connect: GUI endpoint will be available with the GUI Server (P13-4)".into(),
                ),
                _ if command.starts_with("/run") => {
                    let prompt = command.trim_start_matches("/run").trim();
                    if prompt.is_empty() {
                        outputs.push("usage: /run <prompt>".into());
                        continue;
                    }
                    match self.shell_run(prompt, &mut current_session).await {
                        Ok(output) => outputs.push(output),
                        Err(message) => outputs.push(message),
                    }
                }
                _ if command.starts_with("/cancel") => {
                    let run_id = command.trim_start_matches("/cancel").trim();
                    if run_id.is_empty() {
                        outputs.push("usage: /cancel <run_id>".into());
                        continue;
                    }
                    outputs.push(
                        self.run_control(
                            RunCommand::Cancel {
                                run_id: run_id.into(),
                            },
                            format,
                        )
                        .output,
                    );
                }
                _ if command.starts_with("/approve") => {
                    let parts: Vec<&str> = command
                        .trim_start_matches("/approve")
                        .split_whitespace()
                        .collect();
                    if parts.len() != 3 {
                        outputs.push(
                            "usage: /approve <run_id> <tool_call_id> approve|approve-run|deny|cancel"
                                .into(),
                        );
                        continue;
                    }
                    let decision = match parts[2] {
                        "approve" => ApprovalDecision::ApproveOnce,
                        "approve-run" => ApprovalDecision::ApproveForRun,
                        "deny" => ApprovalDecision::Deny,
                        "cancel" => ApprovalDecision::Cancel,
                        _ => {
                            outputs.push("decision must be approve|approve-run|deny|cancel".into());
                            continue;
                        }
                    };
                    let response = self.dispatch_envelope(AppCommand::ToolApprove {
                        run_id: RunId::from(parts[0]),
                        tool_call_id: agent_domain::ToolCallId::from(parts[1]),
                        decision,
                    });
                    outputs.push(self.envelope_outcome("approval", response, format).output);
                }
                other => outputs.push(render(
                    &self.dispatch(placeholder("shell", vec![other.to_owned()])),
                    format,
                )),
            }
        }
        HostOutcome {
            output: outputs.join("\n"),
            exit_code: 0,
        }
    }

    /// 系统服务模式：install / start / stop（默认 dry-run）。
    fn service_mode(&self, command: ServiceCommand, format: OutputFormat) -> HostOutcome {
        let (action, apply) = match command {
            ServiceCommand::Install { apply } => ("install", apply),
            ServiceCommand::Start { apply } => ("start", apply),
            ServiceCommand::Stop { apply } => ("stop", apply),
        };
        let name = self.service_name();
        let plan = self.service_plan(action);
        let applied = if apply {
            match self.execute_service_action(action) {
                Ok(()) => Some(true),
                Err(error) => {
                    if format == OutputFormat::Json {
                        let value = serde_json::json!({
                            "service": name,
                            "action": action,
                            "dry_run": false,
                            "plan": plan,
                            "applied": false,
                            "error": error,
                        });
                        return HostOutcome {
                            output: serde_json::to_string(&value).expect("service JSON"),
                            exit_code: 1,
                        };
                    }
                    return HostOutcome {
                        output: format!("{plan}\napply failed: {error}"),
                        exit_code: 1,
                    };
                }
            }
        } else {
            None
        };

        if format == OutputFormat::Json {
            let value = serde_json::json!({
                "service": name,
                "action": action,
                "dry_run": applied.is_none(),
                "plan": plan,
                "applied": applied,
            });
            HostOutcome {
                output: serde_json::to_string(&value).expect("service JSON"),
                exit_code: 0,
            }
        } else {
            let status = match applied {
                Some(true) => "applied".to_string(),
                Some(false) => "apply failed".to_string(),
                None => {
                    "dry-run: no system changes were made (pass --apply to execute)".to_string()
                }
            };
            HostOutcome {
                output: format!("{plan}\n{status}"),
                exit_code: 0,
            }
        }
    }

    // ---------- 信封路由 ----------

    fn run_control(&self, command: RunCommand, format: OutputFormat) -> HostOutcome {
        let (envelope_command, kind) = match command {
            RunCommand::Cancel { run_id } => (
                AppCommand::RunCancel {
                    run_id: RunId::from(run_id),
                },
                "run.cancel",
            ),
            RunCommand::Retry { run_id } => (
                AppCommand::RunRetry {
                    run_id: RunId::from(run_id),
                },
                "run.retry",
            ),
        };
        let response = self.dispatch_envelope(envelope_command);
        self.envelope_outcome(kind, response, format)
    }

    fn ensure_workspace(&self, path: Option<&str>) -> Result<WorkspaceId, String> {
        let path = path.map(str::to_string);
        // 已有 workspace 中按 root 路径复用。
        if let AppResponse::Data(value) = &self.dispatch_query(AppQuery::WorkspaceList).response {
            if let Some(workspaces) = value.as_array() {
                for workspace in workspaces {
                    let matches = match (
                        path.as_ref(),
                        workspace.get("roots").and_then(Value::as_array),
                    ) {
                        (None, _) => true,
                        (Some(path), Some(roots)) => roots.iter().any(|root| {
                            root.get("path").and_then(Value::as_str) == Some(path.as_str())
                        }),
                        (Some(_), None) => false,
                    };
                    if matches {
                        return Ok(WorkspaceId::from(
                            workspace
                                .get("id")
                                .and_then(Value::as_str)
                                .unwrap_or_default(),
                        ));
                    }
                }
            }
        }
        let root_path = path.unwrap_or_else(|| {
            std::env::current_dir()
                .map(|path| path.to_string_lossy().into_owned())
                .unwrap_or_else(|_| ".".into())
        });
        match self
            .dispatch_envelope(AppCommand::WorkspaceAdd { root_path })
            .response
        {
            AppResponse::Data(value) => Ok(WorkspaceId::from(
                value.get("id").and_then(Value::as_str).unwrap_or_default(),
            )),
            AppResponse::Error(context) => Err(context.message),
            other => Err(format!("unexpected workspace add response: {other:?}")),
        }
    }

    /// shell 的 /run：异步启动（不阻塞 REPL），返回 run_id 供 /watch /status。
    async fn shell_run(
        &self,
        prompt: &str,
        current_session: &mut Option<SessionId>,
    ) -> Result<String, String> {
        let session_id = match current_session {
            Some(session_id) => session_id.clone(),
            None => {
                let workspace_id = self.ensure_workspace(None)?;
                let session_id = match self
                    .dispatch_envelope(AppCommand::SessionCreate {
                        workspace_id,
                        title: Some("shell".into()),
                    })
                    .response
                {
                    AppResponse::Data(value) => SessionId::from(
                        value
                            .get("session_id")
                            .and_then(Value::as_str)
                            .unwrap_or_default(),
                    ),
                    AppResponse::Error(context) => return Err(context.message),
                    other => return Err(format!("unexpected session create response: {other:?}")),
                };
                *current_session = Some(session_id.clone());
                session_id
            }
        };
        let response = self.dispatch_envelope(AppCommand::RunStart {
            session_id,
            user_message: prompt.to_string(),
            model: None,
        });
        match response.response {
            AppResponse::Accepted { .. } => {
                let run_id = self
                    .service
                    .router()
                    .last_started_run()
                    .unwrap_or_else(|| RunId::from(""));
                Ok(format!("run started: {run_id} (use /watch or /status)"))
            }
            AppResponse::Error(context) => Err(context.message),
            other => Err(format!("unexpected run start response: {other:?}")),
        }
    }

    fn query_text(
        &self,
        kind: &str,
        query: AppQuery,
        section: &str,
        format: OutputFormat,
    ) -> String {
        let response = self.dispatch_query(query);
        match (&response.response, format) {
            (AppResponse::Data(value), OutputFormat::Text) => {
                // WorkspaceList 直接返回数组；其余查询按 section 字段取列表。
                let items = value
                    .as_array()
                    .or_else(|| value.get(section).and_then(Value::as_array));
                match items {
                    Some(items) if items.is_empty() => format!("{kind}: none"),
                    Some(items) => {
                        let lines: Vec<String> = items
                            .iter()
                            .map(|item| serde_json::to_string(item).expect("item JSON"))
                            .collect();
                        format!("{kind}:\n{}", lines.join("\n"))
                    }
                    _ => serde_json::to_string(value).expect("query JSON"),
                }
            }
            _ => self.envelope_outcome(kind, response, format).output,
        }
    }

    fn snapshot_section_text(&self, section: &str, format: OutputFormat) -> String {
        let response = self.dispatch_query(AppQuery::SnapshotFetch);
        match (&response.response, format) {
            (AppResponse::Data(value), OutputFormat::Text) => {
                let items = value.get(section).and_then(Value::as_array);
                match items {
                    Some(items) if items.is_empty() => format!("{section}: none"),
                    Some(items) => {
                        let lines: Vec<String> = items
                            .iter()
                            .map(|item| serde_json::to_string(item).expect("item JSON"))
                            .collect();
                        format!("{section}:\n{}", lines.join("\n"))
                    }
                    _ => serde_json::to_string(value).expect("snapshot JSON"),
                }
            }
            _ => self.envelope_outcome("snapshot", response, format).output,
        }
    }

    fn dispatch_envelope(&self, command: AppCommand) -> AppResponseEnvelope {
        self.service.dispatch_envelope(AppCommandEnvelope {
            api_version: API_VERSION,
            command_id: CommandId::from(self.next_id("cmd")),
            source: CommandSource::LocalCli {
                terminal_session_id: None,
            },
            identity: ActorIdentity::LocalUser {
                actor_id: ActorId::from("local-cli"),
                display_name: None,
            },
            expected_revision: None,
            idempotency_key: None,
            issued_at: now_timestamp(),
            command,
        })
    }

    fn dispatch_query(&self, query: AppQuery) -> AppResponseEnvelope {
        self.service.dispatch_query(AppQueryEnvelope {
            api_version: API_VERSION,
            request_id: QueryId::from(self.next_id("query")),
            source: CommandSource::LocalCli {
                terminal_session_id: None,
            },
            identity: ActorIdentity::LocalUser {
                actor_id: ActorId::from("local-cli"),
                display_name: None,
            },
            issued_at: now_timestamp(),
            query,
        })
    }

    fn dispatch(&self, operation: ServiceOperation) -> ServiceResponse {
        self.service.dispatch(ServiceRequest {
            source: CommandSource::LocalCli {
                terminal_session_id: None,
            },
            operation,
        })
    }

    fn legacy(&self, operation: ServiceOperation, format: OutputFormat) -> HostOutcome {
        let response = self.dispatch(operation);
        HostOutcome {
            output: render(&response, format),
            exit_code: i32::from(!response.ok),
        }
    }

    fn error_outcome(&self, kind: &str, message: &str, format: OutputFormat) -> HostOutcome {
        self.envelope_outcome(
            kind,
            AppResponseEnvelope {
                api_version: API_VERSION,
                request_id: QueryId::from(self.next_id("error")),
                responded_at: now_timestamp(),
                response: AppResponse::Error(ErrorContext {
                    category: agent_domain::ErrorCategory::Internal,
                    message: message.into(),
                    retryable: false,
                    retry_after_ms: None,
                    diagnostics: BTreeMap::new(),
                }),
            },
            format,
        )
    }

    /// 把信封响应渲染为统一输出（JSON 单行、Text 友好）。
    fn envelope_outcome(
        &self,
        kind: &str,
        response: AppResponseEnvelope,
        format: OutputFormat,
    ) -> HostOutcome {
        let ok = matches!(
            response.response,
            AppResponse::Accepted { .. } | AppResponse::Data(_) | AppResponse::Artifact { .. }
        );
        let message = match &response.response {
            AppResponse::Error(context) => context.message.clone(),
            _ => format!("{kind} completed"),
        };
        let data = serde_json::to_value(&response).unwrap_or(Value::Null);
        let service_response = ServiceResponse {
            ok,
            kind: kind.into(),
            message,
            data,
        };
        HostOutcome {
            output: render(&service_response, format),
            exit_code: i32::from(!ok),
        }
    }

    fn placeholder_for_command(&self, command: Command, format: OutputFormat) -> HostOutcome {
        let name = match command {
            Command::Workspace(_) => "workspace",
            Command::Session(_) => "session",
            Command::Approval(_) => "approval",
            Command::Gui(_) => "gui",
            Command::Provider(_) => "provider",
            Command::Auth(_) => "auth",
            Command::Plugin(_) => "plugin",
            Command::Mcp(_) => "mcp",
            Command::Models(_) => "models",
            Command::Tools(_) => "tools",
            Command::ImportPi { .. } => "import-pi",
            Command::Benchmark => "benchmark",
            Command::Serve(_)
            | Command::Shell
            | Command::Run(_)
            | Command::Watch
            | Command::Status
            | Command::Shutdown
            | Command::Doctor
            | Command::Service(_)
            | Command::Remote(_) => unreachable!("handled before placeholder mapping"),
        };
        let response = self.dispatch(placeholder(name, Vec::new()));
        HostOutcome {
            output: render(&response, format),
            exit_code: i32::from(!response.ok),
        }
    }

    fn next_id(&self, prefix: &str) -> String {
        format!(
            "{}-{}-{}",
            prefix,
            self.instance,
            self.next_command_id.fetch_add(1, Ordering::SeqCst) + 1
        )
    }

    // ---------- 系统服务 ----------

    fn service_name(&self) -> String {
        if self.instance == "default" {
            "pawork".into()
        } else {
            format!("pawork-{}", self.instance)
        }
    }

    fn service_plan(&self, action: &str) -> String {
        let exe = std::env::current_exe()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|_| "pawork".into());
        let name = self.service_name();
        match action {
            "install" => {
                let definition = self.install_definition(&exe, &name);
                let activation = self.activation_command(action, &name);
                format!(
                    "install plan for service '{name}':\n{definition}\nthen activate:\n  {activation}"
                )
            }
            "start" | "stop" => format!(
                "{} plan for service '{name}':\n  {}",
                action,
                self.activation_command(action, &name)
            ),
            _ => format!("unsupported service action: {action}"),
        }
    }

    fn install_definition(&self, exe: &str, name: &str) -> String {
        #[cfg(target_os = "windows")]
        {
            format!(
                "sc create {name} binPath= \"\\\"{exe}\\\" serve --instance {}\" start= auto displayname= \"Pawork Core\"",
                self.instance
            )
        }
        #[cfg(target_os = "macos")]
        {
            format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
                 <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
                 <plist version=\"1.0\">\n\
                 <dict>\n\
                 \x20 <key>Label</key>\n\
                 \x20 <string>{name}</string>\n\
                 \x20 <key>ProgramArguments</key>\n\
                 \x20 <array>\n\
                 \x20\x20 <string>{exe}</string>\n\
                 \x20\x20 <string>serve</string>\n\
                 \x20\x20 <string>--instance</string>\n\
                 \x20\x20 <string>{}</string>\n\
                 \x20 </array>\n\
                 \x20 <key>RunAtLoad</key>\n\
                 \x20 <true/>\n\
                 </dict>\n\
                 </plist>",
                self.instance
            )
        }
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            format!(
                "[Unit]\n\
                 Description=Pawork Core ({})\n\
                 After=network.target\n\
                 \n\
                 [Service]\n\
                 ExecStart={} serve --instance {}\n\
                 Restart=on-failure\n\
                 \n\
                 [Install]\n\
                 WantedBy=multi-user.target",
                self.instance, exe, self.instance
            )
        }
        #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
        {
            "unsupported platform for service install".to_string()
        }
    }

    fn activation_command(&self, action: &str, name: &str) -> String {
        #[cfg(target_os = "windows")]
        {
            format!("sc {action} {name}")
        }
        #[cfg(target_os = "macos")]
        {
            format!("launchctl {action} {name}")
        }
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            format!("systemctl {action} {name}")
        }
        #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
        {
            format!("unsupported platform for service {action}")
        }
    }

    /// `--apply`：真正执行服务操作（install 写入单元/plist 或执行 sc create）。
    fn execute_service_action(&self, action: &str) -> Result<(), String> {
        let name = self.service_name();
        if action == "install" {
            let exe = std::env::current_exe()
                .map(|path| path.display().to_string())
                .map_err(|error| format!("cannot resolve current executable: {error}"))?;
            #[cfg(target_os = "windows")]
            {
                run_program(
                    "sc",
                    &[
                        "create",
                        &name,
                        "binPath=",
                        &format!("\"{exe}\" serve --instance {}", self.instance),
                        "start=",
                        "auto",
                        "displayname=",
                        "Pawork Core",
                    ],
                )
            }
            #[cfg(target_os = "macos")]
            {
                let plist = self.install_definition(&exe, &name);
                let path = home_dir()?
                    .join("Library/LaunchAgents")
                    .join(format!("{name}.plist"));
                write_file(&path, &plist)
            }
            #[cfg(all(unix, not(target_os = "macos")))]
            {
                let unit = self.install_definition(&exe, &name);
                let path =
                    std::path::PathBuf::from("/etc/systemd/system").join(format!("{name}.service"));
                write_file(&path, &unit)?;
                let _ = run_program("systemctl", &["daemon-reload"]);
                Ok(())
            }
            #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
            {
                Err("unsupported platform for service install".into())
            }
        } else {
            #[cfg(target_os = "windows")]
            {
                run_program("sc", &[action, &name])
            }
            #[cfg(target_os = "macos")]
            {
                run_program("launchctl", &[action, &name])
            }
            #[cfg(all(unix, not(target_os = "macos")))]
            {
                run_program("systemctl", &[action, &name])
            }
            #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
            {
                Err(format!("unsupported platform for service {action}"))
            }
        }
    }
}

fn placeholder(command: &str, arguments: Vec<String>) -> ServiceOperation {
    ServiceOperation::Placeholder {
        command: command.into(),
        arguments,
    }
}

fn now_timestamp() -> Timestamp {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0);
    Timestamp::from_unix_millis(millis)
}

fn terminal(state: &RunState) -> bool {
    matches!(
        state,
        RunState::Completed | RunState::Cancelled | RunState::Failed | RunState::Interrupted
    )
}

fn run_program(program: &str, args: &[&str]) -> Result<(), String> {
    let output = std::process::Command::new(program)
        .args(args)
        .output()
        .map_err(|error| format!("failed to run {program}: {error}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "{program} {} exited with {}: {}",
            args.join(" "),
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn write_file(path: &std::path::Path, content: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
    }
    std::fs::write(path, content)
        .map_err(|error| format!("cannot write {}: {error}", path.display()))
}

#[cfg(target_os = "macos")]
fn home_dir() -> Result<std::path::PathBuf, String> {
    std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .ok_or_else(|| "HOME is not set".to_string())
}

const SHELL_HELP: &str = "\
commands:
  /run <prompt>            start a run in the current session
  /cancel <run_id>         cancel a run
  /approve <run_id> <tool_call_id> approve|approve-run|deny|cancel
  /sessions                list sessions
  /workspaces              list workspaces
  /status                  core status
  /watch                   toggle live event streaming
  /connect                 GUI endpoint (GUI Server, P13-4)
  /quit | /exit            leave the shell";

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    use core_runtime::CoreRuntime;
    use test_support::{MockProvider, MockScript};
    use transport_remote_placeholder::MockRemoteTransportProvider;

    #[tokio::test]
    async fn doctor_uses_direct_app_service_route() {
        let service = Arc::new(AppService::new("test"));
        let host = CliHost::new(Arc::clone(&service));
        let cli = Cli::try_parse_from(["pawork", "--json", "doctor"]).expect("parse");
        let outcome = host.execute(cli).await;
        assert_eq!(outcome.exit_code, 0);
        assert_eq!(service.source_count("local_cli"), 1);
        let output: serde_json::Value = serde_json::from_str(&outcome.output).expect("JSON");
        assert_eq!(output["kind"], "doctor");
    }

    #[tokio::test]
    async fn run_mode_executes_end_to_end_with_mock_provider() {
        let runtime = CoreRuntime::new("cli-host-run-test");
        runtime.register_provider(Arc::new(MockProvider::new(
            MockScript::new().text("hello from cli-host").complete(),
        )));
        let host = CliHost::with_hub(Arc::clone(runtime.service()), Arc::clone(runtime.hub()));
        let cli = Cli::try_parse_from([
            "pawork",
            "run",
            "--workspace",
            ".",
            "--prompt",
            "do something",
        ])
        .expect("parse");
        let outcome = host.execute(cli).await;
        assert_eq!(outcome.exit_code, 0, "output: {}", outcome.output);
        assert!(
            outcome.output.contains("finished: Completed"),
            "output: {}",
            outcome.output
        );
        assert!(outcome.output.contains("hello from cli-host"));
        // 全局序列经 Event Hub 重写后连续。
        let events = runtime
            .hub()
            .replay(core_api::GlobalSequence(1), None)
            .expect("replay");
        for pair in events.windows(2) {
            assert!(
                pair[1]
                    .global_sequence
                    .is_immediately_after(pair[0].global_sequence),
                "hub global sequence must be contiguous in replay"
            );
        }
    }

    #[tokio::test]
    async fn run_mode_without_provider_fails_gracefully() {
        let runtime = CoreRuntime::new("cli-host-no-provider");
        let host = CliHost::with_hub(Arc::clone(runtime.service()), Arc::clone(runtime.hub()));
        let cli = Cli::try_parse_from([
            "pawork",
            "--json",
            "run",
            "--workspace",
            ".",
            "--prompt",
            "hello",
        ])
        .expect("parse");
        let outcome = host.execute(cli).await;
        assert_ne!(outcome.exit_code, 0);
        let output: serde_json::Value = serde_json::from_str(&outcome.output).expect("JSON");
        assert_eq!(output["kind"], "run");
        assert_eq!(output["ok"], false);
        assert!(output["message"]
            .as_str()
            .expect("message")
            .contains("provider"));
    }

    #[tokio::test]
    async fn run_retry_routes_through_envelope() {
        let service = Arc::new(AppService::new("retry-test"));
        let host = CliHost::new(service);
        let cli = Cli::try_parse_from(["pawork", "run", "retry", "run-1"]).expect("parse");
        let outcome = host.execute(cli).await;
        assert_ne!(outcome.exit_code, 0);
        assert!(outcome.output.contains("not found"));
    }

    #[test]
    fn service_install_defaults_to_dry_run() {
        let service = Arc::new(AppService::new("default"));
        let host = CliHost::new(service);
        let cli = Cli::try_parse_from(["pawork", "service", "install"]).expect("parse");
        let Command::Service(service_command) = cli.command else {
            panic!("expected service command");
        };
        let outcome = host.service_mode(service_command.command, OutputFormat::Text);
        assert_eq!(outcome.exit_code, 0);
        assert!(outcome.output.contains("pawork"));
        assert!(outcome.output.contains("dry-run"));
    }

    #[tokio::test]
    async fn remote_publish_and_unpublish_with_mock_provider() {
        let service = Arc::new(AppService::new("remote-test"));
        let mut host = CliHost::new(Arc::clone(&service));
        let provider: Arc<dyn RemoteGuiTransportProvider> =
            Arc::new(MockRemoteTransportProvider::default());
        host.attach_remote_provider(provider);

        let cli = Cli::try_parse_from(["pawork", "--json", "remote", "publish", "--name", "edge"])
            .expect("parse publish");
        let outcome = host.execute(cli).await;
        assert_eq!(outcome.exit_code, 0, "output: {}", outcome.output);
        let value: serde_json::Value = serde_json::from_str(&outcome.output).expect("JSON");
        assert_eq!(value["ok"], true);
        assert_eq!(value["kind"], "remote");
        assert_eq!(value["data"]["action"], "publish");
        assert_eq!(value["data"]["adapter"], "mock");
        assert_eq!(value["data"]["status"], "published");
        let handle_id = value["data"]["handle_id"].as_str().expect("handle id");
        assert_eq!(handle_id, "edge-0");
        assert_eq!(value["data"]["endpoint"]["kind"], "remote");
        assert_eq!(value["data"]["endpoint"]["adapter"], "mock");
        assert_eq!(value["data"]["endpoint"]["address"], "mock://edge-0");

        let cli = Cli::try_parse_from([
            "pawork",
            "--json",
            "remote",
            "unpublish",
            "--handle",
            handle_id,
        ])
        .expect("parse unpublish");
        let outcome = host.execute(cli).await;
        assert_eq!(outcome.exit_code, 0, "output: {}", outcome.output);
        let value: serde_json::Value = serde_json::from_str(&outcome.output).expect("JSON");
        assert_eq!(value["data"]["action"], "unpublish");
        assert_eq!(value["data"]["handle_id"], "edge-0");
        assert_eq!(value["data"]["status"], "unpublished");
    }

    #[tokio::test]
    async fn remote_publish_text_mode_prints_endpoint_and_status() {
        let service = Arc::new(AppService::new("remote-text"));
        let mut host = CliHost::new(service);
        host.attach_remote_provider(Arc::new(MockRemoteTransportProvider::default()));
        let cli =
            Cli::try_parse_from(["pawork", "remote", "publish", "--name", "edge"]).expect("parse");
        let outcome = host.execute(cli).await;
        assert_eq!(outcome.exit_code, 0, "output: {}", outcome.output);
        assert!(
            outcome
                .output
                .contains("remote endpoint published via adapter 'mock'"),
            "output: {}",
            outcome.output
        );
        assert!(outcome.output.contains("mock://edge-0"));
    }

    #[tokio::test]
    async fn remote_without_provider_returns_structured_error() {
        let service = Arc::new(AppService::new("remote-test"));
        let host = CliHost::new(service);
        let cli = Cli::try_parse_from(["pawork", "--json", "remote", "publish"]).expect("parse");
        let outcome = host.execute(cli).await;
        assert_ne!(outcome.exit_code, 0);
        let value: serde_json::Value = serde_json::from_str(&outcome.output).expect("JSON");
        assert_eq!(value["ok"], false);
        assert_eq!(value["kind"], "remote");
        assert!(
            value["message"]
                .as_str()
                .expect("message")
                .contains("provider"),
            "output: {}",
            outcome.output
        );
    }

    #[tokio::test]
    async fn remote_unpublish_unknown_handle_returns_structured_error() {
        let service = Arc::new(AppService::new("remote-test"));
        let mut host = CliHost::new(service);
        host.attach_remote_provider(Arc::new(MockRemoteTransportProvider::default()));
        let cli = Cli::try_parse_from([
            "pawork",
            "--json",
            "remote",
            "unpublish",
            "--handle",
            "never-published",
        ])
        .expect("parse");
        let outcome = host.execute(cli).await;
        assert_ne!(outcome.exit_code, 0);
        let value: serde_json::Value = serde_json::from_str(&outcome.output).expect("JSON");
        assert_eq!(value["ok"], false);
        assert_eq!(value["data"]["action"], "unpublish");
        assert!(
            value["message"]
                .as_str()
                .expect("message")
                .contains("unknown remote publish handle"),
            "output: {}",
            outcome.output
        );
    }
}
