//! Controller 层：唯一业务出口是 pawork-client。
//!
//! 职责：连接握手 + 事件泵、SessionGet 分页、SessionCreate / SessionFork /
//! RunStart / RunCancel / ToolApprove / ModelList，以及 TerminalCreate /
//! TerminalWrite / TerminalResize。重连走 [`GuiClient::connect_with_resume`]，
//! 记录 last_acked `global_sequence`（来自事件与 Ack），按 Replay /
//! SnapshotRequired / UpToDate 三态交给 projection。

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use pawork_client::{
    ActorIdentity, AppCommand, AppEventEnvelope, AppQuery, AppResponse, AppResponseEnvelope,
    ClientAuthentication, ClientConfig, ClientError, CommandSource, ConnectOptions, GlobalSequence,
    GuiCapability, GuiClient, GuiTransportClient, LocalTransport, ProtocolErrorCode,
    ResumeDisposition, ResumeOutcome, Snapshot, TimelinePage, TransportEndpoint, TOKEN_SCHEME,
};
use serde_json::json;

use crate::projection::{
    parse_general_settings, parse_provider_status_entries, sessions_in_snapshot, ModelEntry,
    SettingsProvidersData,
};

const PAGE_LIMIT: u32 = 500;
const MAX_PAGES: usize = 200;

/// UI 消费的控制器事件（经 smol channel 跨线程投递）。
#[derive(Clone, Debug)]
pub enum ControllerEvent {
    Disconnected {
        reason: String,
    },
    Snapshot(Snapshot),
    TimelineLoaded {
        session_id: String,
        page: TimelinePage,
    },
    Event(AppEventEnvelope),
    SessionCreated {
        session_id: String,
    },
    WorkspaceOpened {
        workspace_id: String,
        name: String,
    },
    /// 发送回执：text 随行携带，供 UI 在 wire 用户消息事件缺席时乐观回显。
    MessageSent {
        session_id: String,
        run_id: String,
        text: String,
    },
    ModelsLoaded(Vec<ModelEntry>),
    /// provider_auth_status 查询成功（SET-3 只读供应商页；SET-5 起随载荷
    /// 携带 Host 权威默认模型）。
    ProviderStatusLoaded(SettingsProvidersData),
    /// set_default_model 获 Host Data 确认（SET-5；echo 携带已确认 pair，
    /// Composer 据此同步）。随后 controller 重查 provider_auth_status 取回
    /// 权威 default。
    DefaultModelConfirmed {
        provider_id: String,
        model_id: String,
    },
    /// general_settings 查询成功（SET-6a 通用页；Host 权威 proxy_url）。
    GeneralSettingsLoaded(Option<String>),
    /// set_proxy_url 获 Host Data 确认（SET-6a；回执即写后状态）。
    ProxyUrlConfirmed {
        proxy_url: Option<String>,
    },
    /// auth_start 响应（SET-4）：OAuth 授权等待信息；进度经 AuthChanged
    /// 事件流下发，token 不经过 Desktop。
    AuthStarted {
        provider_id: String,
        verification_url: String,
        user_code: Option<String>,
        expires_at: Option<String>,
    },
    SessionForked {
        session_id: String,
    },
    TerminalCreated {
        workspace_id: String,
        terminal_session_id: String,
    },
    TerminalCreateFailed {
        workspace_id: String,
        reason: String,
    },
    TerminalWriteSucceeded {
        terminal_session_id: String,
    },
    TerminalWriteFailed {
        terminal_session_id: String,
        reason: String,
    },
    TerminalResizeSucceeded {
        terminal_session_id: String,
        columns: u16,
        rows: u16,
    },
    TerminalResizeFailed {
        terminal_session_id: String,
        reason: String,
    },
    /// terminal_close 已被 Host 接受（ADR-045）。running 的终态由 live
    /// TerminalExited 事件刷新；exited 清理由 UI 在回执后本地移除条目。
    TerminalCloseSucceeded {
        terminal_session_id: String,
    },
    TerminalCloseFailed {
        terminal_session_id: String,
        reason: String,
    },
    /// diff_list_files 成功（epoch 为 UI 侧请求代次，防过期响应覆盖新状态）。
    DiffFilesLoaded {
        epoch: u64,
        session_id: Option<String>,
        files: Vec<DiffFileSummary>,
        git: Option<GitDiffInfo>,
    },
    /// diff_get 成功；file 为 None 表示该路径已不在 diff 中（host 空响应）。
    DiffContentLoaded {
        epoch: u64,
        path: String,
        session_id: Option<String>,
        file: Option<DiffFileDetail>,
    },
    /// mcp_list 成功（响应形状 {"servers":[{name,transport,state,tools,last_error}]}）。
    McpServersLoaded {
        epoch: u64,
        servers: Vec<McpServerEntry>,
    },
    DiffFilesFailed {
        epoch: u64,
        reason: String,
    },
    DiffContentFailed {
        epoch: u64,
        path: String,
        reason: String,
    },
    McpServersFailed {
        epoch: u64,
        reason: String,
    },
    OperationFailed {
        action: &'static str,
        reason: String,
    },
}

/// Changes 面 Files 行（diff_list_files 响应的视图模型）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffFileSummary {
    pub path: String,
    /// host 序列化的 snake_case 状态（added / modified / …）；缺失记 unknown。
    pub status: String,
    pub additions: u64,
    pub deletions: u64,
    pub binary: bool,
}

/// diff_list_files 携带的 git 信息；字段缺失保持 None，UI 显示 unknown。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GitDiffInfo {
    pub branch: Option<String>,
    pub work_dir: Option<String>,
    pub dirty_files: Option<u64>,
}

/// diff 行类型（host LineKind 的 snake_case wire 名）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiffLineKind {
    Context,
    Addition,
    Deletion,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffLineDetail {
    pub kind: DiffLineKind,
    /// 行文本（不含 +/-/空格 前缀）。
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffHunkDetail {
    /// hunk 头原文（如 `@@ -1,3 +1,4 @@`）。
    pub header: String,
    pub lines: Vec<DiffLineDetail>,
}

/// diff_get 响应的单文件视图模型（仅保留渲染所需字段）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffFileDetail {
    pub path: String,
    /// rename / copy 时的原始路径。
    pub previous_path: Option<String>,
    pub status: String,
    pub binary: bool,
    pub additions: u64,
    pub deletions: u64,
    pub hunks: Vec<DiffHunkDetail>,
}

/// Resources 页 MCP server 行；tools 在 wire 上是名称数组，这里只留数量。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct McpServerEntry {
    pub name: String,
    pub transport: String,
    pub state: String,
    pub tool_count: u64,
    pub last_error: Option<String>,
}

/// 握手 / 重连结果：`resume` 为 None 表示首连（无 last_ack）。
pub struct DesktopConnect {
    pub snapshot: Snapshot,
    pub resume: Option<ResumeOutcome>,
    pub events: smol::channel::Receiver<ControllerEvent>,
}

struct SharedState {
    client: Mutex<Option<GuiClient>>,
    events: Mutex<Option<smol::channel::Sender<ControllerEvent>>>,
    last_acked: Mutex<Option<u64>>,
}

pub struct DesktopController {
    runtime: tokio::runtime::Handle,
    state: Arc<SharedState>,
}

impl DesktopController {
    pub fn new(runtime: tokio::runtime::Handle) -> Self {
        Self {
            runtime,
            state: Arc::new(SharedState {
                client: Mutex::new(None),
                events: Mutex::new(None),
                last_acked: Mutex::new(None),
            }),
        }
    }

    fn current_client(&self) -> Option<GuiClient> {
        self.state
            .client
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    /// 连接 + 握手 + 订阅。有 last_ack 时走 `connect_with_resume`，不要永远
    /// 全新 Snapshot。
    pub async fn connect(&self, socket: PathBuf) -> Result<DesktopConnect, String> {
        let token_path = crate::platform::token_path_for_socket(&socket);
        let authentication = load_desktop_authentication(&token_path)?;
        let (sender, receiver) = smol::channel::bounded::<ControllerEvent>(512);
        let transport: Arc<dyn GuiTransportClient> = Arc::new(LocalTransport::default());
        let endpoint = TransportEndpoint::Local {
            address: socket.to_string_lossy().into_owned(),
        };
        let options = ConnectOptions {
            timeout_ms: 10_000,
            client_label: Some("pawork-desktop".into()),
            max_frame_bytes: 1024 * 1024,
        };
        let last_ack = self.last_acked_sequence().map(GlobalSequence);
        let has_last_ack = last_ack.is_some();
        // 连接期全部 client 调用（握手 / ack / subscribe_all）都必须在 tokio
        // runtime 上执行：cx.spawn 的 gpui 前台执行器没有 reactor，
        // receive_frame 内的 tokio::time 会在真窗口启动路径直接 panic。
        let state = Arc::clone(&self.state);
        let connected = self
            .runtime
            .spawn(async move {
                let (handshake, resume) = GuiClient::connect_with_resume_config(
                    transport,
                    endpoint,
                    options,
                    Some(authentication),
                    last_ack,
                    desktop_client_config(),
                )
                .await
                .map_err(|error| error.to_string())?;
                let mut snapshot = handshake
                    .initial_snapshot()
                    .ok_or_else(|| "handshake did not deliver an initial snapshot".to_string())?;
                if !has_last_ack {
                    record_shared_last_acked(&state, snapshot.snapshot_sequence.0);
                    let _ = handshake.ack(snapshot.snapshot_sequence).await;
                }
                if let Some(outcome) = &resume {
                    match &outcome.disposition {
                        ResumeDisposition::Replay {
                            through_sequence, ..
                        } => {
                            record_shared_last_acked(&state, through_sequence.0);
                            let _ = handshake.ack(*through_sequence).await;
                        }
                        ResumeDisposition::UpToDate { current_sequence } => {
                            record_shared_last_acked(&state, current_sequence.0);
                        }
                        ResumeDisposition::SnapshotRequired { .. } => {
                            if let Some(fresh) = &outcome.snapshot {
                                snapshot = fresh.clone();
                            }
                            record_shared_last_acked(&state, snapshot.snapshot_sequence.0);
                            let _ = handshake.ack(snapshot.snapshot_sequence).await;
                        }
                    }
                }
                handshake
                    .subscribe_all()
                    .await
                    .map_err(|error| error.to_string())?;
                Ok::<_, String>((handshake, resume, snapshot))
            })
            .await
            .map_err(|error| format!("connect task failed: {error}"))??;
        let (handshake, resume, snapshot) = connected;

        *self.state.client.lock().unwrap_or_else(|p| p.into_inner()) = Some(handshake.clone());
        *self.state.events.lock().unwrap_or_else(|p| p.into_inner()) = Some(sender.clone());

        let pump_client = handshake.clone();
        let pump_events = sender;
        let pump_state = Arc::clone(&self.state);
        self.runtime.spawn(async move {
            // 宿主 heartbeat_timeout 为 30s，任意入站帧都会刷新；空闲时由 desktop 周期 heartbeat 保活。
            let mut idle_ticks = 0u32;
            loop {
                match pump_client.next_event_timeout(Duration::from_secs(1)).await {
                    Ok(event) => {
                        idle_ticks = 0;
                        record_shared_last_acked(&pump_state, event.global_sequence.0);
                        let _ = pump_client.ack(event.global_sequence).await;
                        if pump_events
                            .send(ControllerEvent::Event(event))
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                    Err(ClientError::Timeout { .. }) => {
                        idle_ticks += 1;
                        if idle_ticks < 15 {
                            continue;
                        }
                        idle_ticks = 0;
                        if let Err(error) = pump_client.heartbeat().await {
                            let reason = error.to_string();
                            *pump_state.client.lock().unwrap_or_else(|p| p.into_inner()) = None;
                            let _ = pump_events
                                .send(ControllerEvent::Disconnected { reason })
                                .await;
                            break;
                        }
                    }
                    Err(error) => {
                        let reason = error.to_string();
                        *pump_state.client.lock().unwrap_or_else(|p| p.into_inner()) = None;
                        let _ = pump_events
                            .send(ControllerEvent::Disconnected { reason })
                            .await;
                        break;
                    }
                }
            }
        });
        Ok(DesktopConnect {
            snapshot,
            resume,
            events: receiver,
        })
    }

    pub fn last_acked_sequence(&self) -> Option<u64> {
        *self
            .state
            .last_acked
            .lock()
            .unwrap_or_else(|p| p.into_inner())
    }

    /// 分页加载 session 时间线：SessionGet 按 timeline_after_sequence 链式
    /// 拉取直到 complete；分页期间先到的 live 事件由 projection 按 sequence
    /// 去重（gui-design §4.1 第 3 条）。
    pub fn open_session(&self, session_id: String) {
        let Some(client) = self.current_client() else {
            if let Some(events) = self.try_event_sender() {
                try_emit(
                    &events,
                    ControllerEvent::OperationFailed {
                        action: "open session",
                        reason: "not connected".into(),
                    },
                );
            }
            return;
        };
        let events = self.event_sender();
        self.runtime.spawn(async move {
            let mut after: Option<u64> = None;
            for _ in 0..MAX_PAGES {
                let query = session_get_query(&session_id, after);
                let response = match client
                    .query(query, command_source(), actor_identity())
                    .await
                {
                    Ok(response) => response,
                    Err(error) => {
                        try_emit(
                            &events,
                            ControllerEvent::OperationFailed {
                                action: "open session",
                                reason: error.to_string(),
                            },
                        );
                        return;
                    }
                };
                let page = match timeline_page(&response) {
                    Ok(Some(page)) => page,
                    Ok(None) => {
                        try_emit(
                            &events,
                            ControllerEvent::OperationFailed {
                                action: "open session",
                                reason: "session_get response carried no timeline page".into(),
                            },
                        );
                        return;
                    }
                    Err(reason) => {
                        try_emit(
                            &events,
                            ControllerEvent::OperationFailed {
                                action: "open session",
                                reason,
                            },
                        );
                        return;
                    }
                };
                let complete = page.complete;
                after = page.next_sequence;
                if events
                    .send(ControllerEvent::TimelineLoaded {
                        session_id: session_id.clone(),
                        page,
                    })
                    .await
                    .is_err()
                {
                    return;
                }
                if complete {
                    return;
                }
            }
            try_emit(
                &events,
                ControllerEvent::OperationFailed {
                    action: "open session",
                    reason: format!("timeline exceeded {MAX_PAGES} pages"),
                },
            );
        });
    }

    /// 新建 session：SessionCreate 只回 Accepted（无 session id），重取 snapshot
    /// 挑 updated_at_ms 最新的 session 返回（host gui_host 行为）。
    pub fn create_session(&self, workspace_id: String) {
        let Some(client) = self.current_client() else {
            if let Some(events) = self.try_event_sender() {
                try_emit(
                    &events,
                    ControllerEvent::OperationFailed {
                        action: "create session",
                        reason: "not connected".into(),
                    },
                );
            }
            return;
        };
        let events = self.event_sender();
        self.runtime.spawn(async move {
            let command = session_create_command(&workspace_id);
            if let Err(error) = client
                .command(command, command_source(), actor_identity())
                .await
            {
                try_emit(
                    &events,
                    ControllerEvent::OperationFailed {
                        action: "create session",
                        reason: error.to_string(),
                    },
                );
                return;
            }
            match client.snapshot().await {
                Ok(snapshot) => {
                    let latest = sessions_in_snapshot(&snapshot)
                        .into_iter()
                        .map(|session| session.session_id)
                        .next();
                    if events
                        .send(ControllerEvent::Snapshot(snapshot))
                        .await
                        .is_err()
                    {
                        return;
                    }
                    if let Some(session_id) = latest {
                        let _ = events
                            .send(ControllerEvent::SessionCreated { session_id })
                            .await;
                    } else {
                        try_emit(
                            &events,
                            ControllerEvent::OperationFailed {
                                action: "create session",
                                reason: "host accepted SessionCreate but snapshot has no sessions"
                                    .into(),
                            },
                        );
                    }
                }
                Err(error) => {
                    try_emit(
                        &events,
                        ControllerEvent::OperationFailed {
                            action: "create session",
                            reason: error.to_string(),
                        },
                    );
                }
            }
        });
    }

    /// 选择一个真实目录作为当前项目；成功后重取 snapshot，让 UI 只消费
    /// Host 的 canonical workspace 结果，不在 Desktop 侧猜名称或 id。
    pub fn open_workspace(&self, root_path: PathBuf) {
        let Some(client) = self.current_client() else {
            if let Some(events) = self.try_event_sender() {
                try_emit(
                    &events,
                    ControllerEvent::OperationFailed {
                        action: "open project",
                        reason: "not connected".into(),
                    },
                );
            }
            return;
        };
        let events = self.event_sender();
        self.runtime.spawn(async move {
            let command = workspace_add_command(&root_path);
            let response = match client
                .command(command, command_source(), actor_identity())
                .await
            {
                Ok(response) => response,
                Err(error) => {
                    try_emit(
                        &events,
                        ControllerEvent::OperationFailed {
                            action: "open project",
                            reason: error.to_string(),
                        },
                    );
                    return;
                }
            };
            let Some((workspace_id, name)) = workspace_opened(&response) else {
                try_emit(
                    &events,
                    ControllerEvent::OperationFailed {
                        action: "open project",
                        reason: format!("unexpected response: {:?}", response.response),
                    },
                );
                return;
            };
            match client.snapshot().await {
                Ok(snapshot) => {
                    if events
                        .send(ControllerEvent::Snapshot(snapshot))
                        .await
                        .is_err()
                    {
                        return;
                    }
                    let _ = events
                        .send(ControllerEvent::WorkspaceOpened { workspace_id, name })
                        .await;
                }
                Err(error) => {
                    try_emit(
                        &events,
                        ControllerEvent::OperationFailed {
                            action: "open project",
                            reason: error.to_string(),
                        },
                    );
                }
            }
        });
    }

    /// 发送用户消息：RunStart。可选 `(provider, model)` 只影响下一轮。
    pub fn send_message(&self, session_id: String, text: String, model: Option<(String, String)>) {
        let Some(client) = self.current_client() else {
            return;
        };
        let events = self.event_sender();
        self.runtime.spawn(async move {
            let command = run_start_command(&session_id, &text, model.as_ref());
            match client
                .command(command, command_source(), actor_identity())
                .await
            {
                Ok(response) => match response.response {
                    AppResponse::Accepted {
                        run_id: Some(run_id),
                        ..
                    } => {
                        let run_id = run_id.as_str().to_string();
                        let _ = events
                            .send(ControllerEvent::MessageSent {
                                session_id,
                                run_id,
                                text,
                            })
                            .await;
                    }
                    other => {
                        try_emit(
                            &events,
                            ControllerEvent::OperationFailed {
                                action: "send message",
                                reason: format!("unexpected response: {other:?}"),
                            },
                        );
                    }
                },
                Err(error) => {
                    try_emit(
                        &events,
                        ControllerEvent::OperationFailed {
                            action: "send message",
                            reason: error.to_string(),
                        },
                    );
                }
            }
        });
    }

    pub fn cancel_run(&self, run_id: String) {
        let Some(client) = self.current_client() else {
            return;
        };
        let events = self.event_sender();
        self.runtime.spawn(async move {
            let command = run_cancel_command(&run_id);
            if let Err(error) = client
                .command(command, command_source(), actor_identity())
                .await
            {
                try_emit(
                    &events,
                    ControllerEvent::OperationFailed {
                        action: "cancel run",
                        reason: error.to_string(),
                    },
                );
            }
        });
    }

    pub fn approve(&self, run_id: String, tool_call_id: String, decision: &str) {
        let Some(client) = self.current_client() else {
            return;
        };
        let events = self.event_sender();
        let command = tool_approve_command(&run_id, &tool_call_id, decision);
        self.runtime.spawn(async move {
            if let Err(error) = client
                .command(command, command_source(), actor_identity())
                .await
            {
                try_emit(
                    &events,
                    ControllerEvent::OperationFailed {
                        action: "approve tool",
                        reason: error.to_string(),
                    },
                );
            }
        });
    }

    /// 主动断开：关窗 / `--probe-smoke` 重连前调用。不发 RunCancel（ADR-026）。
    pub async fn disconnect(&self) {
        let client = self
            .state
            .client
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take();
        if let Some(client) = client {
            let _ = client.close().await;
        }
    }

    /// 给 `--probe` 用的同步目录查询：不经 UI channel。
    pub async fn fetch_models(&self) -> Result<Vec<ModelEntry>, String> {
        let client = self
            .current_client()
            .ok_or_else(|| "not connected".to_string())?;
        let response = client
            .query(model_list_query(), command_source(), actor_identity())
            .await
            .map_err(|error| error.to_string())?;
        parse_models(&response)
    }

    /// 对 Timeline 某条 event_id 发 SessionFork。Host 仍可能 unsupported，
    /// 错误走既有 OperationFailed，不改 host/app。
    pub fn fork_session(&self, session_id: String, parent_event_id: String) {
        let Some(client) = self.current_client() else {
            if let Some(events) = self.try_event_sender() {
                try_emit(
                    &events,
                    ControllerEvent::OperationFailed {
                        action: "fork session",
                        reason: "not connected".into(),
                    },
                );
            }
            return;
        };
        let events = self.event_sender();
        self.runtime.spawn(async move {
            let command = session_fork_command(&session_id, &parent_event_id);
            match client
                .command(command, command_source(), actor_identity())
                .await
            {
                Ok(response) => match &response.response {
                    AppResponse::Error(_) => {
                        try_emit(
                            &events,
                            ControllerEvent::OperationFailed {
                                action: "fork session",
                                reason: "server returned an error response".into(),
                            },
                        );
                    }
                    AppResponse::Accepted { .. } | AppResponse::Data(_) => {
                        let hinted = forked_session_id(&response);
                        match client.snapshot().await {
                            Ok(snapshot) => {
                                let latest = hinted.or_else(|| {
                                    sessions_in_snapshot(&snapshot)
                                        .into_iter()
                                        .map(|session| session.session_id)
                                        .next()
                                });
                                if events
                                    .send(ControllerEvent::Snapshot(snapshot))
                                    .await
                                    .is_err()
                                {
                                    return;
                                }
                                if let Some(session_id) = latest {
                                    let _ = events
                                        .send(ControllerEvent::SessionForked { session_id })
                                        .await;
                                }
                            }
                            Err(error) => try_emit(
                                &events,
                                ControllerEvent::OperationFailed {
                                    action: "fork session",
                                    reason: error.to_string(),
                                },
                            ),
                        }
                    }
                    other => try_emit(
                        &events,
                        ControllerEvent::OperationFailed {
                            action: "fork session",
                            reason: format!("unexpected response: {other:?}"),
                        },
                    ),
                },
                Err(error) => try_emit(
                    &events,
                    ControllerEvent::OperationFailed {
                        action: "fork session",
                        reason: error.to_string(),
                    },
                ),
            }
        });
    }

    pub fn terminal_create(&self, workspace_id: String, cwd: Option<String>) {
        let Some(client) = self.current_client() else {
            self.emit_reliable(ControllerEvent::TerminalCreateFailed {
                workspace_id,
                reason: "not connected".into(),
            });
            return;
        };
        let events = self.event_sender();
        self.runtime.spawn(async move {
            let command = match terminal_create_command(&workspace_id, cwd.as_deref()) {
                Ok(command) => command,
                Err(reason) => {
                    let _ = events
                        .send(ControllerEvent::TerminalCreateFailed {
                            workspace_id,
                            reason,
                        })
                        .await;
                    return;
                }
            };
            match client
                .command(command, command_source(), actor_identity())
                .await
            {
                Ok(response) => match terminal_session_id(&response) {
                    Some(terminal_session_id) => {
                        let _ = events
                            .send(ControllerEvent::TerminalCreated {
                                workspace_id,
                                terminal_session_id,
                            })
                            .await;
                    }
                    None => {
                        let _ = events
                            .send(ControllerEvent::TerminalCreateFailed {
                                workspace_id,
                                reason: format!("unexpected response: {:?}", response.response),
                            })
                            .await;
                    }
                },
                Err(error) => {
                    let _ = events
                        .send(ControllerEvent::TerminalCreateFailed {
                            workspace_id,
                            reason: error.to_string(),
                        })
                        .await;
                }
            }
        });
    }

    pub fn terminal_write(&self, terminal_session_id: String, data: String) {
        let Some(client) = self.current_client() else {
            self.emit_reliable(ControllerEvent::TerminalWriteFailed {
                terminal_session_id,
                reason: "not connected".into(),
            });
            return;
        };
        let events = self.event_sender();
        self.runtime.spawn(async move {
            let command = terminal_write_command(&terminal_session_id, &data);
            match client
                .command(command, command_source(), actor_identity())
                .await
            {
                Ok(response) if !matches!(&response.response, AppResponse::Error(_)) => {
                    let _ = events
                        .send(ControllerEvent::TerminalWriteSucceeded {
                            terminal_session_id,
                        })
                        .await;
                }
                Ok(response) => {
                    let _ = events
                        .send(ControllerEvent::TerminalWriteFailed {
                            terminal_session_id,
                            reason: format!("server returned {:?}", response.response),
                        })
                        .await;
                }
                Err(error) => {
                    let _ = events
                        .send(ControllerEvent::TerminalWriteFailed {
                            terminal_session_id,
                            reason: error.to_string(),
                        })
                        .await;
                }
            }
        });
    }

    pub fn terminal_resize(&self, terminal_session_id: String, columns: u16, rows: u16) {
        let Some(client) = self.current_client() else {
            self.emit_reliable(ControllerEvent::TerminalResizeFailed {
                terminal_session_id,
                reason: "not connected".into(),
            });
            return;
        };
        let events = self.event_sender();
        self.runtime.spawn(async move {
            let command = terminal_resize_command(&terminal_session_id, columns, rows);
            match client
                .command(command, command_source(), actor_identity())
                .await
            {
                Ok(response) if !matches!(&response.response, AppResponse::Error(_)) => {
                    let _ = events
                        .send(ControllerEvent::TerminalResizeSucceeded {
                            terminal_session_id,
                            columns,
                            rows,
                        })
                        .await;
                }
                Ok(response) => {
                    let _ = events
                        .send(ControllerEvent::TerminalResizeFailed {
                            terminal_session_id,
                            reason: format!("server returned {:?}", response.response),
                        })
                        .await;
                }
                Err(error) => {
                    let _ = events
                        .send(ControllerEvent::TerminalResizeFailed {
                            terminal_session_id,
                            reason: error.to_string(),
                        })
                        .await;
                }
            }
        });
    }

    /// ADR-045：终止（running）或清理（exited/killed tombstone）终端会话。
    /// 成功仅发 Succeeded 回执：running 的终态由 live TerminalExited 刷新，
    /// exited 清理由 UI 在回执后本地移除条目，不在此重复改 projection。
    pub fn terminal_close(&self, terminal_session_id: String) {
        let Some(client) = self.current_client() else {
            self.emit_reliable(ControllerEvent::TerminalCloseFailed {
                terminal_session_id,
                reason: "not connected".into(),
            });
            return;
        };
        let events = self.event_sender();
        self.runtime.spawn(async move {
            let command = terminal_close_command(&terminal_session_id);
            match client
                .command(command, command_source(), actor_identity())
                .await
            {
                Ok(response) if !matches!(&response.response, AppResponse::Error(_)) => {
                    let _ = events
                        .send(ControllerEvent::TerminalCloseSucceeded {
                            terminal_session_id,
                        })
                        .await;
                }
                Ok(response) => {
                    let _ = events
                        .send(ControllerEvent::TerminalCloseFailed {
                            terminal_session_id,
                            reason: format!("server returned {:?}", response.response),
                        })
                        .await;
                }
                Err(error) => {
                    // ADR-045：Close 的目标已从 Host 注册表消失（如本端此前
                    // 的 Stop 已就地注销）——not_found 是「条目不存在」的
                    // 权威确认，清理目标确定达成，按成功收敛让 UI 移除本地
                    // 条目，不把诚实 not_found 当失败卡死面板。
                    if matches!(
                        &error,
                        ClientError::Protocol(protocol)
                            if protocol.code == ProtocolErrorCode::RequestNotFound
                    ) {
                        let _ = events
                            .send(ControllerEvent::TerminalCloseSucceeded {
                                terminal_session_id,
                            })
                            .await;
                        return;
                    }
                    let _ = events
                        .send(ControllerEvent::TerminalCloseFailed {
                            terminal_session_id,
                            reason: error.to_string(),
                        })
                        .await;
                }
            }
        });
    }

    pub fn load_models(&self) {
        let Some(client) = self.current_client() else {
            return;
        };
        let events = self.event_sender();
        self.runtime.spawn(async move {
            let query = model_list_query();
            match client
                .query(query, command_source(), actor_identity())
                .await
            {
                Ok(response) => match parse_models(&response) {
                    Ok(models) => {
                        let _ = events.send(ControllerEvent::ModelsLoaded(models)).await;
                    }
                    Err(reason) => try_emit(
                        &events,
                        ControllerEvent::OperationFailed {
                            action: "load models",
                            reason,
                        },
                    ),
                },
                Err(error) => try_emit(
                    &events,
                    ControllerEvent::OperationFailed {
                        action: "load models",
                        reason: error.to_string(),
                    },
                ),
            }
        });
    }

    /// 拉取 Settings「模型与供应商」页只读状态（provider_auth_status，
    /// provider_id=None → 全部）。返回是否已派出（断线时由 UI 保留 stale
    /// 只读结果，不进入 loading）。
    pub fn load_provider_status(&self) -> bool {
        let Some(client) = self.current_client() else {
            return false;
        };
        let events = self.event_sender();
        self.runtime.spawn(async move {
            match client
                .query(
                    provider_auth_status_query(),
                    command_source(),
                    actor_identity(),
                )
                .await
            {
                Ok(response) => match parse_provider_status_response(&response) {
                    Ok(providers) => {
                        let _ = events
                            .send(ControllerEvent::ProviderStatusLoaded(providers))
                            .await;
                    }
                    Err(reason) => try_emit(
                        &events,
                        ControllerEvent::OperationFailed {
                            action: "load provider status",
                            reason,
                        },
                    ),
                },
                Err(error) => try_emit(
                    &events,
                    ControllerEvent::OperationFailed {
                        action: "load provider status",
                        reason: error.to_string(),
                    },
                ),
            }
        });
        true
    }

    /// 设为默认模型（set_default_model，非重放命令）。Data 确认后发
    /// `DefaultModelConfirmed`（Composer 同步）并重查 provider_auth_status
    /// 取回权威 default；Error / 传输失败经 OperationFailed 呈现，不动
    /// UI 现有状态。
    pub fn set_default_model(&self, provider_id: String, model_id: String) {
        let Some(client) = self.current_client() else {
            self.emit_reliable(ControllerEvent::OperationFailed {
                action: "set default model".into(),
                reason: "not connected".into(),
            });
            return;
        };
        let events = self.event_sender();
        self.runtime.spawn(async move {
            let command = set_default_model_command(&provider_id, &model_id);
            let response = match client
                .command(command, command_source(), actor_identity())
                .await
            {
                Ok(response) => response,
                Err(error) => {
                    try_emit(
                        &events,
                        ControllerEvent::OperationFailed {
                            action: "set default model",
                            reason: error.to_string(),
                        },
                    );
                    return;
                }
            };
            let confirmed = match parse_default_model_confirmation(&response) {
                Ok(confirmed) => confirmed,
                Err(reason) => {
                    try_emit(
                        &events,
                        ControllerEvent::OperationFailed {
                            action: "set default model",
                            reason,
                        },
                    );
                    return;
                }
            };
            let _ = events
                .send(ControllerEvent::DefaultModelConfirmed {
                    provider_id: confirmed.0,
                    model_id: confirmed.1,
                })
                .await;
            // 确认后重查权威 provider 状态（含 default）；失败走既有
            // load provider status 通道，UI 保留现有只读列表。
            match client
                .query(
                    provider_auth_status_query(),
                    command_source(),
                    actor_identity(),
                )
                .await
            {
                Ok(response) => match parse_provider_status_response(&response) {
                    Ok(data) => {
                        let _ = events
                            .send(ControllerEvent::ProviderStatusLoaded(data))
                            .await;
                    }
                    Err(reason) => try_emit(
                        &events,
                        ControllerEvent::OperationFailed {
                            action: "load provider status",
                            reason,
                        },
                    ),
                },
                Err(error) => try_emit(
                    &events,
                    ControllerEvent::OperationFailed {
                        action: "load provider status",
                        reason: error.to_string(),
                    },
                ),
            }
        });
    }

    /// 拉取 Settings「通用」页（general_settings）。返回是否已派出
    ///（断线时由 UI 保留 stale 只读结果，不进入 loading）。
    pub fn load_general_settings(&self) -> bool {
        let Some(client) = self.current_client() else {
            return false;
        };
        let events = self.event_sender();
        self.runtime.spawn(async move {
            match client
                .query(
                    general_settings_query(),
                    command_source(),
                    actor_identity(),
                )
                .await
            {
                Ok(response) => match parse_general_settings_response(&response) {
                    Ok(proxy_url) => {
                        let _ = events
                            .send(ControllerEvent::GeneralSettingsLoaded(proxy_url))
                            .await;
                    }
                    Err(reason) => try_emit(
                        &events,
                        ControllerEvent::OperationFailed {
                            action: "load general settings",
                            reason,
                        },
                    ),
                },
                Err(error) => try_emit(
                    &events,
                    ControllerEvent::OperationFailed {
                        action: "load general settings",
                        reason: error.to_string(),
                    },
                ),
            }
        });
        true
    }

    /// 设置或清除 Global `proxy_url`（set_proxy_url）。Data 确认后发
    /// `ProxyUrlConfirmed`（回执即写后状态）；Error / 传输失败经
    /// OperationFailed 呈现，不动 UI 现有生效值。
    pub fn set_proxy_url(&self, proxy_url: Option<String>) {
        let Some(client) = self.current_client() else {
            self.emit_reliable(ControllerEvent::OperationFailed {
                action: "set proxy url".into(),
                reason: "not connected".into(),
            });
            return;
        };
        let events = self.event_sender();
        self.runtime.spawn(async move {
            let command = set_proxy_url_command(proxy_url.as_deref());
            let response = match client
                .command(command, command_source(), actor_identity())
                .await
            {
                Ok(response) => response,
                Err(error) => {
                    try_emit(
                        &events,
                        ControllerEvent::OperationFailed {
                            action: "set proxy url",
                            reason: error.to_string(),
                        },
                    );
                    return;
                }
            };
            match parse_general_settings_response(&response) {
                Ok(confirmed) => {
                    let _ = events
                        .send(ControllerEvent::ProxyUrlConfirmed {
                            proxy_url: confirmed,
                        })
                        .await;
                }
                Err(reason) => try_emit(
                    &events,
                    ControllerEvent::OperationFailed {
                        action: "set proxy url",
                        reason,
                    },
                ),
            }
        });
    }

    /// 发起 OAuth 授权（auth_start）。响应只携带 verification_url /
    /// user_code / expires_at，进度经 AuthChanged 事件收敛。
    pub fn auth_start(&self, provider_id: String) {
        let Some(client) = self.current_client() else {
            self.emit_reliable(ControllerEvent::OperationFailed {
                action: "start provider auth".into(),
                reason: "not connected".into(),
            });
            return;
        };
        let events = self.event_sender();
        self.runtime.spawn(async move {
            let command = auth_start_command(&provider_id, "oauth");
            let response = match client
                .command(command, command_source(), actor_identity())
                .await
            {
                Ok(response) => response,
                Err(error) => {
                    try_emit(
                        &events,
                        ControllerEvent::OperationFailed {
                            action: "start provider auth",
                            reason: error.to_string(),
                        },
                    );
                    return;
                }
            };
            match parse_auth_started(&response) {
                Ok((verification_url, user_code, expires_at)) => {
                    let _ = events
                        .send(ControllerEvent::AuthStarted {
                            provider_id,
                            verification_url,
                            user_code,
                            expires_at,
                        })
                        .await;
                }
                Err(reason) => try_emit(
                    &events,
                    ControllerEvent::OperationFailed {
                        action: "start provider auth",
                        reason,
                    },
                ),
            }
        });
    }

    /// 提交并验证 API key（auth_set_api_key，非重放命令）。明文只在本次
    /// 调用栈上转成冻结 wire 命令后即弃：不写日志、不进事件 / projection /
    /// 持久状态；结果（含失败原因）由 Host 经 AuthChanged 下发。
    pub fn auth_set_api_key(&self, provider_id: String, api_key: String) {
        let Some(client) = self.current_client() else {
            self.emit_reliable(ControllerEvent::OperationFailed {
                action: "verify api key".into(),
                reason: "not connected".into(),
            });
            return;
        };
        let events = self.event_sender();
        self.runtime.spawn(async move {
            let command = auth_set_api_key_command(&provider_id, &api_key);
            if let Err(error) = client
                .command(command, command_source(), actor_identity())
                .await
            {
                try_emit(
                    &events,
                    ControllerEvent::OperationFailed {
                        action: "verify api key",
                        reason: error.to_string(),
                    },
                );
            }
            // 成功路径无回执事件：Host 已先经 AuthChanged::Succeeded 下发
            // 脱敏凭证，UI 状态由事件泵收敛。
        });
    }

    /// 取消进行中的 OAuth 等待（auth_cancel；对 api_key 验证无效，Host
    /// 返回结构化错误）。Cancelled 事件到达后 UI 复位。
    pub fn auth_cancel(&self, provider_id: String) {
        let Some(client) = self.current_client() else {
            self.emit_reliable(ControllerEvent::OperationFailed {
                action: "cancel provider auth".into(),
                reason: "not connected".into(),
            });
            return;
        };
        let events = self.event_sender();
        self.runtime.spawn(async move {
            let command = auth_cancel_command(&provider_id);
            if let Err(error) = client
                .command(command, command_source(), actor_identity())
                .await
            {
                try_emit(
                    &events,
                    ControllerEvent::OperationFailed {
                        action: "cancel provider auth",
                        reason: error.to_string(),
                    },
                );
            }
        });
    }

    /// 移除凭证（auth_remove；env 来源凭证由 Host 拒绝并说明）。Removed
    /// 事件到达后 UI 复位，失败经 OperationFailed 呈现。
    pub fn auth_remove(&self, provider_id: String) {
        let Some(client) = self.current_client() else {
            self.emit_reliable(ControllerEvent::OperationFailed {
                action: "remove provider auth".into(),
                reason: "not connected".into(),
            });
            return;
        };
        let events = self.event_sender();
        self.runtime.spawn(async move {
            let command = auth_remove_command(&provider_id);
            if let Err(error) = client
                .command(command, command_source(), actor_identity())
                .await
            {
                try_emit(
                    &events,
                    ControllerEvent::OperationFailed {
                        action: "remove provider auth",
                        reason: error.to_string(),
                    },
                );
            }
        });
    }

    /// 拉取 Changes 面文件清单（diff_list_files）。epoch 由 UI 递增，
    /// 响应原样带回，过期代次在 UI 侧丢弃。
    pub fn diff_list_files(&self, workspace_id: String, epoch: u64) {
        let Some(client) = self.current_client() else {
            self.emit_reliable(ControllerEvent::DiffFilesFailed {
                epoch,
                reason: "not connected".into(),
            });
            return;
        };
        let events = self.event_sender();
        self.runtime.spawn(async move {
            let query = diff_list_files_query(&workspace_id);
            match client
                .query(query, command_source(), actor_identity())
                .await
            {
                Ok(response) => match parse_diff_files(&response) {
                    Ok((session_id, files, git)) => {
                        let _ = events
                            .send(ControllerEvent::DiffFilesLoaded {
                                epoch,
                                session_id,
                                files,
                                git,
                            })
                            .await;
                    }
                    Err(reason) => {
                        let _ = events
                            .send(ControllerEvent::DiffFilesFailed { epoch, reason })
                            .await;
                    }
                },
                Err(error) => {
                    let _ = events
                        .send(ControllerEvent::DiffFilesFailed {
                            epoch,
                            reason: error.to_string(),
                        })
                        .await;
                }
            }
        });
    }

    /// 拉取单文件 diff（diff_get）。host 对不存在路径返回空 files，
    /// 解析为 None。
    pub fn diff_get(&self, workspace_id: String, path: String, epoch: u64) {
        let Some(client) = self.current_client() else {
            self.emit_reliable(ControllerEvent::DiffContentFailed {
                epoch,
                path,
                reason: "not connected".into(),
            });
            return;
        };
        let events = self.event_sender();
        self.runtime.spawn(async move {
            let query = diff_get_query(&workspace_id, &path);
            match client
                .query(query, command_source(), actor_identity())
                .await
            {
                Ok(response) => match parse_diff_file(&response) {
                    Ok((session_id, file)) => {
                        let _ = events
                            .send(ControllerEvent::DiffContentLoaded {
                                epoch,
                                path,
                                session_id,
                                file,
                            })
                            .await;
                    }
                    Err(reason) => {
                        let _ = events
                            .send(ControllerEvent::DiffContentFailed {
                                epoch,
                                path,
                                reason,
                            })
                            .await;
                    }
                },
                Err(error) => {
                    let _ = events
                        .send(ControllerEvent::DiffContentFailed {
                            epoch,
                            path,
                            reason: error.to_string(),
                        })
                        .await;
                }
            }
        });
    }

    /// 拉取 Resources 页 MCP server 清单（mcp_list）。
    pub fn mcp_list(&self, epoch: u64) {
        let Some(client) = self.current_client() else {
            self.emit_reliable(ControllerEvent::McpServersFailed {
                epoch,
                reason: "not connected".into(),
            });
            return;
        };
        let events = self.event_sender();
        self.runtime.spawn(async move {
            let query = mcp_list_query();
            match client
                .query(query, command_source(), actor_identity())
                .await
            {
                Ok(response) => match parse_mcp_servers(&response) {
                    Ok(servers) => {
                        let _ = events
                            .send(ControllerEvent::McpServersLoaded { epoch, servers })
                            .await;
                    }
                    Err(reason) => {
                        let _ = events
                            .send(ControllerEvent::McpServersFailed { epoch, reason })
                            .await;
                    }
                },
                Err(error) => {
                    let _ = events
                        .send(ControllerEvent::McpServersFailed {
                            epoch,
                            reason: error.to_string(),
                        })
                        .await;
                }
            }
        });
    }

    fn event_sender(&self) -> smol::channel::Sender<ControllerEvent> {
        self.state
            .events
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
            .expect("event channel exists after connect")
    }

    fn try_event_sender(&self) -> Option<smol::channel::Sender<ControllerEvent>> {
        self.state
            .events
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    /// 关键生命周期/回执事件不得被 512 槽 event channel 的瞬时峰值吞掉。
    /// 同步 API 里没有 await 点，因此把可靠投递交给 runtime；只有 UI 已
    /// 销毁（receiver 关闭）时才允许失败。
    fn emit_reliable(&self, event: ControllerEvent) {
        if let Some(events) = self.try_event_sender() {
            self.runtime.spawn(async move {
                let _ = events.send(event).await;
            });
        }
    }
}

fn try_emit(events: &smol::channel::Sender<ControllerEvent>, event: ControllerEvent) {
    let _ = events.try_send(event);
}

fn record_shared_last_acked(state: &SharedState, sequence: u64) {
    let mut slot = state.last_acked.lock().unwrap_or_else(|p| p.into_inner());
    *slot = Some(advance_last_acked(*slot, sequence));
}

fn advance_last_acked(current: Option<u64>, incoming: u64) -> u64 {
    current.map_or(incoming, |prev| prev.max(incoming))
}

fn desktop_client_config() -> ClientConfig {
    let mut config = ClientConfig::default();
    config.client_name = "pawork-desktop".into();
    config.capabilities = desktop_capabilities();
    config
}

fn desktop_capabilities() -> Vec<GuiCapability> {
    vec![
        GuiCapability::Events,
        GuiCapability::Snapshots,
        GuiCapability::Approvals,
        GuiCapability::TerminalStreaming,
    ]
}

/// source / identity 占位：服务端 host_stamp_command / host_stamp_query 会统一
/// 覆盖为 LocalGui + LocalUser（host/gui-server/src/session.rs），
/// 客户端只填必填信封字段，不伪造本地身份。
fn command_source() -> CommandSource {
    CommandSource::Automation
}

fn actor_identity() -> ActorIdentity {
    ActorIdentity::System
}

/// WorkspaceId / SessionId 等 domain id 未从 pawork-client re-export；命令与
/// 查询经冻结的 serde 形状（method/params）构造，避免引入第二个业务依赖。
fn session_create_command(workspace_id: &str) -> AppCommand {
    serde_json::from_value(json!({
        "method": "session_create",
        "params": { "workspace_id": workspace_id }
    }))
    .expect("session_create command shape is frozen")
}

fn workspace_add_command(root_path: &std::path::Path) -> AppCommand {
    serde_json::from_value(json!({
        "method": "workspace_add",
        "params": { "root_path": root_path.to_string_lossy() }
    }))
    .expect("workspace_add command shape is frozen")
}

fn session_fork_command(session_id: &str, parent_event_id: &str) -> AppCommand {
    serde_json::from_value(json!({
        "method": "session_fork",
        "params": {
            "session_id": session_id,
            "parent_event_id": parent_event_id
        }
    }))
    .expect("session_fork command shape is frozen")
}

fn is_workspace_relative_cwd(cwd: &str) -> bool {
    let trimmed = cwd.trim();
    if trimmed.is_empty() {
        return false;
    }
    let bytes = trimmed.as_bytes();
    let has_windows_prefix = bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':';
    !(trimmed.starts_with(['/', '\\'])
        || has_windows_prefix
        || trimmed
            .split(['/', '\\'])
            .any(|component| component == ".."))
}

fn terminal_create_command(workspace_id: &str, cwd: Option<&str>) -> Result<AppCommand, String> {
    let mut params = json!({ "workspace_id": workspace_id });
    if let Some(cwd) = cwd.map(str::trim).filter(|cwd| !cwd.is_empty()) {
        if !is_workspace_relative_cwd(cwd) {
            return Err("cwd must be a workspace-relative path".into());
        }
        params["working_directory"] = json!(cwd);
    }
    serde_json::from_value(json!({
        "method": "terminal_create",
        "params": params
    }))
    .map_err(|error| format!("terminal_create command shape: {error}"))
}

fn terminal_write_command(terminal_session_id: &str, data: &str) -> AppCommand {
    serde_json::from_value(json!({
        "method": "terminal_write",
        "params": {
            "terminal_session_id": terminal_session_id,
            "data": data
        }
    }))
    .expect("terminal_write command shape is frozen")
}

fn terminal_resize_command(terminal_session_id: &str, columns: u16, rows: u16) -> AppCommand {
    serde_json::from_value(json!({
        "method": "terminal_resize",
        "params": {
            "terminal_session_id": terminal_session_id,
            "columns": columns,
            "rows": rows
        }
    }))
    .expect("terminal_resize command shape is frozen")
}

fn terminal_close_command(terminal_session_id: &str) -> AppCommand {
    serde_json::from_value(json!({
        "method": "terminal_close",
        "params": {
            "terminal_session_id": terminal_session_id
        }
    }))
    .expect("terminal_close command shape is frozen")
}

fn auth_start_command(provider_id: &str, flow: &str) -> AppCommand {
    serde_json::from_value(json!({
        "method": "auth_start",
        "params": { "provider_id": provider_id, "flow": flow }
    }))
    .expect("auth_start command shape is frozen")
}

/// ApiKeySecret 在 wire 上是透明字符串；明文只在本函数栈上的 Value 里
/// 短暂停留，from_value 后即弃，不落任何字段或日志。
fn auth_set_api_key_command(provider_id: &str, api_key: &str) -> AppCommand {
    serde_json::from_value(json!({
        "method": "auth_set_api_key",
        "params": { "provider_id": provider_id, "api_key": api_key }
    }))
    .expect("auth_set_api_key command shape is frozen")
}

fn auth_cancel_command(provider_id: &str) -> AppCommand {
    serde_json::from_value(json!({
        "method": "auth_cancel",
        "params": { "provider_id": provider_id }
    }))
    .expect("auth_cancel command shape is frozen")
}

fn auth_remove_command(provider_id: &str) -> AppCommand {
    serde_json::from_value(json!({
        "method": "auth_remove",
        "params": { "provider_id": provider_id }
    }))
    .expect("auth_remove command shape is frozen")
}

fn set_default_model_command(provider_id: &str, model_id: &str) -> AppCommand {
    serde_json::from_value(json!({
        "method": "set_default_model",
        "params": { "provider_id": provider_id, "model_id": model_id }
    }))
    .expect("set_default_model command shape is frozen")
}

fn forked_session_id(response: &AppResponseEnvelope) -> Option<String> {
    match &response.response {
        AppResponse::Data(data) => data
            .get("session_id")
            .or_else(|| data.get("branch_id"))
            .and_then(|value| value.as_str())
            .map(str::to_string),
        _ => None,
    }
}

fn workspace_opened(response: &AppResponseEnvelope) -> Option<(String, String)> {
    match &response.response {
        AppResponse::Data(data) => Some((
            data.get("id")?.as_str()?.to_string(),
            data.get("name")?.as_str()?.to_string(),
        )),
        _ => None,
    }
}

fn terminal_session_id(response: &AppResponseEnvelope) -> Option<String> {
    match &response.response {
        AppResponse::Data(data) => data
            .get("terminal_session_id")
            .or_else(|| data.get("id"))
            .and_then(|value| value.as_str())
            .map(str::to_string),
        _ => None,
    }
}

fn session_get_query(session_id: &str, after: Option<u64>) -> AppQuery {
    serde_json::from_value(json!({
        "method": "session_get",
        "params": {
            "session_id": session_id,
            "timeline_after_sequence": after,
            "timeline_limit": PAGE_LIMIT
        }
    }))
    .expect("session_get query shape is frozen")
}

fn load_desktop_authentication(
    token_path: &std::path::Path,
) -> Result<ClientAuthentication, String> {
    let bytes = std::fs::read(token_path).map_err(|error| {
        format!(
            "gui token file not found or unreadable ({}): {error}",
            token_path.display()
        )
    })?;
    let text = String::from_utf8(bytes).map_err(|_| {
        format!(
            "gui token file is empty or malformed: {}",
            token_path.display()
        )
    })?;
    let proof = text.trim();
    if proof.is_empty() {
        return Err(format!(
            "gui token file is empty or malformed: {}",
            token_path.display()
        ));
    }
    Ok(ClientAuthentication {
        scheme: TOKEN_SCHEME.into(),
        proof: proof.to_string(),
    })
}

fn run_start_command(session_id: &str, text: &str, model: Option<&(String, String)>) -> AppCommand {
    let mut params = json!({
        "session_id": session_id,
        "user_message": text
    });
    if let Some((provider, id)) = model {
        params["provider"] = json!(provider);
        params["model"] = json!(id);
    }
    serde_json::from_value(json!({
        "method": "run_start",
        "params": params
    }))
    .expect("run_start command shape is frozen")
}

fn run_cancel_command(run_id: &str) -> AppCommand {
    serde_json::from_value(json!({
        "method": "run_cancel",
        "params": { "run_id": run_id }
    }))
    .expect("run_cancel command shape is frozen")
}

fn tool_approve_command(run_id: &str, tool_call_id: &str, decision: &str) -> AppCommand {
    serde_json::from_value(json!({
        "method": "tool_approve",
        "params": {
            "run_id": run_id,
            "tool_call_id": tool_call_id,
            "decision": decision
        }
    }))
    .expect("tool_approve command shape is frozen")
}

fn model_list_query() -> AppQuery {
    serde_json::from_value(json!({
        "method": "model_list",
        "params": {}
    }))
    .expect("model_list query shape is frozen")
}

fn provider_auth_status_query() -> AppQuery {
    serde_json::from_value(json!({
        "method": "provider_auth_status",
        "params": {}
    }))
    .expect("provider_auth_status query shape is frozen")
}

fn general_settings_query() -> AppQuery {
    serde_json::from_value(json!({
        "method": "general_settings"
    }))
    .expect("general_settings query shape is frozen")
}

fn set_proxy_url_command(proxy_url: Option<&str>) -> AppCommand {
    serde_json::from_value(json!({
        "method": "set_proxy_url",
        "params": { "proxy_url": proxy_url }
    }))
    .expect("set_proxy_url command shape is frozen")
}

fn diff_list_files_query(workspace_id: &str) -> AppQuery {
    serde_json::from_value(json!({
        "method": "diff_list_files",
        "params": { "workspace_id": workspace_id }
    }))
    .expect("diff_list_files query shape is frozen")
}

fn diff_get_query(workspace_id: &str, path: &str) -> AppQuery {
    serde_json::from_value(json!({
        "method": "diff_get",
        "params": {
            "workspace_id": workspace_id,
            "path": path
        }
    }))
    .expect("diff_get query shape is frozen")
}

fn mcp_list_query() -> AppQuery {
    serde_json::from_value(json!({
        "method": "mcp_list"
    }))
    .expect("mcp_list query shape is frozen")
}

fn parse_models(response: &AppResponseEnvelope) -> Result<Vec<ModelEntry>, String> {
    match &response.response {
        AppResponse::Data(data) => {
            let entries = data
                .as_array()
                .ok_or_else(|| "model list is not an array".to_string())?;
            Ok(entries
                .iter()
                .filter_map(|entry| {
                    let provider_id = entry.get("provider_id").and_then(|value| value.as_str())?;
                    let id = entry.get("id").and_then(|value| value.as_str())?;
                    let display_name = entry
                        .get("display_name")
                        .and_then(|value| value.as_str())
                        .unwrap_or(id);
                    Some(ModelEntry {
                        provider_id: provider_id.to_string(),
                        id: id.to_string(),
                        display_name: display_name.to_string(),
                        context_window_tokens: entry
                            .get("context_window_tokens")
                            .and_then(serde_json::Value::as_u64),
                    })
                })
                .collect())
        }
        AppResponse::Error(_) => Err("server returned an error response".into()),
        other => Err(format!("unexpected response: {other:?}")),
    }
}

/// 解包 provider_auth_status 信封：`AppResponse::Data` 载荷形如
/// `{"providers":[…]}`，条目解析在 projection（纯状态可单测）。
fn parse_provider_status_response(
    response: &AppResponseEnvelope,
) -> Result<SettingsProvidersData, String> {
    match &response.response {
        AppResponse::Data(data) => parse_provider_status_entries(data),
        AppResponse::Error(_) => Err("server returned an error response".into()),
        other => Err(format!("unexpected response: {other:?}")),
    }
}

/// 解包 general_settings / set_proxy_url 信封：Data 为
/// `{ "proxy_url": string | null }`；Error 取 Host 脱敏 message 原文
///（不含 proxy URL）。
fn parse_general_settings_response(
    response: &AppResponseEnvelope,
) -> Result<Option<String>, String> {
    match &response.response {
        AppResponse::Data(data) => parse_general_settings(data),
        AppResponse::Error(error) => Err(error.message.clone()),
        other => Err(format!("unexpected response: {other:?}")),
    }
}

/// 解包 set_default_model 响应：Data 携带 Host 确认的 provider/model pair。
fn parse_default_model_confirmation(
    response: &AppResponseEnvelope,
) -> Result<(String, String), String> {
    match &response.response {
        AppResponse::Data(data) => Ok((
            required_str(data, "provider_id")?,
            required_str(data, "model_id")?,
        )),
        AppResponse::Error(_) => Err("server returned an error response".into()),
        other => Err(format!("unexpected response: {other:?}")),
    }
}

/// 解包 auth_start 响应：verification_url 必填，user_code / expires_at
/// 仅 device flow 携带（PKCE 为 None）。
fn parse_auth_started(
    response: &AppResponseEnvelope,
) -> Result<(String, Option<String>, Option<String>), String> {
    match &response.response {
        AppResponse::Data(data) => {
            let verification_url = data
                .get("verification_url")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| "auth start missing verification_url".to_string())?
                .to_string();
            Ok((
                verification_url,
                optional_str(data, "user_code"),
                optional_str(data, "expires_at"),
            ))
        }
        AppResponse::Error(_) => Err("server returned an error response".into()),
        other => Err(format!("unexpected response: {other:?}")),
    }
}

fn timeline_page(response: &AppResponseEnvelope) -> Result<Option<TimelinePage>, String> {
    match &response.response {
        AppResponse::Data(data) => match data.get("timeline_page") {
            Some(page) => serde_json::from_value::<TimelinePage>(page.clone())
                .map(Some)
                .map_err(|error| format!("decode timeline page: {error}")),
            None => Ok(None),
        },
        AppResponse::Error(_) => Err("server returned an error response".into()),
        other => Err(format!("unexpected response: {other:?}")),
    }
}

fn required_str(entry: &serde_json::Value, field: &str) -> Result<String, String> {
    entry
        .get(field)
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| format!("entry missing {field}"))
}

fn optional_str(entry: &serde_json::Value, field: &str) -> Option<String> {
    entry
        .get(field)
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
}

/// diff_list_files 响应：session_id 在无会话（SessionNotFound 空响应）时缺失。
fn parse_diff_files(
    response: &AppResponseEnvelope,
) -> Result<(Option<String>, Vec<DiffFileSummary>, Option<GitDiffInfo>), String> {
    match &response.response {
        AppResponse::Data(data) => {
            let session_id = optional_str(data, "session_id");
            let files_value = data
                .get("files")
                .ok_or_else(|| "diff list missing files".to_string())?;
            let files = files_value
                .as_array()
                .ok_or_else(|| "diff files is not an array".to_string())?
                .iter()
                .map(|entry| {
                    Ok(DiffFileSummary {
                        path: required_str(entry, "path")?,
                        status: optional_str(entry, "status").unwrap_or_else(|| "unknown".into()),
                        additions: entry
                            .get("additions")
                            .and_then(serde_json::Value::as_u64)
                            .unwrap_or(0),
                        deletions: entry
                            .get("deletions")
                            .and_then(serde_json::Value::as_u64)
                            .unwrap_or(0),
                        binary: entry
                            .get("binary")
                            .and_then(serde_json::Value::as_bool)
                            .unwrap_or(false),
                    })
                })
                .collect::<Result<Vec<_>, String>>()?;
            let git = data.get("git").map(|git| GitDiffInfo {
                branch: optional_str(git, "branch"),
                work_dir: optional_str(git, "work_dir"),
                dirty_files: git.get("dirty_files").and_then(serde_json::Value::as_u64),
            });
            Ok((session_id, files, git))
        }
        AppResponse::Error(_) => Err("server returned an error response".into()),
        other => Err(format!("unexpected response: {other:?}")),
    }
}

/// diff_get 响应：带回 Host 实际解析的 latest session；files 为空（路径
/// 不在 diff / 无会话）时 file 为 None。
fn parse_diff_file(
    response: &AppResponseEnvelope,
) -> Result<(Option<String>, Option<DiffFileDetail>), String> {
    match &response.response {
        AppResponse::Data(data) => {
            let session_id = optional_str(data, "session_id");
            let Some(files) = data.get("files").and_then(serde_json::Value::as_array) else {
                return Err("diff response missing files".into());
            };
            let Some(entry) = files.first() else {
                return Ok((session_id, None));
            };
            let hunks = entry
                .get("hunks")
                .and_then(serde_json::Value::as_array)
                .map(|hunks| {
                    hunks
                        .iter()
                        .map(|hunk| DiffHunkDetail {
                            header: hunk
                                .get("header")
                                .and_then(serde_json::Value::as_str)
                                .unwrap_or("")
                                .to_string(),
                            lines: hunk
                                .get("lines")
                                .and_then(serde_json::Value::as_array)
                                .map(|lines| {
                                    lines
                                        .iter()
                                        .map(|line| DiffLineDetail {
                                            kind: match line
                                                .get("kind")
                                                .and_then(serde_json::Value::as_str)
                                            {
                                                Some("addition") => DiffLineKind::Addition,
                                                Some("deletion") => DiffLineKind::Deletion,
                                                _ => DiffLineKind::Context,
                                            },
                                            text: line
                                                .get("text")
                                                .and_then(serde_json::Value::as_str)
                                                .unwrap_or("")
                                                .to_string(),
                                        })
                                        .collect()
                                })
                                .unwrap_or_default(),
                        })
                        .collect()
                })
                .unwrap_or_default();
            Ok((
                session_id,
                Some(DiffFileDetail {
                    path: required_str(entry, "path")?,
                    previous_path: entry
                        .get("previous_path")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string),
                    status: optional_str(entry, "status").unwrap_or_else(|| "unknown".into()),
                    binary: entry
                        .get("binary")
                        .and_then(serde_json::Value::as_bool)
                        .unwrap_or(false),
                    additions: entry
                        .get("additions")
                        .and_then(serde_json::Value::as_u64)
                        .unwrap_or(0),
                    deletions: entry
                        .get("deletions")
                        .and_then(serde_json::Value::as_u64)
                        .unwrap_or(0),
                    hunks,
                }),
            ))
        }
        AppResponse::Error(_) => Err("server returned an error response".into()),
        other => Err(format!("unexpected response: {other:?}")),
    }
}

/// mcp_list 响应（形状由主代理钉死）：{"servers":[{name,transport,state,tools,last_error}]}。
fn parse_mcp_servers(response: &AppResponseEnvelope) -> Result<Vec<McpServerEntry>, String> {
    match &response.response {
        AppResponse::Data(data) => data
            .get("servers")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| "mcp list missing servers".to_string())?
            .iter()
            .map(|entry| {
                Ok(McpServerEntry {
                    name: required_str(entry, "name")?,
                    transport: optional_str(entry, "transport").unwrap_or_else(|| "unknown".into()),
                    state: optional_str(entry, "state").unwrap_or_else(|| "unknown".into()),
                    tool_count: entry
                        .get("tools")
                        .and_then(serde_json::Value::as_array)
                        .map(Vec::len)
                        .unwrap_or(0) as u64,
                    last_error: optional_str(entry, "last_error"),
                })
            })
            .collect(),
        AppResponse::Error(_) => Err("server returned an error response".into()),
        other => Err(format!("unexpected response: {other:?}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_token_file_fails_closed() {
        let err = load_desktop_authentication(std::path::Path::new(
            "/nonexistent/pawork-desktop-missing.token",
        ))
        .expect_err("missing token must fail");
        assert!(
            err.contains("not found") || err.contains("unreadable"),
            "{err}"
        );
    }

    #[test]
    fn run_start_command_writes_provider_and_model() {
        let command = run_start_command(
            "s-1",
            "hi",
            Some(&("deepseek".into(), "deepseek-v4-flash".into())),
        );
        let value = serde_json::to_value(&command).expect("serialize run_start");
        assert_eq!(value["method"], "run_start");
        assert_eq!(value["params"]["provider"], "deepseek");
        assert_eq!(value["params"]["model"], "deepseek-v4-flash");
    }

    #[test]
    fn last_acked_advances_from_events_and_acks() {
        assert_eq!(advance_last_acked(None, 4), 4);
        assert_eq!(advance_last_acked(Some(4), 2), 4);
        assert_eq!(advance_last_acked(Some(4), 9), 9);
    }

    #[test]
    fn handshake_capabilities_include_terminal_streaming() {
        assert!(desktop_capabilities().contains(&GuiCapability::TerminalStreaming));
    }

    #[test]
    fn session_fork_command_targets_event_id() {
        let command = session_fork_command("s-1", "evt-9");
        let value = serde_json::to_value(&command).expect("serialize fork");
        assert_eq!(value["method"], "session_fork");
        assert_eq!(value["params"]["session_id"], "s-1");
        assert_eq!(value["params"]["parent_event_id"], "evt-9");
    }

    #[test]
    fn terminal_commands_use_workspace_relative_cwd() {
        assert!(terminal_create_command("ws-1", Some("/tmp")).is_err());
        assert!(terminal_create_command("ws-1", Some("../secret")).is_err());
        assert!(terminal_create_command("ws-1", Some(r"C:\Windows")).is_err());
        let created = terminal_create_command("ws-1", Some("src/app")).expect("relative cwd");
        let value = serde_json::to_value(&created).expect("serialize create");
        assert_eq!(value["method"], "terminal_create");
        assert_eq!(value["params"]["workspace_id"], "ws-1");
        assert_eq!(value["params"]["working_directory"], "src/app");

        let write = serde_json::to_value(terminal_write_command("term-1", "ls\n")).unwrap();
        assert_eq!(write["method"], "terminal_write");
        assert_eq!(write["params"]["terminal_session_id"], "term-1");

        let resize = serde_json::to_value(terminal_resize_command("term-1", 80, 24)).unwrap();
        assert_eq!(resize["method"], "terminal_resize");
        assert_eq!(resize["params"]["columns"], 80);
        assert_eq!(resize["params"]["rows"], 24);

        let close = serde_json::to_value(terminal_close_command("term-1")).unwrap();
        assert_eq!(close["method"], "terminal_close");
        assert_eq!(close["params"]["terminal_session_id"], "term-1");
    }

    #[test]
    fn lifecycle_events_carry_workspace_terminal_and_epoch_identity() {
        let created = ControllerEvent::TerminalCreated {
            workspace_id: "ws-1".into(),
            terminal_session_id: "term-1".into(),
        };
        assert!(
            matches!(created, ControllerEvent::TerminalCreated { workspace_id, terminal_session_id }
            if workspace_id == "ws-1" && terminal_session_id == "term-1")
        );
        let failed = ControllerEvent::DiffFilesFailed {
            epoch: 7,
            reason: "stale".into(),
        };
        assert!(matches!(
            failed,
            ControllerEvent::DiffFilesFailed { epoch: 7, .. }
        ));
    }

    #[test]
    fn diff_and_mcp_queries_pin_wire_shapes() {
        let list = serde_json::to_value(diff_list_files_query("ws-1")).unwrap();
        assert_eq!(list["method"], "diff_list_files");
        assert_eq!(list["params"]["workspace_id"], "ws-1");

        let get = serde_json::to_value(diff_get_query("ws-1", "src/main.rs")).unwrap();
        assert_eq!(get["method"], "diff_get");
        assert_eq!(get["params"]["workspace_id"], "ws-1");
        assert_eq!(get["params"]["path"], "src/main.rs");

        let mcp = serde_json::to_value(mcp_list_query()).unwrap();
        assert_eq!(mcp["method"], "mcp_list");
        assert_eq!(mcp["params"], serde_json::Value::Null);
    }

    fn envelope(data: serde_json::Value) -> AppResponseEnvelope {
        serde_json::from_value(serde_json::json!({
            "api_version": { "major": 1, "minor": 1 },
            "request_id": "q-test",
            "responded_at": 0,
            "response": { "type": "data", "data": data }
        }))
        .expect("test response envelope")
    }

    #[test]
    fn parse_diff_files_reads_summaries_and_git() {
        let (session_id, files, git) = parse_diff_files(&envelope(serde_json::json!({
            "session_id": "s-1",
            "files": [
                {
                    "path": "src/app.rs",
                    "status": "modified",
                    "additions": 3,
                    "deletions": 1,
                    "binary": false
                },
                { "path": "logo.png", "status": "added", "additions": 0, "deletions": 0, "binary": true }
            ],
            "git": { "branch": "main", "work_dir": "/tmp/repo", "dirty_files": 4 }
        })))
        .expect("parse diff files");
        assert_eq!(session_id.as_deref(), Some("s-1"));
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].path, "src/app.rs");
        assert_eq!(files[0].status, "modified");
        assert_eq!((files[0].additions, files[0].deletions), (3, 1));
        assert!(files[1].binary);
        let git = git.expect("git info");
        assert_eq!(git.branch.as_deref(), Some("main"));
        assert_eq!(git.dirty_files, Some(4));
    }

    #[test]
    fn parse_diff_files_marks_no_session_response() {
        let (session_id, files, git) =
            parse_diff_files(&envelope(serde_json::json!({ "files": [] })))
                .expect("empty session response parses");
        assert_eq!(session_id, None);
        assert!(files.is_empty());
        assert_eq!(git, None);
    }

    #[test]
    fn parse_diff_file_reads_hunks_and_lines() {
        let (session_id, file) = parse_diff_file(&envelope(serde_json::json!({
            "session_id": "s-1",
            "path": "src/app.rs",
            "files": [{
                "path": "src/app.rs",
                "previous_path": null,
                "status": "modified",
                "binary": false,
                "additions": 1,
                "deletions": 1,
                "hunks": [{
                    "header": "@@ -1,2 +1,2 @@",
                    "lines": [
                        { "kind": "context", "text": "fn main() {" },
                        { "kind": "addition", "text": "    println!(\"new\");" },
                        { "kind": "deletion", "text": "    println!(\"old\");" }
                    ]
                }]
            }]
        })))
        .expect("parse diff file");
        assert_eq!(session_id.as_deref(), Some("s-1"));
        let file = file.expect("file present");
        assert_eq!(file.hunks.len(), 1);
        assert_eq!(file.hunks[0].header, "@@ -1,2 +1,2 @@");
        assert_eq!(file.hunks[0].lines.len(), 3);
        assert_eq!(file.hunks[0].lines[1].kind, DiffLineKind::Addition);
        assert_eq!(file.hunks[0].lines[2].kind, DiffLineKind::Deletion);
        assert_eq!(file.hunks[0].lines[2].text, "    println!(\"old\");");

        let (session_id, missing) = parse_diff_file(&envelope(serde_json::json!({
            "session_id": "s-2",
            "path": "gone.rs",
            "files": [],
            "complete": true
        })))
        .expect("empty diff parses");
        assert_eq!(session_id.as_deref(), Some("s-2"));
        assert_eq!(missing, None);
    }

    #[test]
    fn parse_mcp_servers_reads_pinned_shape() {
        let servers = parse_mcp_servers(&envelope(serde_json::json!({
            "servers": [
                {
                    "name": "fetch",
                    "transport": "stdio",
                    "state": "ready",
                    "tools": ["fetch_url", "search"],
                    "last_error": null
                },
                {
                    "name": "broken",
                    "transport": "http",
                    "state": "failed",
                    "tools": [],
                    "last_error": "connection refused"
                }
            ]
        })))
        .expect("parse mcp servers");
        assert_eq!(servers.len(), 2);
        assert_eq!(servers[0].tool_count, 2);
        assert_eq!(servers[0].last_error, None);
        assert_eq!(servers[1].state, "failed");
        assert_eq!(servers[1].last_error.as_deref(), Some("connection refused"));
    }
}
