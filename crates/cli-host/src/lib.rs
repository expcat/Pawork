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
    ActorId, CommandId, ErrorContext, ModelId, ProviderId, QueryId, RunId, SessionId, TenantId,
    Timestamp, WorkspaceId,
};
use app_service::{AppService, ServiceOperation, ServiceRequest, ServiceResponse};
use cli_command::{
    AcpCommand, Cli, Command, HeadlessArgs, RemoteCommand, RunArgs, RunCommand, ServiceCommand,
    UsageArgs, UsageUnit, UsageWindow,
};
use cli_renderer::{render, render_event, OutputFormat};
use client_adapter_api::SessionRegistry;
use core_api::{
    ActorIdentity, AppCommand, AppCommandEnvelope, AppQuery, AppQueryEnvelope, AppResponse,
    AppResponseEnvelope, ApprovalDecision, CommandSource, RunState, API_VERSION,
};
use core_api::{
    QuotaConfidence, QuotaMeasure, QuotaOverviewQuery, QuotaOverviewView, QuotaReset, QuotaUnit,
    QuotaWindow, WindowReadView,
};
use serde_json::Value;
use session_store::{SessionStore, SqliteClientSessionRegistryStore};
use subscription_hub::{EventHub, HubError};
use transport_remote_placeholder::{
    RemoteGuiTransportProvider, RemotePublishRequest, TransportEndpoint,
};

mod acp;
mod headless;

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

    /// 绑定一个已发布的远程端点（P17-11：publish 后由同一 Core 实际监听）。
    /// 实现应把监听器与 accept 循环按 `handle_id` 登记，供 [`Self::close_remote`]
    /// 关闭；不支持远程端点的宿主返回错误。
    fn bind_remote(&self, handle_id: &str, endpoint: &TransportEndpoint) -> Result<(), String> {
        let _ = (handle_id, endpoint);
        Err("this gui server host does not support remote endpoints".into())
    }

    /// 关闭一个已绑定远程端点的监听器（unpublish / revoke 前调用）。
    /// 未知 handle 返回错误；不支持远程端点的宿主无操作。
    fn close_remote(&self, handle_id: &str) -> Result<(), String> {
        let _ = handle_id;
        Ok(())
    }
}

/// CLI 宿主：持有同一进程内的 AppService 与 EventHub，按命令路由到
/// 四种运行模式或信封命令。
pub struct CliHost {
    service: Arc<AppService>,
    hub: Arc<EventHub>,
    instance: String,
    gui_server: Option<Arc<dyn GuiServerHost>>,
    remote_provider: Option<Arc<dyn RemoteGuiTransportProvider>>,
    session_store: Option<Arc<SessionStore>>,
    acp_registry: Option<Arc<SessionRegistry>>,
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
            session_store: None,
            acp_registry: None,
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

    /// 注入 SessionStore（headless 模式的 compat 持久化入口；未注入时
    /// `compat_import` / `compat_history` 返回显式 `UnsupportedCapability`）。
    pub fn attach_session_store(&mut self, store: Arc<SessionStore>) {
        self.session_store = Some(store);
    }

    /// 注入 ACP Session Registry（`pawork acp serve` 库调用方；真实二进制
    /// 由 main 构造 SQLite-backed registry 后直接走 [`CliHost::run_acp_stdio`]）。
    pub fn attach_acp_registry(&mut self, registry: Arc<SessionRegistry>) {
        self.acp_registry = Some(registry);
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
            Command::Acp(acp) => self.acp_mode(acp.command, format).await,
            Command::Headless(args) => self.headless_mode(args, format).await,
            Command::Watch => self.watch(format).await,
            Command::Status => self.legacy(ServiceOperation::Status, format),
            Command::Shutdown => self.legacy(ServiceOperation::Shutdown, format),
            Command::Doctor => self.legacy(ServiceOperation::Doctor, format),
            Command::Service(service) => self.service_mode(service.command, format),
            Command::Remote(remote) => self.remote_mode(remote.command, format).await,
            Command::Usage(args) => self.usage_mode(args, format),
            other => self.placeholder_for_command(other, format),
        }
    }

    // ---------- Remote Transport（P13-6 占位 Adapter） ----------

    /// 远程 GUI 端点生命周期：publish / unpublish。
    ///
    /// 无 Provider 时返回结构化错误；发布成功输出 endpoint 与状态，JSON 模式
    /// 携带 handle_id / endpoint 供 unpublish / revoke 使用。publish 成功后由
    /// 已装配的 GUI Server 宿主实际 bind 并接受连接；unpublish / revoke 先
    /// 关闭宿主侧监听器，再撤销端点（凭证失效、连接断开）。
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
                        let bound = match &self.gui_server {
                            Some(host) => match host.bind_remote(&handle.id, &handle.endpoint) {
                                Ok(()) => true,
                                Err(error) => {
                                    // 回滚发布，避免留下未被监听的悬挂端点。
                                    let _ = provider.unpublish(&handle.id).await;
                                    return self.remote_failure(
                                        "publish",
                                        &format!("bind failed: {error}"),
                                        format,
                                    );
                                }
                            },
                            None => {
                                // 远程端点没有同一 Core 的 GuiServer bind/accept
                                // 就不可发布：立即回滚 listener 与 endpoint credential。
                                let rollback = provider.unpublish(&handle.id).await;
                                let message = match rollback {
                                    Ok(()) => "no gui server host is attached; publish rolled back"
                                        .to_string(),
                                    Err(error) => format!(
                                        "no gui server host is attached; publish rollback failed: {error}"
                                    ),
                                };
                                return self.remote_failure("publish", &message, format);
                            }
                        };
                        let address = match &handle.endpoint {
                            TransportEndpoint::Remote { address, .. } => address.clone(),
                            other => format!("{other:?}"),
                        };
                        let response = ServiceResponse {
                            ok: true,
                            kind: "remote".into(),
                            message: format!(
                                "remote endpoint published via adapter '{}' (handle {}): {}{}",
                                description.adapter,
                                handle.id,
                                address,
                                if bound { " (bound)" } else { " (not bound)" },
                            ),
                            data: serde_json::json!({
                                "action": "publish",
                                "adapter": description.adapter,
                                "handle_id": handle.id,
                                "endpoint": handle.endpoint,
                                "bound": bound,
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
            RemoteCommand::Unpublish { handle } => {
                if let Some(host) = &self.gui_server {
                    let _ = host.close_remote(&handle);
                }
                match provider.unpublish(&handle).await {
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
                }
            }
            RemoteCommand::Revoke { handle } => {
                if let Some(host) = &self.gui_server {
                    let _ = host.close_remote(&handle);
                }
                match provider.revoke(&handle).await {
                    Ok(()) => {
                        let response = ServiceResponse {
                            ok: true,
                            kind: "remote".into(),
                            message: format!("remote endpoint revoked (handle {handle})"),
                            data: serde_json::json!({
                                "action": "revoke",
                                "handle_id": handle,
                                "status": "revoked",
                            }),
                        };
                        HostOutcome {
                            output: render(&response, format),
                            exit_code: 0,
                        }
                    }
                    Err(error) => self.remote_failure("revoke", &error.to_string(), format),
                }
            }
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
            profile: None,
        });
        let AppResponseEnvelope {
            response: AppResponse::Accepted { run_id, .. },
            ..
        } = &response
        else {
            return self.envelope_outcome("run", response, format);
        };
        // RunStart 响应携带该命令确定启动的 run id（并发来源各自绑定，
        // 不依赖全局状态）。
        let run_id = run_id.clone().unwrap_or_else(|| RunId::from(""));

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

    // ---------- Headless（P17-8） ----------

    /// `headless` 模式：`--json-stdio` 开启 NDJSON 循环；未开启返回显式错误
    /// （不产生任何 TUI/CLI 文本输出）。
    async fn headless_mode(&self, args: HeadlessArgs, format: OutputFormat) -> HostOutcome {
        if args.json_stdio {
            // 真实二进制入口在 main 中直接调用 [`CliHost::run_headless_stdio`]
            // 以保持 stdout 纯净；库调用方走这里获得等价行为。
            let exit_code = self.run_headless_stdio().await;
            HostOutcome {
                output: String::new(),
                exit_code,
            }
        } else {
            self.error_outcome(
                "headless",
                "headless mode requires --json-stdio (the NDJSON protocol entry)",
                format,
            )
        }
    }

    /// 以 tokio stdin/stdout 运行 headless NDJSON 循环（`pawork headless
    /// --json-stdio` 的真实进程入口；stdout 只写 JSONL 帧）。
    pub async fn run_headless_stdio(&self) -> i32 {
        let stdin = tokio::io::BufReader::new(tokio::io::stdin());
        let stdout = tokio::io::stdout();
        match self.headless_loop(stdin, stdout).await {
            Ok(()) => 0,
            Err(error) => {
                eprintln!("headless: {error}");
                1
            }
        }
    }

    /// 以任意异步 reader/writer 运行 headless NDJSON 循环（进程入口与测试
    /// 共用同一实现）。
    pub async fn headless_loop<R, W>(&self, reader: R, writer: W) -> std::io::Result<()>
    where
        R: tokio::io::AsyncBufRead + Unpin,
        W: tokio::io::AsyncWrite + Unpin,
    {
        use headless_json::stdio::{run_loop, LoopConfig};

        let mut handler = headless::HeadlessHandler::new(
            Arc::clone(&self.service),
            Arc::clone(&self.hub),
            self.instance.clone(),
            self.session_store.clone(),
        );
        run_loop(reader, writer, LoopConfig::default(), &mut handler).await
    }

    // ---------- ACP Host（P17-7） ----------

    /// `acp` 模式：`serve` 启动 ACP stdio JSON-RPC 循环；未注入 registry 时
    /// 返回显式错误（真实二进制在 main 中直接走 [`CliHost::run_acp_stdio`]
    /// 以保持 stdout 纯净）。
    async fn acp_mode(&self, command: AcpCommand, format: OutputFormat) -> HostOutcome {
        match command {
            AcpCommand::Serve => {
                let Some(registry) = self.acp_registry.clone() else {
                    return self.error_outcome(
                        "acp",
                        "acp serve requires a session registry (attach via attach_acp_registry)",
                        format,
                    );
                };
                let exit_code = self.run_acp_loop_stdio(registry).await;
                HostOutcome {
                    output: String::new(),
                    exit_code,
                }
            }
        }
    }

    /// `pawork acp serve` 真实进程入口：用同一实例 SQLite SessionStore 构造
    /// SessionRegistry（复用 `SqliteClientSessionRegistryStore`，不私建
    /// ownership/credential 状态），然后运行 stdin/stdout JSON-RPC 循环；
    /// stdout 只写协议帧，诊断进 stderr。
    pub async fn run_acp_stdio(&self, store: SessionStore) -> i32 {
        let registry = match Self::acp_registry_from_store(store).await {
            Ok(registry) => registry,
            Err(error) => {
                eprintln!("acp serve: session registry unavailable: {error}");
                return 1;
            }
        };
        self.run_acp_loop_stdio(registry).await
    }

    /// 以 SQLite SessionStore 构造 SessionRegistry（P17-7 复用同一 Core 的
    /// instance SessionStore 作为 authoritative 记录源）。
    pub async fn acp_registry_from_store(
        store: SessionStore,
    ) -> Result<Arc<SessionRegistry>, String> {
        let registry_store = Arc::new(SqliteClientSessionRegistryStore::new(store));
        SessionRegistry::new(registry_store)
            .await
            .map(Arc::new)
            .map_err(|error| error.to_string())
    }

    async fn run_acp_loop_stdio(&self, registry: Arc<SessionRegistry>) -> i32 {
        // ACP 没有 workspace 登记方法：与 shell/run 模式一致，先把进程 cwd
        // 登记为 workspace，否则 `session/new` 的 cwd 解析必然失败。
        if let Err(error) = self.ensure_workspace(None) {
            eprintln!("acp serve: cannot resolve workspace: {error}");
            return 1;
        }
        let stdin = tokio::io::BufReader::new(tokio::io::stdin());
        let stdout = tokio::io::stdout();
        match self.acp_loop(stdin, stdout, registry).await {
            Ok(()) => 0,
            Err(error) => {
                eprintln!("acp serve: {error}");
                1
            }
        }
    }

    /// 以任意异步 reader/writer 运行 ACP stdio JSON-RPC 循环（进程入口与
    /// 测试共用同一实现；事件经共享 Event Hub 订阅，由调用方运行 EventPump）。
    pub async fn acp_loop<R, W>(
        &self,
        reader: R,
        writer: W,
        registry: Arc<SessionRegistry>,
    ) -> std::io::Result<()>
    where
        R: tokio::io::AsyncBufRead + Unpin,
        W: tokio::io::AsyncWrite + Unpin + Send + 'static,
    {
        acp::run_loop(
            Arc::clone(&self.service),
            Arc::clone(&self.hub),
            registry,
            reader,
            writer,
        )
        .await
    }

    // ---------- Usage（P14-8） ----------

    /// `pawork usage`：查询 typed QuotaOverview 并渲染（Text / JSON）。
    fn usage_mode(&self, args: UsageArgs, format: OutputFormat) -> HostOutcome {
        let query = usage_query_from_args(&args);
        let response = self.dispatch_query(AppQuery::QuotaOverview { query });
        match format {
            OutputFormat::Json => self.envelope_outcome("usage", response, format),
            OutputFormat::Text => HostOutcome {
                output: render_usage_text(&response),
                exit_code: i32::from(!matches!(response.response, AppResponse::Data(_))),
            },
        }
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
            profile: None,
        });
        match response.response {
            AppResponse::Accepted { run_id, .. } => {
                let run_id = run_id.unwrap_or_else(|| RunId::from(""));
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
            | Command::Acp(_)
            | Command::Headless(_)
            | Command::Watch
            | Command::Status
            | Command::Shutdown
            | Command::Doctor
            | Command::Service(_)
            | Command::Remote(_)
            | Command::Usage(_) => unreachable!("handled before placeholder mapping"),
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

// ---------- Usage（P14-8）参数 → 查询 / 文本渲染 ----------

/// 把 typed [`UsageArgs`] 直接映射为 typed [`QuotaOverviewQuery`]
/// （缺省 local/local/default；无效 window/unit 已在 clap 解析边界拒绝）。
fn usage_query_from_args(args: &UsageArgs) -> QuotaOverviewQuery {
    QuotaOverviewQuery {
        tenant_id: TenantId::new(args.tenant_or_default()),
        account_id: args.account_or_default(),
        provider_id: args.provider.as_deref().map(ProviderId::from),
        credential_id: args.credential.clone(),
        model_id: args.model.as_deref().map(ModelId::from),
        windows: args
            .window
            .iter()
            .copied()
            .map(usage_window_to_quota)
            .collect(),
        unit: args.unit.as_ref().map(usage_unit_to_quota),
    }
}

fn usage_window_to_quota(window: UsageWindow) -> QuotaWindow {
    match window {
        UsageWindow::Overall => QuotaWindow::Overall,
        UsageWindow::Rolling5h => QuotaWindow::Rolling5h,
        UsageWindow::Weekly => QuotaWindow::Weekly,
        UsageWindow::Monthly => QuotaWindow::Monthly,
    }
}

fn usage_unit_to_quota(unit: &UsageUnit) -> QuotaUnit {
    match unit {
        UsageUnit::Count => QuotaUnit::Count,
        UsageUnit::Token => QuotaUnit::Token,
        UsageUnit::Cost { currency } => QuotaUnit::Cost {
            currency: currency.clone(),
        },
    }
}

/// 把 QuotaOverview 响应渲染为人类可读文本：先反序列化为 typed
/// [`QuotaOverviewView`]，再按字段渲染；每个窗口一行 + 失败列表。
fn render_usage_text(response: &AppResponseEnvelope) -> String {
    let value = match &response.response {
        AppResponse::Data(value) => value,
        AppResponse::Error(context) => {
            return format!("usage: error: {}", context.message);
        }
        other => return format!("usage: unexpected response: {other:?}"),
    };
    let view: QuotaOverviewView = match serde_json::from_value(value.clone()) {
        Ok(view) => view,
        Err(error) => return format!("usage: cannot render quota overview: {error}"),
    };
    let header = format!(
        "usage for {tenant}/{account} (provider={provider}, cache={cache})",
        tenant = view.scope.tenant_id,
        account = view.scope.account_id,
        provider = view.scope.provider_id,
        cache = if view.from_cache { "hit" } else { "miss" },
    );
    let mut lines = vec![header];
    for entry in &view.windows {
        let window = window_label(entry.window);
        let line = match &entry.read {
            WindowReadView::Ok { snapshot, .. } => {
                let snapshot = snapshot.as_ref();
                format!(
                    "{window} {unit}: used={used} limit={limit} remaining={remaining} reset={reset} confidence={confidence} source={source} stale={stale}",
                    unit = unit_label(&snapshot.unit),
                    used = measure_label(snapshot.values.used),
                    limit = measure_label(snapshot.values.limit),
                    remaining = measure_label(snapshot.values.remaining),
                    reset = reset_label(&snapshot.reset),
                    confidence = confidence_label(snapshot.confidence),
                    source = snapshot.provenance.source,
                    stale = snapshot.provenance.stale,
                )
            }
            // 失败不一定都来自 adapter（如 scope 校验失败），文案不声称全是 adapter failure。
            WindowReadView::Failed { failures } => {
                format!("{window}: failed ({} failure(s))", failures.len())
            }
            WindowReadView::NoData => format!("{window}: no data"),
        };
        lines.push(line);
    }
    lines.join("\n")
}

fn window_label(window: QuotaWindow) -> &'static str {
    match window {
        QuotaWindow::Overall => "overall",
        QuotaWindow::Rolling5h => "rolling5h",
        QuotaWindow::Weekly => "weekly",
        QuotaWindow::Monthly => "monthly",
    }
}

fn unit_label(unit: &QuotaUnit) -> String {
    match unit {
        QuotaUnit::Count => "count".into(),
        QuotaUnit::Token => "token".into(),
        QuotaUnit::Cost { currency } => format!("cost:{currency}"),
    }
}

fn measure_label(measure: QuotaMeasure) -> String {
    match measure {
        QuotaMeasure::Exact(value) => value.to_string(),
        QuotaMeasure::Infinite => "inf".into(),
        QuotaMeasure::Unknown => "?".into(),
    }
}

fn reset_label(reset: &QuotaReset) -> &'static str {
    match reset {
        QuotaReset::Absolute { .. } => "absolute",
        QuotaReset::Relative { .. } => "relative",
        QuotaReset::Unknown => "unknown",
    }
}

fn confidence_label(confidence: QuotaConfidence) -> &'static str {
    match confidence {
        QuotaConfidence::Exact => "exact",
        QuotaConfidence::Derived => "derived",
        QuotaConfidence::Scraped => "scraped",
    }
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
    use std::sync::Mutex;
    use test_support::{MockProvider, MockScript};
    use transport_remote_placeholder::{
        ConnectOptions, MockRemoteConnector, MockRemoteTransportProvider, RemoteGuiConnector,
    };

    /// 记录远程端点生命周期调用的假 GUI Server 宿主（P17-11 接线测试）。
    struct RecordingGuiHost {
        binds: Mutex<Vec<String>>,
        closes: Mutex<Vec<String>>,
        bind_result: Result<(), String>,
    }

    impl RecordingGuiHost {
        fn new(bind_result: Result<(), String>) -> Self {
            Self {
                binds: Mutex::new(Vec::new()),
                closes: Mutex::new(Vec::new()),
                bind_result,
            }
        }
    }

    impl GuiServerHost for RecordingGuiHost {
        fn start(&self, _instance: &str) -> Result<(), String> {
            Ok(())
        }

        fn stop(&self) -> Result<(), String> {
            Ok(())
        }

        fn bind_remote(
            &self,
            handle_id: &str,
            _endpoint: &TransportEndpoint,
        ) -> Result<(), String> {
            self.binds.lock().unwrap().push(handle_id.into());
            self.bind_result.clone()
        }

        fn close_remote(&self, handle_id: &str) -> Result<(), String> {
            self.closes.lock().unwrap().push(handle_id.into());
            Ok(())
        }
    }

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
        let provider = Arc::new(MockRemoteTransportProvider::default());
        host.attach_gui_server(Arc::new(RecordingGuiHost::new(Ok(()))));
        host.attach_remote_provider(provider as Arc<dyn RemoteGuiTransportProvider>);

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
        host.attach_gui_server(Arc::new(RecordingGuiHost::new(Ok(()))));
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

    #[tokio::test]
    async fn remote_publish_binds_via_gui_host_and_revoke_closes_and_invalidates() {
        let service = Arc::new(AppService::new("remote-test"));
        let mut host = CliHost::new(service);
        let mock = Arc::new(MockRemoteTransportProvider::default());
        host.attach_remote_provider(Arc::clone(&mock) as Arc<dyn RemoteGuiTransportProvider>);
        let gui_host = Arc::new(RecordingGuiHost::new(Ok(())));
        host.attach_gui_server(gui_host.clone());

        let cli = Cli::try_parse_from(["pawork", "--json", "remote", "publish", "--name", "edge"])
            .expect("parse publish");
        let outcome = host.execute(cli).await;
        assert_eq!(outcome.exit_code, 0, "output: {}", outcome.output);
        let value: serde_json::Value = serde_json::from_str(&outcome.output).expect("JSON");
        assert_eq!(value["data"]["bound"], true);
        assert_eq!(value["data"]["handle_id"], "edge-0");
        assert_eq!(
            gui_host.binds.lock().unwrap().as_slice(),
            &["edge-0".to_string()],
            "publish must bind the endpoint via the gui server host"
        );

        // revoke：先关闭宿主侧监听器，再撤销端点（凭证失效、不可再连接）。
        let cli =
            Cli::try_parse_from(["pawork", "--json", "remote", "revoke", "--handle", "edge-0"])
                .expect("parse revoke");
        let outcome = host.execute(cli).await;
        assert_eq!(outcome.exit_code, 0, "output: {}", outcome.output);
        let value: serde_json::Value = serde_json::from_str(&outcome.output).expect("JSON");
        assert_eq!(value["data"]["action"], "revoke");
        assert_eq!(value["data"]["handle_id"], "edge-0");
        assert_eq!(value["data"]["status"], "revoked");
        assert_eq!(
            gui_host.closes.lock().unwrap().as_slice(),
            &["edge-0".to_string()],
            "revoke must close the bound listener via the gui server host"
        );

        // 撤销后该端点不可再连接（Mock 槽位已移除）。
        let connector = MockRemoteConnector::new(Arc::clone(mock.transport()));
        let error = match connector
            .connect(
                &TransportEndpoint::Remote {
                    address: "mock://edge-0".into(),
                    adapter: "mock".into(),
                },
                ConnectOptions {
                    timeout_ms: 1_000,
                    client_label: Some("cli-host-test".into()),
                    max_frame_bytes: 1024 * 1024,
                },
            )
            .await
        {
            Err(error) => error,
            Ok(_) => panic!("connect after revoke must fail"),
        };
        assert!(
            error.to_string().contains("no remote listener"),
            "unexpected error: {error:?}"
        );
    }

    #[tokio::test]
    async fn remote_publish_bind_failure_rolls_back_publish() {
        let service = Arc::new(AppService::new("remote-test"));
        let mut host = CliHost::new(service);
        let mock = Arc::new(MockRemoteTransportProvider::default());
        host.attach_remote_provider(Arc::clone(&mock) as Arc<dyn RemoteGuiTransportProvider>);
        host.attach_gui_server(Arc::new(RecordingGuiHost::new(Err("bind boom".into()))));

        let cli = Cli::try_parse_from(["pawork", "--json", "remote", "publish", "--name", "edge"])
            .expect("parse publish");
        let outcome = host.execute(cli).await;
        assert_ne!(outcome.exit_code, 0, "output: {}", outcome.output);
        let value: serde_json::Value = serde_json::from_str(&outcome.output).expect("JSON");
        assert_eq!(value["ok"], false);
        assert_eq!(value["data"]["action"], "publish");
        assert!(
            value["message"]
                .as_str()
                .expect("message")
                .contains("bind failed: bind boom"),
            "output: {}",
            outcome.output
        );

        // 发布已回滚：该端点未被预占，不可连接。
        let connector = MockRemoteConnector::new(Arc::clone(mock.transport()));
        let error = match connector
            .connect(
                &TransportEndpoint::Remote {
                    address: "mock://edge-0".into(),
                    adapter: "mock".into(),
                },
                ConnectOptions {
                    timeout_ms: 1_000,
                    client_label: Some("cli-host-test".into()),
                    max_frame_bytes: 1024 * 1024,
                },
            )
            .await
        {
            Err(error) => error,
            Ok(_) => panic!("connect after rolled-back publish must fail"),
        };
        assert!(
            error.to_string().contains("no remote listener"),
            "unexpected error: {error:?}"
        );
    }

    #[tokio::test]
    async fn remote_publish_without_gui_host_fails_closed_and_rolls_back() {
        let service = Arc::new(AppService::new("remote-test"));
        let mut host = CliHost::new(service);
        let mock = Arc::new(MockRemoteTransportProvider::default());
        host.attach_remote_provider(Arc::clone(&mock) as Arc<dyn RemoteGuiTransportProvider>);

        let cli = Cli::try_parse_from(["pawork", "--json", "remote", "publish", "--name", "edge"])
            .expect("parse publish");
        let outcome = host.execute(cli).await;
        assert_ne!(outcome.exit_code, 0, "output: {}", outcome.output);
        let value: serde_json::Value = serde_json::from_str(&outcome.output).expect("JSON");
        assert_eq!(value["ok"], false);
        assert!(value["message"]
            .as_str()
            .expect("message")
            .contains("publish rolled back"));

        let connector = MockRemoteConnector::new(Arc::clone(mock.transport()));
        let result = connector
            .connect(
                &TransportEndpoint::Remote {
                    address: "mock://edge-0".into(),
                    adapter: "mock".into(),
                },
                ConnectOptions {
                    timeout_ms: 1_000,
                    client_label: Some("fail-closed".into()),
                    max_frame_bytes: 1024,
                },
            )
            .await;
        assert!(
            result.is_err(),
            "rolled-back endpoint must not be connectable"
        );
    }

    #[tokio::test]
    async fn remote_revoke_unknown_handle_returns_structured_error() {
        let service = Arc::new(AppService::new("remote-test"));
        let mut host = CliHost::new(service);
        host.attach_remote_provider(Arc::new(MockRemoteTransportProvider::default()));
        let cli = Cli::try_parse_from([
            "pawork",
            "--json",
            "remote",
            "revoke",
            "--handle",
            "never-published",
        ])
        .expect("parse");
        let outcome = host.execute(cli).await;
        assert_ne!(outcome.exit_code, 0);
        let value: serde_json::Value = serde_json::from_str(&outcome.output).expect("JSON");
        assert_eq!(value["ok"], false);
        assert_eq!(value["data"]["action"], "revoke");
        assert!(
            value["message"]
                .as_str()
                .expect("message")
                .contains("unknown remote publish handle"),
            "output: {}",
            outcome.output
        );
    }

    #[tokio::test]
    async fn usage_json_returns_envelope_with_no_data_windows() {
        // 显式 provider（P14 review §2.4：缺失即 validation error）；无 quota
        // runtime：每个窗口 NoData + from_cache=false；JSON envelope 合法。
        let service = Arc::new(AppService::new("usage-json"));
        let host = CliHost::new(service);
        let cli = Cli::try_parse_from(["pawork", "--json", "usage", "--provider", "mock"])
            .expect("parse");
        let outcome = host.execute(cli).await;
        assert_eq!(outcome.exit_code, 0, "output: {}", outcome.output);
        let value: serde_json::Value = serde_json::from_str(&outcome.output).expect("JSON");
        assert_eq!(value["ok"], true);
        assert_eq!(value["kind"], "usage");
        // 视图落在 data.response.data。
        let windows = &value["data"]["response"]["data"]["windows"];
        assert!(windows.is_array(), "windows array: {windows}");
        assert!(
            windows
                .as_array()
                .unwrap()
                .iter()
                .all(|w| w["read"]["status"] == "no_data"),
            "no-data windows: {windows}"
        );
        assert_eq!(value["data"]["response"]["data"]["from_cache"], false);
        // scope 反映默认 local/local/default。
        assert_eq!(
            value["data"]["response"]["data"]["scope"]["tenant_id"],
            "local"
        );
        assert_eq!(
            value["data"]["response"]["data"]["scope"]["account_id"],
            "local/default"
        );
        // scope 携带显式 provider。
        assert_eq!(
            value["data"]["response"]["data"]["scope"]["provider_id"],
            "mock"
        );
    }

    #[tokio::test]
    async fn usage_text_lists_windows_and_cache_status() {
        let service = Arc::new(AppService::new("usage-text"));
        let host = CliHost::new(service);
        let cli = Cli::try_parse_from(["pawork", "usage", "--provider", "mock"]).expect("parse");
        let outcome = host.execute(cli).await;
        assert_eq!(outcome.exit_code, 0, "output: {}", outcome.output);
        // 头部含默认作用域、显式 provider 与 cache=miss；每个窗口一行 no data。
        assert!(
            outcome
                .output
                .contains("usage for local/local/default (provider=mock, cache=miss)"),
            "output: {}",
            outcome.output
        );
        assert!(
            outcome.output.contains("cache=miss"),
            "output: {}",
            outcome.output
        );
        assert!(
            outcome.output.contains("no data"),
            "output: {}",
            outcome.output
        );
    }

    #[tokio::test]
    async fn usage_filters_parse_into_query() {
        // 仅验证参数解析 → 查询分发链路不 panic；非默认作用域会被授权拒绝。
        let service = Arc::new(AppService::new("usage-filter"));
        let host = CliHost::new(service);
        let cli = Cli::try_parse_from([
            "pawork",
            "--json",
            "usage",
            "--tenant",
            "acme",
            "--account",
            "acme/team",
            "--provider",
            "anthropic",
            "--window",
            "monthly",
            "--unit",
            "token",
        ])
        .expect("parse");
        let outcome = host.execute(cli).await;
        // 非默认作用域 + LocalCli → Authorization 错误（exit_code != 0）。
        assert_ne!(outcome.exit_code, 0, "expected denial: {}", outcome.output);
        let value: serde_json::Value = serde_json::from_str(&outcome.output).expect("JSON");
        assert_eq!(value["ok"], false, "output: {}", outcome.output);
        // ErrorContext.category 序列化为 snake_case；在 data.response.error.category。
        let category = value["data"]["response"]["data"]["category"]
            .as_str()
            .unwrap_or("?");
        assert_eq!(category, "authorization", "output: {}", outcome.output);
    }

    #[test]
    fn usage_query_maps_typed_window_and_unit_directly() {
        let args = UsageArgs {
            tenant: None,
            account: None,
            provider: None,
            credential: None,
            model: None,
            window: vec![UsageWindow::Weekly, UsageWindow::Monthly],
            unit: Some(UsageUnit::Cost {
                currency: "USD".into(),
            }),
        };
        let query = usage_query_from_args(&args);
        assert_eq!(query.tenant_id.as_str(), "local");
        assert_eq!(query.account_id, "local/default");
        assert_eq!(
            query.windows,
            vec![QuotaWindow::Weekly, QuotaWindow::Monthly]
        );
        assert_eq!(
            query.unit,
            Some(QuotaUnit::Cost {
                currency: "USD".into()
            })
        );

        // 缺省：空窗口表（= 所有窗口）+ 无单位。
        let defaults = UsageArgs {
            tenant: None,
            account: None,
            provider: None,
            credential: None,
            model: None,
            window: Vec::new(),
            unit: None,
        };
        let query = usage_query_from_args(&defaults);
        assert!(query.windows.is_empty());
        assert!(query.unit.is_none());
        assert_eq!(query.provider_id, None);
    }

    #[test]
    fn render_usage_text_renders_typed_view_fields() {
        use core_api::{
            QuotaAdapterKind, QuotaFailureView, QuotaProvenanceView, QuotaScopeView,
            QuotaSnapshotView, QuotaValues, WindowReadEntry,
        };

        let scope = QuotaScopeView {
            tenant_id: TenantId::new("local"),
            account_id: "local/default".into(),
            provider_id: ProviderId::from("mock"),
            model_id: None,
            credential_hint: None,
        };
        let view = QuotaOverviewView {
            scope: scope.clone(),
            windows: vec![
                WindowReadEntry {
                    window: QuotaWindow::Monthly,
                    read: WindowReadView::Ok {
                        snapshot: Box::new(QuotaSnapshotView {
                            scope: scope.clone(),
                            window: QuotaWindow::Monthly,
                            unit: QuotaUnit::Cost {
                                currency: "USD".into(),
                            },
                            values: QuotaValues {
                                used: QuotaMeasure::Exact(120),
                                limit: QuotaMeasure::Exact(1000),
                                remaining: QuotaMeasure::Exact(880),
                            },
                            reset: QuotaReset::Absolute {
                                at: Timestamp::from_unix_millis(1_000),
                                uncertain: false,
                            },
                            confidence: QuotaConfidence::Exact,
                            provenance: QuotaProvenanceView {
                                adapter_kind: QuotaAdapterKind::ApiKeyApi,
                                source: "mock-source".into(),
                                endpoint: Some("https://example.test/billing".into()),
                                fetched_at: Timestamp::from_unix_millis(1_000),
                                observed_at: None,
                                stale: false,
                            },
                            served_stale: false,
                        }),
                        failures: Vec::new(),
                    },
                },
                WindowReadEntry {
                    window: QuotaWindow::Weekly,
                    read: WindowReadView::Ok {
                        snapshot: Box::new(QuotaSnapshotView {
                            scope: scope.clone(),
                            window: QuotaWindow::Weekly,
                            unit: QuotaUnit::Token,
                            values: QuotaValues {
                                used: QuotaMeasure::Exact(50),
                                limit: QuotaMeasure::Infinite,
                                remaining: QuotaMeasure::Unknown,
                            },
                            reset: QuotaReset::Relative {
                                after_secs: 3600,
                                observed_at: Timestamp::from_unix_millis(1_000),
                                uncertain: true,
                            },
                            confidence: QuotaConfidence::Derived,
                            provenance: QuotaProvenanceView {
                                adapter_kind: QuotaAdapterKind::LocalLedger,
                                source: "ledger".into(),
                                endpoint: None,
                                fetched_at: Timestamp::from_unix_millis(1_000),
                                observed_at: Some(Timestamp::from_unix_millis(1_000)),
                                stale: true,
                            },
                            served_stale: true,
                        }),
                        failures: Vec::new(),
                    },
                },
                WindowReadEntry {
                    window: QuotaWindow::Overall,
                    read: WindowReadView::Failed {
                        failures: vec![
                            QuotaFailureView {
                                adapter_kind: Some(QuotaAdapterKind::ApiKeyApi),
                                error_code: "forbidden".into(),
                                detail: "denied".into(),
                                retry_after_ms: None,
                            },
                            QuotaFailureView {
                                adapter_kind: Some(QuotaAdapterKind::WebScrape),
                                error_code: "timeout".into(),
                                detail: "timed out".into(),
                                retry_after_ms: Some(5000),
                            },
                        ],
                    },
                },
                WindowReadEntry {
                    window: QuotaWindow::Rolling5h,
                    read: WindowReadView::NoData,
                },
            ],
            generated_at: Timestamp::from_unix_millis(2_000),
            from_cache: true,
        };
        let envelope = AppResponseEnvelope {
            api_version: API_VERSION,
            request_id: QueryId::from("render-test"),
            responded_at: Timestamp::from_unix_millis(2_000),
            response: AppResponse::Data(serde_json::to_value(&view).expect("serialize view")),
        };

        let text = render_usage_text(&envelope);
        let expected = "\
usage for local/local/default (provider=mock, cache=hit)
monthly cost:USD: used=120 limit=1000 remaining=880 reset=absolute confidence=exact source=mock-source stale=false
weekly token: used=50 limit=inf remaining=? reset=relative confidence=derived source=ledger stale=true
overall: failed (2 failure(s))
rolling5h: no data";
        assert_eq!(text, expected);
    }

    #[test]
    fn render_usage_text_handles_error_response() {
        let envelope = AppResponseEnvelope {
            api_version: API_VERSION,
            request_id: QueryId::from("error-test"),
            responded_at: now_timestamp(),
            response: AppResponse::Error(agent_domain::ErrorContext {
                category: agent_domain::ErrorCategory::Authorization,
                message: "not allowed".into(),
                retryable: false,
                retry_after_ms: None,
                diagnostics: BTreeMap::new(),
            }),
        };
        assert_eq!(render_usage_text(&envelope), "usage: error: not allowed");
    }
}
