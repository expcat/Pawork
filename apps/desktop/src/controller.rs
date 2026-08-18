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
    GuiCapability, GuiClient, GuiTransportClient, LocalTransport, ResumeDisposition, ResumeOutcome,
    Snapshot, TimelinePage, TransportEndpoint, TOKEN_SCHEME,
};
use serde_json::json;

use crate::projection::{sessions_in_snapshot, ModelEntry};

const PAGE_LIMIT: u32 = 500;
const MAX_PAGES: usize = 200;

/// UI 消费的控制器事件（经 smol channel 跨线程投递）。
#[derive(Clone, Debug)]
pub enum ControllerEvent {
    Disconnected { reason: String },
    Snapshot(Snapshot),
    TimelineLoaded { session_id: String, page: TimelinePage },
    Event(AppEventEnvelope),
    SessionCreated { session_id: String },
    MessageSent { session_id: String, run_id: String },
    ModelsLoaded(Vec<ModelEntry>),
    SessionForked { session_id: String },
    TerminalCreated { terminal_session_id: String },
    OperationFailed { action: &'static str, reason: String },
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
        self.state.client.lock().unwrap_or_else(|p| p.into_inner()).clone()
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
        let handshake = self
            .runtime
            .spawn(async move {
                GuiClient::connect_with_resume_config(
                    transport,
                    endpoint,
                    options,
                    Some(authentication),
                    last_ack,
                    desktop_client_config(),
                )
                .await
            })
            .await
            .map_err(|error| format!("connect task failed: {error}"))?
            .map_err(|error| error.to_string())?;
        let (handshake, resume) = handshake;
        let mut snapshot = handshake
            .initial_snapshot()
            .ok_or_else(|| "handshake did not deliver an initial snapshot".to_string())?;
        if last_ack.is_none() {
            self.record_last_acked(snapshot.snapshot_sequence.0);
            let _ = handshake.ack(snapshot.snapshot_sequence).await;
        }
        if let Some(outcome) = &resume {
            match &outcome.disposition {
                ResumeDisposition::Replay { through_sequence, .. } => {
                    self.record_last_acked(through_sequence.0);
                    let _ = handshake.ack(*through_sequence).await;
                }
                ResumeDisposition::UpToDate { current_sequence } => {
                    self.record_last_acked(current_sequence.0);
                }
                ResumeDisposition::SnapshotRequired { .. } => {
                    if let Some(fresh) = &outcome.snapshot {
                        snapshot = fresh.clone();
                    }
                    self.record_last_acked(snapshot.snapshot_sequence.0);
                    let _ = handshake.ack(snapshot.snapshot_sequence).await;
                }
            }
        }
        handshake
            .subscribe_all()
            .await
            .map_err(|error| error.to_string())?;

        *self.state.client.lock().unwrap_or_else(|p| p.into_inner()) = Some(handshake.clone());
        *self.state.events.lock().unwrap_or_else(|p| p.into_inner()) = Some(sender.clone());

        let pump_client = handshake.clone();
        let pump_events = sender;
        let pump_state = Arc::clone(&self.state);
        self.runtime.spawn(async move {
            loop {
                match pump_client.next_event_timeout(Duration::from_secs(1)).await {
                    Ok(event) => {
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
                    Err(ClientError::Timeout { .. }) => continue,
                    Err(error) => {
                        let reason = error.to_string();
                        *pump_state
                            .client
                            .lock()
                            .unwrap_or_else(|p| p.into_inner()) = None;
                        let _ = pump_events
                            .try_send(ControllerEvent::Disconnected { reason });
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

    pub(crate) fn record_last_acked(&self, sequence: u64) {
        record_shared_last_acked(&self.state, sequence);
    }

    /// 分页加载 session 时间线：SessionGet 按 timeline_after_sequence 链式
    /// 拉取直到 complete；分页期间先到的 live 事件由 projection 按 sequence
    /// 去重（gui-design §4.1 第 3 条）。
    pub fn open_session(&self, session_id: String) {
        let Some(client) = self.current_client() else {
            return;
        };
        let events = self.event_sender();
        self.runtime.spawn(async move {
            let mut after: Option<u64> = None;
            for _ in 0..MAX_PAGES {
                let query = session_get_query(&session_id, after);
                let response = match client.query(query, command_source(), actor_identity()).await {
                    Ok(response) => response,
                    Err(error) => {
                        try_emit(&events, ControllerEvent::OperationFailed {
                            action: "open session",
                            reason: error.to_string(),
                        });
                        return;
                    }
                };
                let page = match timeline_page(&response) {
                    Ok(Some(page)) => page,
                    Ok(None) => return,
                    Err(reason) => {
                        try_emit(&events, ControllerEvent::OperationFailed {
                            action: "open session",
                            reason,
                        });
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
            try_emit(&events, ControllerEvent::OperationFailed {
                action: "open session",
                reason: format!("timeline exceeded {MAX_PAGES} pages"),
            });
        });
    }

    /// 新建 session：SessionCreate 只回 Accepted（无 session id），重取 snapshot
    /// 挑 updated_at_ms 最新的 session 返回（host gui_host 行为）。
    pub fn create_session(&self, workspace_id: String) {
        let Some(client) = self.current_client() else {
            if let Some(events) = self.try_event_sender() {
                try_emit(&events, ControllerEvent::OperationFailed {
                    action: "create session",
                    reason: "not connected".into(),
                });
            }
            return;
        };
        let events = self.event_sender();
        self.runtime.spawn(async move {
            let command = session_create_command(&workspace_id);
            if let Err(error) =
                client.command(command, command_source(), actor_identity()).await
            {
                try_emit(&events, ControllerEvent::OperationFailed {
                    action: "create session",
                    reason: error.to_string(),
                });
                return;
            }
            match client.snapshot().await {
                Ok(snapshot) => {
                    let latest = sessions_in_snapshot(&snapshot)
                        .into_iter()
                        .map(|session| session.session_id)
                        .next();
                    if events.send(ControllerEvent::Snapshot(snapshot)).await.is_err() {
                        return;
                    }
                    if let Some(session_id) = latest {
                        let _ = events
                            .send(ControllerEvent::SessionCreated { session_id })
                            .await;
                    } else {
                        try_emit(&events, ControllerEvent::OperationFailed {
                            action: "create session",
                            reason: "host accepted SessionCreate but snapshot has no sessions".into(),
                        });
                    }
                }
                Err(error) => {
                    try_emit(&events, ControllerEvent::OperationFailed {
                        action: "create session",
                        reason: error.to_string(),
                    });
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
            match client.command(command, command_source(), actor_identity()).await {
                Ok(response) => match response.response {
                    AppResponse::Accepted { run_id: Some(run_id), .. } => {
                        let run_id = run_id.as_str().to_string();
                        let _ = events
                            .send(ControllerEvent::MessageSent {
                                session_id,
                                run_id,
                            })
                            .await;
                    }
                    other => {
                        try_emit(&events, ControllerEvent::OperationFailed {
                            action: "send message",
                            reason: format!("unexpected response: {other:?}"),
                        });
                    }
                },
                Err(error) => {
                    try_emit(&events, ControllerEvent::OperationFailed {
                        action: "send message",
                        reason: error.to_string(),
                    });
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
                try_emit(&events, ControllerEvent::OperationFailed {
                    action: "cancel run",
                    reason: error.to_string(),
                });
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
                try_emit(&events, ControllerEvent::OperationFailed {
                    action: "approve tool",
                    reason: error.to_string(),
                });
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
        let client = self.current_client().ok_or_else(|| "not connected".to_string())?;
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
                try_emit(&events, ControllerEvent::OperationFailed {
                    action: "fork session",
                    reason: "not connected".into(),
                });
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
                        try_emit(&events, ControllerEvent::OperationFailed {
                            action: "fork session",
                            reason: "server returned an error response".into(),
                        });
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
                                if events.send(ControllerEvent::Snapshot(snapshot)).await.is_err()
                                {
                                    return;
                                }
                                if let Some(session_id) = latest {
                                    let _ = events
                                        .send(ControllerEvent::SessionForked { session_id })
                                        .await;
                                }
                            }
                            Err(error) => try_emit(&events, ControllerEvent::OperationFailed {
                                action: "fork session",
                                reason: error.to_string(),
                            }),
                        }
                    }
                    other => try_emit(&events, ControllerEvent::OperationFailed {
                        action: "fork session",
                        reason: format!("unexpected response: {other:?}"),
                    }),
                },
                Err(error) => try_emit(&events, ControllerEvent::OperationFailed {
                    action: "fork session",
                    reason: error.to_string(),
                }),
            }
        });
    }

    pub fn terminal_create(&self, workspace_id: String, cwd: Option<String>) {
        let Some(client) = self.current_client() else {
            if let Some(events) = self.try_event_sender() {
                try_emit(&events, ControllerEvent::OperationFailed {
                    action: "create terminal",
                    reason: "not connected".into(),
                });
            }
            return;
        };
        let events = self.event_sender();
        self.runtime.spawn(async move {
            let command = match terminal_create_command(&workspace_id, cwd.as_deref()) {
                Ok(command) => command,
                Err(reason) => {
                    try_emit(&events, ControllerEvent::OperationFailed {
                        action: "create terminal",
                        reason,
                    });
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
                            .send(ControllerEvent::TerminalCreated { terminal_session_id })
                            .await;
                    }
                    None => try_emit(&events, ControllerEvent::OperationFailed {
                        action: "create terminal",
                        reason: format!("unexpected response: {:?}", response.response),
                    }),
                },
                Err(error) => try_emit(&events, ControllerEvent::OperationFailed {
                    action: "create terminal",
                    reason: error.to_string(),
                }),
            }
        });
    }

    pub fn terminal_write(&self, terminal_session_id: String, data: String) {
        let Some(client) = self.current_client() else {
            return;
        };
        let events = self.event_sender();
        self.runtime.spawn(async move {
            let command = terminal_write_command(&terminal_session_id, &data);
            if let Err(error) = client
                .command(command, command_source(), actor_identity())
                .await
            {
                try_emit(&events, ControllerEvent::OperationFailed {
                    action: "write terminal",
                    reason: error.to_string(),
                });
            }
        });
    }

    pub fn terminal_resize(&self, terminal_session_id: String, columns: u16, rows: u16) {
        let Some(client) = self.current_client() else {
            return;
        };
        let events = self.event_sender();
        self.runtime.spawn(async move {
            let command = terminal_resize_command(&terminal_session_id, columns, rows);
            if let Err(error) = client
                .command(command, command_source(), actor_identity())
                .await
            {
                try_emit(&events, ControllerEvent::OperationFailed {
                    action: "resize terminal",
                    reason: error.to_string(),
                });
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
            match client.query(query, command_source(), actor_identity()).await {
                Ok(response) => match parse_models(&response) {
                    Ok(models) => {
                        let _ = events.send(ControllerEvent::ModelsLoaded(models)).await;
                    }
                    Err(reason) => try_emit(&events, ControllerEvent::OperationFailed {
                        action: "load models",
                        reason,
                    }),
                },
                Err(error) => try_emit(&events, ControllerEvent::OperationFailed {
                    action: "load models",
                    reason: error.to_string(),
                }),
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
}

fn try_emit(
    events: &smol::channel::Sender<ControllerEvent>,
    event: ControllerEvent,
) {
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
    let has_windows_prefix =
        bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':';
    !(trimmed.starts_with(['/', '\\'])
        || has_windows_prefix
        || trimmed.split(['/', '\\']).any(|component| component == ".."))
}

fn terminal_create_command(
    workspace_id: &str,
    cwd: Option<&str>,
) -> Result<AppCommand, String> {
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

fn load_desktop_authentication(token_path: &std::path::Path) -> Result<ClientAuthentication, String> {
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

fn run_start_command(
    session_id: &str,
    text: &str,
    model: Option<&(String, String)>,
) -> AppCommand {
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

fn parse_models(response: &AppResponseEnvelope) -> Result<Vec<ModelEntry>, String> {
    match &response.response {
        AppResponse::Data(data) => {
            let entries = data.as_array().ok_or_else(|| "model list is not an array".to_string())?;
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
    }
}
