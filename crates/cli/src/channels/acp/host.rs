//! ACP Host 胶水层：把 ACP v1 JSON-RPC 消息接到 canonical 执行面。
//!
//! 本层只做三件事：
//!
//! 1. **握手与协商**：`initialize` 校验 `protocolVersion == 1`（拒绝实验 v2），
//!    经 [`AcpClientAdapterFactory`] 生成协商 adapter，未支持能力显式降级记录。
//! 2. **会话生命周期**：`session/new`（SessionCreate → Attach）、`session/prompt`
//!    （RunStart + 等待终态）、`session/resume`（Reattach）、`session/close`
//!    （RunCancel → Disconnect）、`session/cancel` / `$/cancel_request` 通知。
//! 3. **事件回译**：从 [`AcpCommandHost::subscribe`] 收取 canonical 事件，
//!    按 run → client session 归属路由，经 adapter 编码为 `session/update`
//!    通知或 `session/request_permission` 请求。
//!
//! 所有权/凭证/Core 一律不在这里重建：session 记录只读写
//! [`SessionRegistry`]，命令/查询全部经 [`AcpCommandHost`]。本 crate 不依赖
//! `pawork-app`，也不接入 S11 审计控制面。
//!
//! 并发模型：单 actor 循环独占 5 张 map + negotiated + outbox；公开 API 经
//! mpsc 信箱进出。prompt 串行只覆盖建立临界区（reserve → dispatch → bind）；
//! 绑定完成后 turn 执行期跨会话可并发。cancel / fail-closed 走独立紧急信箱，
//! 不被活跃 prompt 队头或 Core dispatch 等待阻塞。禁止 `std::sync::Mutex` / `RwLock`。

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use pawork_domain::{ConnectionId, DegradeEvent, DegradeKind, DegradeSeverity, QueryId, RunId, SessionId, ToolCallId, WorkspaceId};
use pawork_protocol::adapter::{
    AdapterError, AdapterSessionContext, AdapterWireFrame, CanonicalClientRequest,
    CanonicalCoreFrame, CapabilitySnapshot, ClientAdapter, ClientCapability, ClientProtocol,
    ClientSessionId, ClientSessionRecord, ClientSessionState, SessionRegistry,
    CLIENT_ADAPTER_SCHEMA_VERSION,
};
use pawork_protocol::{
    ActorIdentity, AppCommand, AppCommandEnvelope, AppEvent, AppEventEnvelope, AppQuery,
    AppQueryEnvelope, AppResponse, ApprovalDecision, CommandSource, EventStream, RunState,
    API_VERSION,
};
use serde_json::{json, Value};
use tokio::sync::{broadcast, mpsc, oneshot, watch};

use crate::channels::acp::adapter::{
    AcpClientAdapter, AcpClientAdapterFactory, CwdResolver, NegotiatedAcpAdapter, SessionResolver,
};
use crate::channels::acp::command_host::{AcpCommandHost, AcpHostError};
use crate::channels::acp::map;
use crate::channels::acp::now_timestamp;
use crate::channels::acp::wire::{
    CancelRequestParams, ClientCapabilities, Implementation, InitializeParams, InitializeResult,
    JsonRpcError, JsonRpcId, JsonRpcNotification, JsonRpcRequest, ParamsExt, SessionNewResult,
    SessionPromptResult, StopReason, ERROR_INVALID_REQUEST, ERROR_METHOD_NOT_FOUND,
    ERROR_REQUEST_CANCELLED, PROTOCOL_VERSION,
};

/// 连接级 ACP 身份（写在 wire `agentInfo` 与 canonical `ActorIdentity::Automation`）。
pub const ACP_AGENT_NAME: &str = "pawork-acp";
pub const ACP_AGENT_VERSION: &str = "0.0.0";

/// 每连接唯一后缀：同一进程内多个 AcpHost（多连接）必须拥有各自独立的
/// `connection_id`，否则 cross-connection resume 的 claim 无法区分新旧连接。
static ACP_CONNECTION_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// cwd → workspace 解析：cwd 必须位于某个已登记 workspace root 内
/// （组件级前缀匹配），否则显式 `HostUnavailable`。不静默 `WorkspaceAdd`。
struct HostCwdResolver {
    command_host: Arc<dyn AcpCommandHost>,
    identity_name: String,
    next_query: AtomicU64,
}

#[async_trait::async_trait]
impl CwdResolver for HostCwdResolver {
    async fn resolve(&self, cwd: &str) -> Result<WorkspaceId, AdapterError> {
        let request_id = format!(
            "cwd-resolve-{}",
            self.next_query.fetch_add(1, Ordering::SeqCst)
        );
        let envelope = AppQueryEnvelope {
            api_version: API_VERSION,
            request_id: QueryId::from(format!("acp-{request_id}")),
            source: CommandSource::Automation,
            identity: ActorIdentity::Automation {
                name: self.identity_name.clone(),
            },
            issued_at: now_timestamp(),
            query: AppQuery::WorkspaceList,
        };
        let response = self
            .command_host
            .query(envelope)
            .await
            .map_err(|error| AdapterError::HostUnavailable(error.to_string()))?;
        let AppResponse::Data(value) = response.response else {
            return Err(AdapterError::HostUnavailable(
                "workspace list query failed; cannot resolve cwd".into(),
            ));
        };
        // 规范化两侧再比较：进程登记的 root 来自 `current_dir()`（macOS 上
        // 已解析 /var → /private/var），而 ACP 客户端传入的 cwd 常是原始
        // 环境变量路径（未解析 symlink / 含重复分隔符）。
        let cwd_path = std::fs::canonicalize(cwd).unwrap_or_else(|_| PathBuf::from(cwd));
        let Some(workspaces) = value.as_array() else {
            return Err(AdapterError::HostUnavailable(
                "workspace list query returned an unexpected shape".into(),
            ));
        };
        for workspace in workspaces {
            let Some(workspace_id) = workspace.get("id").and_then(Value::as_str) else {
                continue;
            };
            let Some(roots) = workspace.get("roots").and_then(Value::as_array) else {
                continue;
            };
            for root in roots {
                let Some(root_path) = root.get("path").and_then(Value::as_str) else {
                    continue;
                };
                let root_path =
                    std::fs::canonicalize(root_path).unwrap_or_else(|_| PathBuf::from(root_path));
                if cwd_path.starts_with(&root_path) {
                    return Ok(WorkspaceId::from(workspace_id));
                }
            }
        }
        Err(AdapterError::HostUnavailable(format!(
            "cwd `{cwd}` is not inside any registered workspace root"
        )))
    }
}

/// 一次 prompt 的结果（run 终态时经 flush barrier 投递）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PromptResolution {
    Stopped(StopReason),
    Failed,
}

/// 单一有序 outbox 条目：协议帧，或 prompt 终态冲刷屏障。
#[derive(Debug)]
pub enum OutboxItem {
    /// 一条待写出的 JSON-RPC 帧（通知 / 请求）。
    Frame(Value),
    /// 冲刷屏障：消费方把此前所有 `Frame` 写出（或取走）后才允许释放该 prompt。
    FlushBarrier {
        completion: tokio::sync::mpsc::Sender<PromptResolution>,
        resolution: PromptResolution,
    },
}

struct PendingPrompt {
    client_session_id: ClientSessionId,
    run_id: RunId,
    completion: tokio::sync::mpsc::Sender<PromptResolution>,
}

struct PendingPermission {
    run_id: RunId,
    tool_call_id: ToolCallId,
    client_session_id: ClientSessionId,
}

/// 同 session 一次 prompt 的原子占用：Reserved 是注册窗口占位，Active 才绑定 run。
struct PromptOccupancy {
    request_id: JsonRpcId,
    run_id: Option<RunId>,
    early_session_cancel: bool,
    early_request_cancel: bool,
}

#[derive(Clone, Default)]
struct HostSnapshot {
    occupancy: BTreeMap<ClientSessionId, Option<RunId>>,
    run_sessions: BTreeMap<RunId, ClientSessionId>,
    initialized: bool,
    degraded: Vec<ClientCapability>,
}

enum Mail {
    Request {
        id: JsonRpcId,
        method: String,
        params: Option<Value>,
        reply: oneshot::Sender<Result<RequestStart, JsonRpcError>>,
    },
    Notification {
        method: String,
        params: Option<Value>,
        reply: oneshot::Sender<Result<(), JsonRpcError>>,
    },
    Response {
        id: JsonRpcId,
        result: Result<Value, JsonRpcError>,
        reply: oneshot::Sender<Result<(), JsonRpcError>>,
    },
    DrainAndPump {
        reply: oneshot::Sender<()>,
    },
    PumpEvents {
        events: Vec<AppEventEnvelope>,
        reply: oneshot::Sender<()>,
    },
    DrainOutbox {
        reply: std::sync::mpsc::Sender<Vec<OutboxItem>>,
    },
}

enum RequestStart {
    Ready(Value),
    Prompt(mpsc::Receiver<PromptResolution>),
}

enum UrgentMail {
    Notification {
        method: String,
        params: Option<Value>,
        reply: oneshot::Sender<Result<(), JsonRpcError>>,
    },
    FailClosed {
        reason: String,
        reply: std::sync::mpsc::Sender<()>,
    },
}

/// ACP v1 宿主（in-process 胶水，无传输假设）。
pub struct AcpHost {
    command_host: Arc<dyn AcpCommandHost>,
    registry: Arc<SessionRegistry>,
    connection_id: ConnectionId,
    mail_tx: mpsc::UnboundedSender<Mail>,
    urgent_tx: mpsc::UnboundedSender<UrgentMail>,
    snapshot_rx: watch::Receiver<HostSnapshot>,
}

impl AcpHost {
    pub fn new(command_host: Arc<dyn AcpCommandHost>, registry: Arc<SessionRegistry>) -> Self {
        let event_rx = command_host.subscribe();
        let identity_name = format!("acp:{ACP_AGENT_NAME}");
        let cwd_resolver = Arc::new(HostCwdResolver {
            command_host: Arc::clone(&command_host),
            identity_name: identity_name.clone(),
            next_query: AtomicU64::new(0),
        });
        let connection_id = ConnectionId::from(format!(
            "acp-connection-{}-{}",
            std::process::id(),
            ACP_CONNECTION_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let (mail_tx, mail_rx) = mpsc::unbounded_channel();
        let (urgent_tx, urgent_rx) = mpsc::unbounded_channel();
        let (snapshot_tx, snapshot_rx) = watch::channel(HostSnapshot::default());
        let factory = AcpClientAdapterFactory::new(
            crate::channels::acp::adapter::ACP_SUPPORTED_CAPABILITIES
                .iter()
                .map(|name| ClientCapability::new(*name)),
            Arc::clone(&registry),
            cwd_resolver,
            Arc::new(SnapshotSessionResolver {
                snapshot_rx: snapshot_rx.clone(),
            }),
            Implementation {
                name: ACP_AGENT_NAME.into(),
                title: Some("Pawork ACP Host".into()),
                version: ACP_AGENT_VERSION.into(),
            },
        );
        let actor = AcpActor {
            command_host: Arc::clone(&command_host),
            registry: Arc::clone(&registry),
            factory,
            connection_id: connection_id.clone(),
            negotiated: None,
            session_contexts: BTreeMap::new(),
            occupancy: BTreeMap::new(),
            run_sessions: BTreeMap::new(),
            pending_prompts: HashMap::new(),
            pending_permissions: HashMap::new(),
            outbox: VecDeque::new(),
            held_events: VecDeque::new(),
            next_request_id: 1,
            event_rx,
            snapshot_tx,
            mail_rx,
            urgent_rx,
            deferred_mail: VecDeque::new(),
        };
        // Actor 状态本身是 Send，但公开 API 含同步 drain/fail-closed 等待。
        // 挂到调用方 ambient runtime 时，current-thread 测试 runtime 上的
        // std recv 会冻住唯一 worker，actor 无法回执。因此仍用独立 OS 线程
        // + current_thread runtime。Core dispatch/query 已 tokio::spawn 到
        // 该 runtime；GuiHostAdapter 的工作线程不受此循环占用。
        if let Err(error) = std::thread::Builder::new()
            .name("pawork-acp-actor".into())
            .spawn(move || match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime.block_on(actor.run()),
                Err(error) => report_acp_state(
                    "failed to start ACP actor runtime",
                    json!({ "error": error.to_string() }),
                ),
            })
        {
            report_acp_state(
                "failed to spawn ACP actor thread",
                json!({ "error": error.to_string() }),
            );
        }
        Self {
            command_host,
            registry,
            connection_id,
            mail_tx,
            urgent_tx,
            snapshot_rx,
        }
    }

    pub fn registry(&self) -> &Arc<SessionRegistry> {
        &self.registry
    }

    /// 本连接唯一标识（authoritative registry 中记录的 connection_id；
    /// 跨连接 resume 后记录会 claim 到新连接）。
    pub fn connection_id(&self) -> &ConnectionId {
        &self.connection_id
    }

    /// 订阅 Core 事件流（独立 receiver；正式宿主事件源见 [`AcpHost::drain_and_pump`]）。
    pub fn subscribe(&self) -> broadcast::Receiver<AppEventEnvelope> {
        self.command_host.subscribe()
    }

    /// 取走全部当前可读的出站条目（同步、非阻塞；传输层冲刷用，保持队列顺序）。
    pub fn drain_outbox_items(&self) -> Vec<OutboxItem> {
        let (reply, rx) = std::sync::mpsc::channel();
        if self.mail_tx.send(Mail::DrainOutbox { reply }).is_err() {
            return Vec::new();
        }
        wait_std(rx).unwrap_or_default()
    }

    /// 取走出站 JSON-RPC 消息（通知 + 请求），并清空 outbox；队列中的冲刷
    /// 屏障在把此前帧全部取走后就地释放。
    pub fn take_outbox(&self) -> Vec<Value> {
        let mut frames = Vec::new();
        for item in self.drain_outbox_items() {
            match item {
                OutboxItem::Frame(frame) => frames.push(frame),
                OutboxItem::FlushBarrier {
                    completion,
                    resolution,
                } => {
                    if let Err(error) = completion.try_send(resolution) {
                        tracing::debug!(?error, "acp prompt completion dropped");
                    }
                }
            }
        }
        frames
    }

    /// 传输层失败收尾：丢弃无法写出的帧，但仍释放队列中全部 prompt 屏障。
    pub fn resolve_queued_prompts(&self) {
        for item in self.drain_outbox_items() {
            if let OutboxItem::FlushBarrier {
                completion,
                resolution,
            } = item
            {
                if let Err(error) = completion.try_send(resolution) {
                    tracing::debug!(?error, "acp prompt completion dropped");
                }
            }
        }
    }

    /// 释放调用方已 drain 但仍未写出的剩余屏障。
    pub fn release_drained_barriers(items: impl IntoIterator<Item = OutboxItem>) {
        for item in items {
            if let OutboxItem::FlushBarrier {
                completion,
                resolution,
            } = item
            {
                if let Err(error) = completion.try_send(resolution) {
                    tracing::debug!(?error, "acp prompt completion dropped");
                }
            }
        }
    }

    /// 订阅滞后且无法可靠补事件时 fail-closed：解除全部未决 prompt / 权限请求。
    /// 等待 actor 回执后再返回，调用方随后读到的 occupancy / has_active_runs 已收敛。
    pub fn fail_closed_all_prompts(&self, reason: &str) {
        let (reply, rx) = std::sync::mpsc::channel();
        if self
            .urgent_tx
            .send(UrgentMail::FailClosed {
                reason: reason.to_string(),
                reply,
            })
            .is_err()
        {
            return;
        }
        // 同步签名保持不变（acp.rs 装配点不改）。actor 在独立线程上回执，
        // recv 返回后 occupancy / has_active_runs 已收敛。
        if wait_std(rx).is_none() {
            tracing::debug!("acp fail-closed reply dropped");
        }
    }

    /// 当前是否有未完成 run（供事件泵循环判定退出）。
    pub fn has_active_runs(&self) -> bool {
        !self.snapshot_rx.borrow().occupancy.is_empty()
    }

    /// 指定 client session 当前绑定的 run id。
    pub fn pending_run(&self, client_session_id: &ClientSessionId) -> Option<RunId> {
        self.snapshot_rx
            .borrow()
            .occupancy
            .get(client_session_id)
            .and_then(|run_id| run_id.clone())
    }

    /// 握手时被显式降级的客户端能力清单（协商审计）。
    pub fn degraded_capabilities(&self) -> Vec<ClientCapability> {
        self.snapshot_rx.borrow().degraded.clone()
    }

    /// 是否已完成 initialize。
    pub fn is_initialized(&self) -> bool {
        self.snapshot_rx.borrow().initialized
    }

    /// 处理 client → agent 的 JSON-RPC 请求，返回 result（或 JSON-RPC 错误）。
    /// `session/prompt` 会等待 run 终态后才返回。
    pub async fn handle_request(
        &self,
        id: JsonRpcId,
        method: &str,
        params: Option<Value>,
    ) -> Result<Value, JsonRpcError> {
        let (reply, rx) = oneshot::channel();
        self.mail_tx
            .send(Mail::Request {
                id,
                method: method.to_string(),
                params,
                reply,
            })
            .map_err(|_| actor_unavailable())?;
        let started = rx.await.map_err(|_| actor_unavailable())?;
        let started = match started {
            Ok(started) => started,
            Err(error) => return Err(error),
        };
        match started {
            RequestStart::Ready(value) => Ok(value),
            RequestStart::Prompt(mut completion_rx) => match completion_rx.recv().await {
                Some(PromptResolution::Stopped(reason)) => serialize_value(
                    SessionPromptResult {
                        stop_reason: reason,
                    },
                    "SessionPromptResult",
                ),
                Some(PromptResolution::Failed) => Err(JsonRpcError::new(
                    crate::channels::acp::wire::ERROR_INTERNAL,
                    "prompt turn failed in Core",
                )),
                None => Err(JsonRpcError::new(
                    crate::channels::acp::wire::ERROR_INTERNAL,
                    "prompt turn ended without a resolution",
                )),
            },
        }
    }

    /// 处理 client → agent 的 JSON-RPC 通知（`session/cancel`、`$/cancel_request`）。
    pub async fn handle_notification(
        &self,
        method: &str,
        params: Option<Value>,
    ) -> Result<(), JsonRpcError> {
        let (reply, rx) = oneshot::channel();
        if method == "session/cancel" || method == "$/cancel_request" {
            self.urgent_tx
                .send(UrgentMail::Notification {
                    method: method.to_string(),
                    params,
                    reply,
                })
                .map_err(|_| actor_unavailable())?;
        } else {
            self.mail_tx
                .send(Mail::Notification {
                    method: method.to_string(),
                    params,
                    reply,
                })
                .map_err(|_| actor_unavailable())?;
        }
        rx.await.map_err(|_| actor_unavailable())?
    }

    /// 处理 client → agent 的 JSON-RPC 响应（当前只关联 `session/request_permission`）。
    pub async fn handle_response(
        &self,
        id: JsonRpcId,
        result: Result<Value, JsonRpcError>,
    ) -> Result<(), JsonRpcError> {
        let (reply, rx) = oneshot::channel();
        self.mail_tx
            .send(Mail::Response { id, result, reply })
            .map_err(|_| actor_unavailable())?;
        rx.await.map_err(|_| actor_unavailable())?
    }

    /// 冲刷已订阅的 Core 事件并回译。
    pub async fn drain_and_pump(&self) {
        let (reply, rx) = oneshot::channel();
        if self.mail_tx.send(Mail::DrainAndPump { reply }).is_err() {
            return;
        }
        if let Err(error) = rx.await {
            tracing::debug!(%error, "acp drain_and_pump reply dropped");
        }
    }

    /// 回译给定 canonical 事件（按 run 归属路由；非归属/无 ACP 表示的事件跳过）。
    pub async fn pump_events(&self, events: Vec<AppEventEnvelope>) {
        let (reply, rx) = oneshot::channel();
        if self
            .mail_tx
            .send(Mail::PumpEvents { events, reply })
            .is_err()
        {
            return;
        }
        if let Err(error) = rx.await {
            tracing::debug!(%error, "acp pump_events reply dropped");
        }
    }
}

struct SnapshotSessionResolver {
    snapshot_rx: watch::Receiver<HostSnapshot>,
}

#[async_trait::async_trait]
impl SessionResolver for SnapshotSessionResolver {
    async fn resolve_client_session(&self, event: &AppEventEnvelope) -> Option<ClientSessionId> {
        let run_id = match &event.stream {
            EventStream::Run(run_id) => Some(run_id),
            _ => run_id_of(&event.payload),
        }?;
        self.snapshot_rx.borrow().run_sessions.get(run_id).cloned()
    }
}

struct AcpActor {
    command_host: Arc<dyn AcpCommandHost>,
    registry: Arc<SessionRegistry>,
    factory: AcpClientAdapterFactory,
    connection_id: ConnectionId,
    negotiated: Option<NegotiatedAcpAdapter>,
    session_contexts: BTreeMap<ClientSessionId, (u64, u64)>,
    occupancy: BTreeMap<ClientSessionId, PromptOccupancy>,
    run_sessions: BTreeMap<RunId, ClientSessionId>,
    pending_prompts: HashMap<JsonRpcId, PendingPrompt>,
    pending_permissions: HashMap<JsonRpcId, PendingPermission>,
    outbox: VecDeque<OutboxItem>,
    held_events: VecDeque<AppEventEnvelope>,
    next_request_id: u64,
    event_rx: broadcast::Receiver<AppEventEnvelope>,
    snapshot_tx: watch::Sender<HostSnapshot>,
    mail_rx: mpsc::UnboundedReceiver<Mail>,
    urgent_rx: mpsc::UnboundedReceiver<UrgentMail>,
    deferred_mail: VecDeque<Mail>,
}

impl AcpActor {
    async fn run(mut self) {
        loop {
            while let Ok(urgent) = self.urgent_rx.try_recv() {
                self.handle_urgent(urgent).await;
            }
            if let Some(mail) = self.deferred_mail.pop_front() {
                self.handle_mail(mail).await;
                continue;
            }
            tokio::select! {
                biased;
                urgent = self.urgent_rx.recv() => {
                    let Some(urgent) = urgent else {
                        return;
                    };
                    self.handle_urgent(urgent).await;
                }
                mail = self.mail_rx.recv() => {
                    let Some(mail) = mail else {
                        return;
                    };
                    self.handle_mail(mail).await;
                }
            }
        }
    }

    async fn handle_urgent(&mut self, urgent: UrgentMail) {
        match urgent {
            UrgentMail::Notification {
                method,
                params,
                reply,
            } => {
                let result = self.handle_notification(&method, params).await;
                if let Err(error) = reply.send(result) {
                    tracing::debug!(?error, "acp urgent notification reply dropped");
                }
            }
            UrgentMail::FailClosed { reason, reply } => {
                self.fail_closed_all_prompts(&reason);
                if let Err(error) = reply.send(()) {
                    tracing::debug!(?error, "acp fail-closed reply dropped");
                }
            }
        }
    }

    async fn handle_mail(&mut self, mail: Mail) {
        match mail {
            Mail::Request {
                id,
                method,
                params,
                reply,
            } => {
                let result = self.handle_request(id, &method, params).await;
                if reply.send(result).is_err() {
                    tracing::debug!("acp request reply dropped");
                }
            }
            Mail::Notification {
                method,
                params,
                reply,
            } => {
                let result = self.handle_notification(&method, params).await;
                if let Err(error) = reply.send(result) {
                    tracing::debug!(?error, "acp notification reply dropped");
                }
            }
            Mail::Response { id, result, reply } => {
                let result = self.handle_response(id, result).await;
                if let Err(error) = reply.send(result) {
                    tracing::debug!(?error, "acp response reply dropped");
                }
            }
            Mail::DrainAndPump { reply } => {
                self.drain_and_pump().await;
                if let Err(error) = reply.send(()) {
                    tracing::debug!(?error, "acp drain_and_pump reply dropped");
                }
            }
            Mail::PumpEvents { events, reply } => {
                self.pump_events(events).await;
                if let Err(error) = reply.send(()) {
                    tracing::debug!(?error, "acp pump_events reply dropped");
                }
            }
            Mail::DrainOutbox { reply } => {
                let items = std::mem::take(&mut self.outbox).into_iter().collect();
                if let Err(error) = reply.send(items) {
                    tracing::debug!(?error, "acp drain_outbox reply dropped");
                }
            }
        }
    }

    fn publish_snapshot(&self) {
        let snapshot = HostSnapshot {
            occupancy: self
                .occupancy
                .iter()
                .map(|(session, slot)| (session.clone(), slot.run_id.clone()))
                .collect(),
            run_sessions: self.run_sessions.clone(),
            initialized: self.negotiated.is_some(),
            degraded: self
                .negotiated
                .as_ref()
                .map(|negotiated| negotiated.degraded.clone())
                .unwrap_or_default(),
        };
        if let Err(error) = self.snapshot_tx.send(snapshot) {
            tracing::debug!(?error, "acp host snapshot receiver dropped");
        }
    }

    async fn handle_request(
        &mut self,
        id: JsonRpcId,
        method: &str,
        params: Option<Value>,
    ) -> Result<RequestStart, JsonRpcError> {
        if method == "initialize" {
            return Ok(RequestStart::Ready(self.initialize(params).await?));
        }
        let adapter = self.adapter()?;
        let params = params.unwrap_or(Value::Null);
        let reserved_session = if method == "session/prompt" {
            Some(self.reserve_prompt_occupancy(&id, &params)?)
        } else {
            None
        };
        let frame = self.client_frame(method, &id, &params);
        let request = match adapter.decode(frame).await {
            Ok(request) => request,
            Err(error) => {
                self.release_reservation(reserved_session);
                return Err(jsonrpc_error(&error));
            }
        };
        match &request {
            CanonicalClientRequest::Command(envelope) => match &envelope.command {
                AppCommand::SessionCreate { .. } => {
                    self.release_reservation(reserved_session);
                    Ok(RequestStart::Ready(self.session_new(request).await?))
                }
                AppCommand::RunStart { .. } => self.session_prompt(&id, &params, request).await,
                other => {
                    self.release_reservation(reserved_session);
                    Err(JsonRpcError::new(
                        ERROR_METHOD_NOT_FOUND,
                        format!(
                            "method `{method}` decodes to unsupported canonical command {other:?}"
                        ),
                    ))
                }
            },
            CanonicalClientRequest::Reattach { .. } => {
                self.release_reservation(reserved_session);
                Ok(RequestStart::Ready(self.session_resume(&params, request).await?))
            }
            CanonicalClientRequest::Disconnect { .. } => {
                self.release_reservation(reserved_session);
                Ok(RequestStart::Ready(self.session_close(request).await?))
            }
            other => {
                self.release_reservation(reserved_session);
                Err(JsonRpcError::new(
                    ERROR_METHOD_NOT_FOUND,
                    format!(
                        "method `{method}` has no host handler for canonical request {other:?}"
                    ),
                ))
            }
        }
    }

    async fn handle_notification(
        &mut self,
        method: &str,
        params: Option<Value>,
    ) -> Result<(), JsonRpcError> {
        self.adapter()?;
        let params = params.unwrap_or(Value::Null);
        match method {
            "session/cancel" => {
                let cancel = self
                    .adapter()?
                    .decode_cancel(params)
                    .await
                    .map_err(|error| jsonrpc_error(&error))?;
                self.cancel_session(&cancel.client_session_id).await;
                Ok(())
            }
            "$/cancel_request" => {
                let params = serde_json::from_value::<CancelRequestParams>(params).map_err(
                    |error| {
                        JsonRpcError::new(
                            crate::channels::acp::wire::ERROR_INVALID_PARAMS,
                            error.to_string(),
                        )
                    },
                )?;
                self.cancel_request(&params.request_id).await;
                Ok(())
            }
            other => Err(JsonRpcError::new(
                ERROR_METHOD_NOT_FOUND,
                format!("unknown ACP notification `{other}`"),
            )),
        }
    }

    async fn handle_response(
        &mut self,
        id: JsonRpcId,
        result: Result<Value, JsonRpcError>,
    ) -> Result<(), JsonRpcError> {
        let Some(permission) = self.pending_permissions.remove(&id) else {
            return Ok(());
        };
        let decision = match result {
            Ok(value) => match self.adapter()?.decode_permission_response(value) {
                Ok(crate::channels::acp::adapter::PermissionDecision::Selected { option_id }) => {
                    map::decision_for_option(&option_id).map_err(|error| jsonrpc_error(&error))?
                }
                Ok(crate::channels::acp::adapter::PermissionDecision::Cancelled) => {
                    ApprovalDecision::Cancel
                }
                Err(error) => return Err(jsonrpc_error(&error)),
            },
            Err(error) if error.code == ERROR_REQUEST_CANCELLED => ApprovalDecision::Cancel,
            Err(_) => ApprovalDecision::Deny,
        };
        let envelope = self.adapter()?.command_envelope(
            &format!("permission-{id}"),
            AppCommand::ToolApprove {
                run_id: permission.run_id,
                tool_call_id: permission.tool_call_id,
                decision,
            },
        );
        self.dispatch_attached(&permission.client_session_id, envelope)
            .await?;
        Ok(())
    }

    async fn drain_and_pump(&mut self) {
        let mut events = Vec::new();
        loop {
            match self.event_rx.try_recv() {
                Ok(event) => events.push(event),
                Err(broadcast::error::TryRecvError::Empty) => break,
                Err(broadcast::error::TryRecvError::Closed) => break,
                Err(broadcast::error::TryRecvError::Lagged(_)) => {
                    self.fail_closed_all_prompts("event subscription lagged");
                    return;
                }
            }
        }
        if !events.is_empty() {
            self.pump_events(events).await;
        }
    }

    async fn pump_events(&mut self, events: Vec<AppEventEnvelope>) {
        for envelope in events {
            self.route_or_hold(envelope).await;
        }
    }

    async fn route_or_hold(&mut self, envelope: AppEventEnvelope) {
        if self.resolve_client_session(&envelope).is_some() {
            self.deliver_event(envelope).await;
            return;
        }
        if run_id_of(&envelope.payload).is_some() {
            self.held_events.push_back(envelope);
        }
    }

    async fn flush_held_events(&mut self) {
        let held = std::mem::take(&mut self.held_events);
        for envelope in held {
            self.route_or_hold(envelope).await;
        }
    }

    async fn deliver_event(&mut self, envelope: AppEventEnvelope) {
        let Some(client_session_id) = self.resolve_client_session(&envelope) else {
            return;
        };
        match &envelope.payload {
            AppEvent::RunChanged { run_id, state } => {
                if terminal_state(state) {
                    self.resolve_prompt(run_id, state);
                }
            }
            AppEvent::ToolApprovalRequired { .. } => {
                self.emit_permission_request(client_session_id, &envelope.payload)
                    .await;
            }
            AppEvent::Diagnostic { .. } => {
                // ACP 不新增 session/update 臂：Diagnostic 维持现状丢弃。
            }
            _ => self.emit_update(client_session_id, &envelope).await,
        }
    }

    fn resolve_client_session(&self, event: &AppEventEnvelope) -> Option<ClientSessionId> {
        let run_id = match &event.stream {
            EventStream::Run(run_id) => Some(run_id),
            _ => run_id_of(&event.payload),
        }?;
        self.run_sessions.get(run_id).cloned()
    }

    async fn initialize(&mut self, params: Option<Value>) -> Result<Value, JsonRpcError> {
        if self.negotiated.is_some() {
            return Err(JsonRpcError::new(
                ERROR_INVALID_REQUEST,
                "initialize was already completed; this host accepts one handshake per connection",
            ));
        }
        let params = serde_json::from_value::<InitializeParams>(params.unwrap_or(Value::Null))
            .map_err(|error| {
                JsonRpcError::new(
                    crate::channels::acp::wire::ERROR_INVALID_PARAMS,
                    error.to_string(),
                )
            })?;
        params.reject_unknown("initialize").map_err(|message| {
            JsonRpcError::new(crate::channels::acp::wire::ERROR_INVALID_PARAMS, message)
        })?;
        if params.protocol_version != PROTOCOL_VERSION {
            return Err(JsonRpcError::new(
                crate::channels::acp::wire::ERROR_INVALID_PARAMS,
                format!(
                    "unsupported protocolVersion {}: this host implements stable wire protocolVersion {PROTOCOL_VERSION} (experimental v2 is not mixed in)",
                    params.protocol_version
                ),
            ));
        }
        let client_version = params
            .client_info
            .as_ref()
            .map(|info| info.version.clone())
            .unwrap_or_else(|| "unknown".into());
        let snapshot = CapabilitySnapshot {
            schema_version: CLIENT_ADAPTER_SCHEMA_VERSION,
            protocol: ClientProtocol::new(crate::channels::acp::adapter::ACP_PROTOCOL),
            protocol_version: PROTOCOL_VERSION.to_string(),
            client_version,
            revision: 1,
            capabilities: declared_client_capabilities(&params.client_capabilities),
        };
        let negotiated = self
            .factory
            .create_concrete(snapshot)
            .map_err(|error| jsonrpc_error(&error))?;
        self.negotiated = Some(negotiated);
        self.publish_snapshot();
        serialize_value(
            InitializeResult {
                protocol_version: PROTOCOL_VERSION,
                agent_capabilities: crate::channels::acp::wire::AgentCapabilities {
                    session_capabilities: crate::channels::acp::wire::SessionCapabilities {
                        resume: Some(crate::channels::acp::wire::EmptyCapability {}),
                        close: Some(crate::channels::acp::wire::EmptyCapability {}),
                        ..crate::channels::acp::wire::SessionCapabilities::default()
                    },
                    ..crate::channels::acp::wire::AgentCapabilities::default()
                },
                agent_info: Some(Implementation {
                    name: ACP_AGENT_NAME.into(),
                    title: Some("Pawork ACP Host".into()),
                    version: ACP_AGENT_VERSION.into(),
                }),
                auth_methods: Vec::new(),
            },
            "InitializeResult",
        )
    }

    async fn session_new(&mut self, request: CanonicalClientRequest) -> Result<Value, JsonRpcError> {
        let adapter = self.adapter()?;
        let placeholder = AdapterSessionContext {
            adapter: Arc::clone(&adapter) as Arc<dyn ClientAdapter>,
            client_session_id: ClientSessionId::new("acp-pending-session"),
            connection_id: self.connection_id.clone(),
            ownership_epoch: 0,
            revision: 0,
        };
        let response = self
            .dispatch_canonical(placeholder, request)
            .await
            .map_err(|error| jsonrpc_error(&error))?;
        let session = canonical_response_value(response, "session/new")?;
        let session_id = session
            .get("session_id")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                JsonRpcError::new(
                    crate::channels::acp::wire::ERROR_INTERNAL,
                    "SessionCreate response did not carry session_id",
                )
            })?
            .to_string();
        let client_session_id = ClientSessionId::new(session_id.clone());
        let record = ClientSessionRecord {
            schema_version: CLIENT_ADAPTER_SCHEMA_VERSION,
            protocol: adapter.protocol().clone(),
            client_session_id: client_session_id.clone(),
            core_session_id: SessionId::from(session_id.clone()),
            connection_id: self.connection_id.clone(),
            ownership_epoch: 1,
            revision: 1,
            state: ClientSessionState::Subscribed,
            capabilities: adapter.capabilities().clone(),
            updated_at: now_timestamp(),
        };
        let attach_context = AdapterSessionContext {
            adapter: Arc::clone(&adapter) as Arc<dyn ClientAdapter>,
            client_session_id: client_session_id.clone(),
            connection_id: self.connection_id.clone(),
            ownership_epoch: 1,
            revision: 1,
        };
        let response = self
            .dispatch_canonical(attach_context, CanonicalClientRequest::Attach(record))
            .await
            .map_err(|error| jsonrpc_error(&error))?;
        let CanonicalCoreFrame::SessionState(record) = response else {
            return Err(JsonRpcError::new(
                crate::channels::acp::wire::ERROR_INTERNAL,
                "session attach did not produce a session state",
            ));
        };
        self.session_contexts.insert(
            client_session_id,
            (record.ownership_epoch, record.revision),
        );
        tracing::debug!(session_id, "acp session/new attached");
        serialize_value(SessionNewResult { session_id }, "SessionNewResult")
    }

    async fn session_prompt(
        &mut self,
        id: &JsonRpcId,
        params: &Value,
        request: CanonicalClientRequest,
    ) -> Result<RequestStart, JsonRpcError> {
        let session_id = params
            .get("sessionId")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                JsonRpcError::new(
                    crate::channels::acp::wire::ERROR_INVALID_PARAMS,
                    "session/prompt params must carry sessionId",
                )
            })?;
        let client_session_id = ClientSessionId::new(session_id);
        if !self.occupancy.contains_key(&client_session_id) {
            return Err(JsonRpcError::new(
                crate::channels::acp::wire::ERROR_INTERNAL,
                "prompt turn failed in Core",
            ));
        }
        self.publish_snapshot();
        let (completion_tx, completion_rx) = tokio::sync::mpsc::channel(1);
        let context = match self.session_context(&client_session_id).await {
            Ok(context) => context,
            Err(error) => {
                self.release_occupancy(&client_session_id, None);
                return Err(error);
            }
        };
        let response = self
            .dispatch_canonical(context, request)
            .await
            .map_err(|error| {
                self.release_occupancy(&client_session_id, None);
                jsonrpc_error(&error)
            })?;
        if !self.occupancy.contains_key(&client_session_id) {
            if let Ok(run_id) = accepted_run_id(response, "session/prompt") {
                if let Ok(adapter) = self.adapter() {
                    if let Err(error) = self
                        .dispatch_attached(
                            &client_session_id,
                            adapter.command_envelope(
                                &format!("fail-closed-{run_id}"),
                                AppCommand::RunCancel { run_id },
                            ),
                        )
                        .await
                    {
                        tracing::warn!(error = ?error, "acp fail-closed RunCancel dispatch failed");
                    }
                }
            }
            return Err(JsonRpcError::new(
                crate::channels::acp::wire::ERROR_INTERNAL,
                "prompt turn failed in Core",
            ));
        }
        let run_id = match accepted_run_id(response, "session/prompt") {
            Ok(run_id) => run_id,
            Err(error) => {
                self.release_occupancy(&client_session_id, None);
                return Err(error);
            }
        };
        if let Some(slot) = self.occupancy.get_mut(&client_session_id) {
            slot.run_id = Some(run_id.clone());
        }
        self.run_sessions
            .insert(run_id.clone(), client_session_id.clone());
        self.pending_prompts.insert(
            id.clone(),
            PendingPrompt {
                client_session_id: client_session_id.clone(),
                run_id,
                completion: completion_tx,
            },
        );
        self.publish_snapshot();
        self.flush_held_events().await;
        let (replay_session_cancel, replay_request_cancel) = self
            .occupancy
            .get_mut(&client_session_id)
            .map(|slot| {
                let session = slot.early_session_cancel;
                let request = slot.early_request_cancel;
                slot.early_session_cancel = false;
                slot.early_request_cancel = false;
                (session, request)
            })
            .unwrap_or((false, false));
        self.publish_snapshot();
        if replay_session_cancel {
            self.cancel_session(&client_session_id).await;
        } else if replay_request_cancel {
            self.cancel_request(id).await;
        }
        Ok(RequestStart::Prompt(completion_rx))
    }

    async fn session_resume(
        &mut self,
        _params: &Value,
        request: CanonicalClientRequest,
    ) -> Result<Value, JsonRpcError> {
        let CanonicalClientRequest::Reattach {
            client_session_id,
            ownership_epoch: _,
            revision: _,
            connection_id: _,
            state,
            updated_at,
        } = request
        else {
            return Err(internal_state(
                "session/resume did not decode to Reattach",
                json!({ "method": "session/resume" }),
            ));
        };
        let record = self.registry.get(&client_session_id).await.ok_or_else(|| {
            JsonRpcError::new(
                crate::channels::acp::wire::ERROR_RESOURCE_NOT_FOUND,
                format!("unknown client session `{}`", client_session_id.0),
            )
        })?;
        if !self.core_session_exists(&record.core_session_id).await {
            return Err(JsonRpcError::new(
                crate::channels::acp::wire::ERROR_RESOURCE_NOT_FOUND,
                format!(
                    "core session `{}` is not available in this host",
                    record.core_session_id
                ),
            ));
        }
        let (claim_epoch, claim_revision) = (record.ownership_epoch, record.revision);
        let context = self.session_context(&client_session_id).await?;
        let response = self
            .dispatch_canonical(
                context,
                CanonicalClientRequest::Reattach {
                    client_session_id: client_session_id.clone(),
                    ownership_epoch: claim_epoch,
                    revision: claim_revision,
                    connection_id: self.connection_id.clone(),
                    state,
                    updated_at,
                },
            )
            .await
            .map_err(|error| jsonrpc_error(&error))?;
        let CanonicalCoreFrame::SessionState(record) = response else {
            return Err(internal_state(
                "session/resume did not produce a session state",
                json!({ "method": "session/resume" }),
            ));
        };
        self.session_contexts
            .insert(client_session_id, (record.ownership_epoch, record.revision));
        Ok(json!({}))
    }

    async fn session_close(&mut self, request: CanonicalClientRequest) -> Result<Value, JsonRpcError> {
        let CanonicalClientRequest::Disconnect {
            client_session_id,
            ownership_epoch,
            revision,
            updated_at,
        } = request
        else {
            return Err(internal_state(
                "session/close did not decode to Disconnect",
                json!({ "method": "session/close" }),
            ));
        };
        self.cancel_session(&client_session_id).await;
        let context = self.session_context(&client_session_id).await?;
        let response = self
            .dispatch_canonical(
                context,
                CanonicalClientRequest::Disconnect {
                    client_session_id: client_session_id.clone(),
                    ownership_epoch,
                    revision,
                    updated_at,
                },
            )
            .await
            .map_err(|error| jsonrpc_error(&error))?;
        let CanonicalCoreFrame::SessionState(record) = response else {
            return Err(internal_state(
                "session/close did not produce a session state",
                json!({ "method": "session/close" }),
            ));
        };
        self.session_contexts
            .insert(client_session_id, (record.ownership_epoch, record.revision));
        Ok(json!({}))
    }

    async fn cancel_session(&mut self, client_session_id: &ClientSessionId) {
        let run_id: RunId = {
            match self.occupancy.get_mut(client_session_id) {
                Some(slot) => {
                    if let Some(run_id) = slot.run_id.clone() {
                        slot.early_session_cancel = false;
                        run_id
                    } else {
                        slot.early_session_cancel = true;
                        return;
                    }
                }
                None => return,
            }
        };
        if let Ok(adapter) = self.adapter() {
            if let Err(error) = self
                .dispatch_attached(
                    client_session_id,
                    adapter.command_envelope(
                        &format!("session-cancel-{run_id}"),
                        AppCommand::RunCancel {
                            run_id: run_id.clone(),
                        },
                    ),
                )
                .await
            {
                tracing::warn!(error = ?error, "acp session-cancel RunCancel dispatch failed");
            }
        }
        let cascaded: Vec<JsonRpcId> = self
            .pending_permissions
            .iter()
            .filter(|(_, pending)| &pending.client_session_id == client_session_id)
            .map(|(id, _)| id.clone())
            .collect();
        for id in cascaded {
            self.pending_permissions.remove(&id);
            self.push_frame(
                JsonRpcNotification {
                    jsonrpc: "2.0".into(),
                    method: "$/cancel_request".into(),
                    params: Some(json!({ "requestId": id })),
                }
                .to_value(),
            );
        }
    }

    async fn cancel_request(&mut self, request_id: &JsonRpcId) {
        let prompt = self
            .pending_prompts
            .get(request_id)
            .map(|prompt| (prompt.client_session_id.clone(), prompt.run_id.clone()));
        if let Some((client_session_id, run_id)) = prompt {
            if let Ok(adapter) = self.adapter() {
                if let Err(error) = self
                    .dispatch_attached(
                        &client_session_id,
                        adapter.command_envelope(
                            &format!("cancel-request-{run_id}"),
                            AppCommand::RunCancel { run_id },
                        ),
                    )
                    .await
                {
                    tracing::warn!(error = ?error, "acp cancel-request RunCancel dispatch failed");
                }
            }
            return;
        }
        if let Some(permission) = self.pending_permissions.remove(request_id) {
            if let Ok(adapter) = self.adapter() {
                if let Err(error) = self
                    .dispatch_attached(
                        &permission.client_session_id,
                        adapter.command_envelope(
                            &format!("cancel-permission-{request_id}"),
                            AppCommand::ToolApprove {
                                run_id: permission.run_id,
                                tool_call_id: permission.tool_call_id,
                                decision: ApprovalDecision::Cancel,
                            },
                        ),
                    )
                    .await
                {
                    tracing::warn!(error = ?error, "acp cancel-permission dispatch failed");
                }
            }
            return;
        }
        if let Some(slot) = self
            .occupancy
            .values_mut()
            .find(|slot| &slot.request_id == request_id)
        {
            slot.early_request_cancel = true;
        }
    }

    fn resolve_prompt(&mut self, run_id: &RunId, state: &RunState) {
        let resolution = match state {
            RunState::Completed => Some(PromptResolution::Stopped(StopReason::EndTurn)),
            RunState::Cancelled | RunState::Interrupted => {
                Some(PromptResolution::Stopped(StopReason::Cancelled))
            }
            RunState::Failed => Some(PromptResolution::Failed),
            _ => None,
        };
        let Some(resolution) = resolution else {
            return;
        };
        let id = self
            .pending_prompts
            .iter()
            .find(|(_, prompt)| &prompt.run_id == run_id)
            .map(|(id, _)| id.clone());
        let Some(id) = id else {
            return;
        };
        let Some(prompt) = self.pending_prompts.remove(&id) else {
            report_acp_state(
                "pending prompt disappeared after lookup",
                json!({ "run_id": run_id.as_str() }),
            );
            return;
        };
        self.release_occupancy(&prompt.client_session_id, Some(run_id));
        self.push_outbox(OutboxItem::FlushBarrier {
            completion: prompt.completion,
            resolution,
        });
    }

    async fn emit_permission_request(
        &mut self,
        client_session_id: ClientSessionId,
        event: &AppEvent,
    ) {
        let result = async {
            let adapter = self.adapter()?;
            let params = adapter
                .permission_request(event, &client_session_id)
                .await
                .map_err(|error| jsonrpc_error(&error))?;
            let run_id = run_id_of(event)
                .ok_or_else(|| {
                    internal_state(
                        "ToolApprovalRequired event without run_id",
                        json!({ "session": client_session_id.0 }),
                    )
                })?
                .clone();
            let tool_call_id = tool_call_id_of(event)
                .ok_or_else(|| {
                    internal_state(
                        "ToolApprovalRequired event without tool_call_id",
                        json!({ "session": client_session_id.0 }),
                    )
                })?
                .clone();
            let id = Value::Number(self.next_request_id.into());
            self.next_request_id = self.next_request_id.saturating_add(1);
            self.pending_permissions.insert(
                id.clone(),
                PendingPermission {
                    run_id,
                    tool_call_id,
                    client_session_id: client_session_id.clone(),
                },
            );
            let params = serialize_value(params, "RequestPermissionParams")?;
            self.push_frame(
                JsonRpcRequest {
                    jsonrpc: "2.0".into(),
                    id,
                    method: "session/request_permission".into(),
                    params: Some(params),
                }
                .to_value(),
            );
            Ok::<(), JsonRpcError>(())
        }
        .await;
        if let Err(error) = result {
            tracing::warn!(
                session_id = %client_session_id.0,
                code = error.code,
                message = %error.message,
                "acp permission request emission failed"
            );
        }
    }

    async fn emit_update(&mut self, client_session_id: ClientSessionId, envelope: &AppEventEnvelope) {
        let result = async {
            let adapter = self.adapter()?;
            let frame = adapter
                .encode(CanonicalCoreFrame::Event(envelope.clone()))
                .await
                .map_err(|error| jsonrpc_error(&error))?;
            if frame.method != "acp.notification" {
                return Ok::<(), JsonRpcError>(());
            }
            self.push_frame(
                JsonRpcNotification {
                    jsonrpc: "2.0".into(),
                    method: "session/update".into(),
                    params: Some(frame.payload),
                }
                .to_value(),
            );
            Ok(())
        }
        .await;
        if let Err(error) = result {
            tracing::warn!(
                session_id = %client_session_id.0,
                code = error.code,
                message = %error.message,
                "acp session/update emission failed"
            );
        }
    }

    fn fail_closed_all_prompts(&mut self, reason: &str) {
        tracing::warn!(
            reason,
            "acp host fail-closed: releasing all in-flight prompts"
        );
        report_acp_state(
            "acp host fail-closed",
            json!({ "reason": reason }),
        );
        self.occupancy.clear();
        let prompts = std::mem::take(&mut self.pending_prompts);
        self.pending_permissions.clear();
        self.run_sessions.clear();
        self.held_events.clear();
        for prompt in prompts.into_values() {
            if let Err(error) = prompt.completion.try_send(PromptResolution::Failed) {
                tracing::debug!(?error, "acp fail-closed prompt completion dropped");
            }
        }
        let items = std::mem::take(&mut self.outbox);
        for item in items {
            if let OutboxItem::FlushBarrier {
                completion,
                resolution,
            } = item
            {
                if let Err(error) = completion.try_send(resolution) {
                    tracing::debug!(?error, "acp fail-closed prompt completion dropped");
                }
            }
        }
        self.publish_snapshot();
    }

    fn push_frame(&mut self, frame: Value) {
        self.push_outbox(OutboxItem::Frame(frame));
    }

    fn push_outbox(&mut self, item: OutboxItem) {
        self.outbox.push_back(item);
    }

    fn adapter(&self) -> Result<Arc<AcpClientAdapter>, JsonRpcError> {
        self.negotiated
            .as_ref()
            .map(|negotiated| Arc::clone(&negotiated.adapter))
            .ok_or_else(|| {
                JsonRpcError::new(
                    ERROR_INVALID_REQUEST,
                    "host is not initialized: call initialize first",
                )
            })
    }

    fn release_occupancy(&mut self, client_session_id: &ClientSessionId, run_id: Option<&RunId>) {
        self.occupancy.remove(client_session_id);
        if let Some(run_id) = run_id {
            self.run_sessions.remove(run_id);
        }
        self.publish_snapshot();
    }

    fn reserve_prompt_occupancy(
        &mut self,
        id: &JsonRpcId,
        params: &Value,
    ) -> Result<ClientSessionId, JsonRpcError> {
        let session_id = params
            .get("sessionId")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                JsonRpcError::new(
                    crate::channels::acp::wire::ERROR_INVALID_PARAMS,
                    "session/prompt params must carry sessionId",
                )
            })?;
        let client_session_id = ClientSessionId::new(session_id);
        if self.occupancy.contains_key(&client_session_id) {
            return Err(JsonRpcError::new(
                ERROR_INVALID_REQUEST,
                format!(
                    "session `{session_id}` already has an active prompt turn;                      this host supports one prompt per session at a time"
                ),
            ));
        }
        self.occupancy.insert(
            client_session_id.clone(),
            PromptOccupancy {
                request_id: id.clone(),
                run_id: None,
                early_session_cancel: false,
                early_request_cancel: false,
            },
        );
        self.publish_snapshot();
        Ok(client_session_id)
    }

    fn release_reservation(&mut self, reserved: Option<ClientSessionId>) {
        if let Some(client_session_id) = reserved {
            self.release_occupancy(&client_session_id, None);
        }
    }

    async fn session_context(
        &mut self,
        client_session_id: &ClientSessionId,
    ) -> Result<AdapterSessionContext, JsonRpcError> {
        let (ownership_epoch, revision) = match self.session_contexts.get(client_session_id).copied()
        {
            Some(epoch_revision) => epoch_revision,
            None => {
                let record = self.registry.get(client_session_id).await.ok_or_else(|| {
                    JsonRpcError::new(
                        crate::channels::acp::wire::ERROR_RESOURCE_NOT_FOUND,
                        format!("unknown client session `{}`", client_session_id.0),
                    )
                })?;
                self.session_contexts.insert(
                    client_session_id.clone(),
                    (record.ownership_epoch, record.revision),
                );
                (record.ownership_epoch, record.revision)
            }
        };
        Ok(AdapterSessionContext {
            adapter: Arc::clone(&self.adapter()?) as Arc<dyn ClientAdapter>,
            client_session_id: client_session_id.clone(),
            connection_id: self.connection_id.clone(),
            ownership_epoch,
            revision,
        })
    }

    async fn dispatch_attached(
        &mut self,
        client_session_id: &ClientSessionId,
        envelope: AppCommandEnvelope,
    ) -> Result<(), JsonRpcError> {
        let context = self.session_context(client_session_id).await?;
        self.require_attached(&context)
            .await
            .map_err(|error| jsonrpc_error(&error))?;
        let response = self
            .command_host
            .dispatch(envelope)
            .await
            .map_err(|error| jsonrpc_error(&host_unavailable(error)))?;
        canonical_response_value(CanonicalCoreFrame::Response(response), "command")?;
        Ok(())
    }

    async fn interruptible_core_call<T: Send + 'static>(
        &mut self,
        work: impl std::future::Future<Output = Result<T, AcpHostError>> + Send + 'static,
    ) -> Result<T, AcpHostError> {
        let mut work = tokio::spawn(work);
        loop {
            tokio::select! {
                biased;
                urgent = self.urgent_rx.recv() => {
                    let Some(urgent) = urgent else {
                        work.abort();
                        return Err(AcpHostError::Unavailable(
                            "ACP host actor is unavailable".into(),
                        ));
                    };
                    self.handle_urgent(urgent).await;
                }
                mail = self.mail_rx.recv() => {
                    let Some(mail) = mail else {
                        work.abort();
                        return Err(AcpHostError::Unavailable(
                            "ACP host actor is unavailable".into(),
                        ));
                    };
                    match mail {
                        Mail::DrainOutbox { reply } => {
                            let items = std::mem::take(&mut self.outbox).into_iter().collect();
                            if let Err(error) = reply.send(items) {
                                tracing::debug!(?error, "acp interruptible drain_outbox reply dropped");
                            }
                        }
                        other => self.deferred_mail.push_back(other),
                    }
                }
                result = &mut work => {
                    return match result {
                        Ok(result) => result,
                        Err(error) if error.is_cancelled() => Err(AcpHostError::Unavailable(
                            "ACP core dispatch cancelled".into(),
                        )),
                        Err(error) => Err(AcpHostError::Unavailable(error.to_string())),
                    };
                }
            }
        }
    }

    async fn dispatch_canonical(
        &mut self,
        context: AdapterSessionContext,
        request: CanonicalClientRequest,
    ) -> Result<CanonicalCoreFrame, AdapterError> {
        match request {
            CanonicalClientRequest::Command(envelope) => {
                if !matches!(envelope.command, AppCommand::SessionCreate { .. }) {
                    let record = self.require_attached(&context).await?;
                    if let AppCommand::SessionClientContextReplace { session_id, .. } =
                        &envelope.command
                    {
                        if record.core_session_id != *session_id {
                            return Err(AdapterError::InvalidFrame(format!(
                                "session_client_context_replace targets a core session not bound to this client session (bound to {})",
                                record.core_session_id
                            )));
                        }
                    }
                }
                let host = Arc::clone(&self.command_host);
                let response = self
                    .interruptible_core_call(async move { host.dispatch(envelope).await })
                    .await
                    .map_err(host_unavailable)?;
                Ok(CanonicalCoreFrame::Response(response))
            }
            CanonicalClientRequest::Query(envelope) => {
                self.require_attached(&context).await?;
                let host = Arc::clone(&self.command_host);
                let response = self
                    .interruptible_core_call(async move { host.query(envelope).await })
                    .await
                    .map_err(host_unavailable)?;
                Ok(CanonicalCoreFrame::Response(response))
            }
            CanonicalClientRequest::Attach(record) => self.attach(context, record).await,
            CanonicalClientRequest::Reattach {
                client_session_id,
                ownership_epoch,
                revision,
                connection_id,
                state,
                updated_at,
            } => {
                self.reattach(
                    context,
                    client_session_id,
                    ownership_epoch,
                    revision,
                    connection_id,
                    state,
                    updated_at,
                )
                .await
            }
            CanonicalClientRequest::Disconnect {
                client_session_id,
                ownership_epoch,
                revision,
                updated_at,
            } => {
                self.disconnect(
                    context,
                    client_session_id,
                    ownership_epoch,
                    revision,
                    updated_at,
                )
                .await
            }
        }
    }

    async fn require_attached(
        &self,
        context: &AdapterSessionContext,
    ) -> Result<ClientSessionRecord, AdapterError> {
        let record = self
            .registry
            .get(&context.client_session_id)
            .await
            .ok_or_else(|| AdapterError::UnknownSession(context.client_session_id.clone()))?;
        if record.state == ClientSessionState::Disconnected {
            return Err(AdapterError::SessionNotAttached(
                context.client_session_id.clone(),
            ));
        }
        ensure_binding(context, &record)?;
        ensure_owner(context, &record)?;
        Ok(record)
    }

    async fn attach(
        &self,
        context: AdapterSessionContext,
        record: ClientSessionRecord,
    ) -> Result<CanonicalCoreFrame, AdapterError> {
        if record.client_session_id != context.client_session_id {
            return Err(AdapterError::InvalidFrame(format!(
                "attach client_session_id {:?} does not match negotiated context {:?}",
                record.client_session_id, context.client_session_id
            )));
        }
        if record.connection_id != context.connection_id {
            return Err(AdapterError::InvalidFrame(format!(
                "attach connection_id {:?} does not match negotiated context {:?}",
                record.connection_id, context.connection_id
            )));
        }
        if record.protocol != *context.adapter.protocol() {
            return Err(AdapterError::InvalidFrame(format!(
                "attach protocol {:?} does not match negotiated adapter protocol {:?}",
                record.protocol,
                context.adapter.protocol()
            )));
        }
        if record.capabilities != *context.adapter.capabilities() {
            return Err(AdapterError::InvalidFrame(
                "attach capability snapshot does not match negotiated adapter".into(),
            ));
        }
        if record.ownership_epoch != context.ownership_epoch || record.revision != context.revision {
            return Err(AdapterError::InvalidFrame(format!(
                "attach ownership {}/{} does not match negotiated context {}/{}",
                record.ownership_epoch, record.revision, context.ownership_epoch, context.revision
            )));
        }
        if !self.core_session_exists(&record.core_session_id).await {
            return Err(AdapterError::CoreSessionNotFound(
                record.core_session_id.clone(),
            ));
        }
        self.registry.register(record.clone()).await?;
        Ok(CanonicalCoreFrame::SessionState(record))
    }

    async fn reattach(
        &self,
        context: AdapterSessionContext,
        client_session_id: ClientSessionId,
        ownership_epoch: u64,
        revision: u64,
        connection_id: ConnectionId,
        state: ClientSessionState,
        updated_at: pawork_domain::Timestamp,
    ) -> Result<CanonicalCoreFrame, AdapterError> {
        ensure_request_handle(&context, &client_session_id, ownership_epoch, revision)?;
        let record = self
            .registry
            .get(&client_session_id)
            .await
            .ok_or_else(|| AdapterError::UnknownSession(client_session_id.clone()))?;
        ensure_binding(&context, &record)?;
        if !self.core_session_exists(&record.core_session_id).await {
            return Err(AdapterError::CoreSessionNotFound(record.core_session_id));
        }
        let record = self
            .registry
            .claim(
                &client_session_id,
                ownership_epoch,
                revision,
                connection_id,
                state,
                updated_at,
            )
            .await?;
        Ok(CanonicalCoreFrame::SessionState(record))
    }

    async fn disconnect(
        &self,
        context: AdapterSessionContext,
        client_session_id: ClientSessionId,
        ownership_epoch: u64,
        revision: u64,
        updated_at: pawork_domain::Timestamp,
    ) -> Result<CanonicalCoreFrame, AdapterError> {
        ensure_request_handle(&context, &client_session_id, ownership_epoch, revision)?;
        let record = self
            .registry
            .get(&client_session_id)
            .await
            .ok_or_else(|| AdapterError::UnknownSession(client_session_id.clone()))?;
        ensure_binding(&context, &record)?;
        let record = self
            .registry
            .transition(
                &client_session_id,
                ownership_epoch,
                revision,
                ClientSessionState::Disconnected,
                updated_at,
            )
            .await?;
        Ok(CanonicalCoreFrame::SessionState(record))
    }

    async fn core_session_exists(&self, session_id: &SessionId) -> bool {
        let envelope = AppQueryEnvelope {
            api_version: API_VERSION,
            request_id: QueryId::from(format!("acp-session-exists-{}", session_id.as_str())),
            source: CommandSource::Automation,
            identity: ActorIdentity::Automation {
                name: format!("acp:{ACP_AGENT_NAME}"),
            },
            issued_at: now_timestamp(),
            query: AppQuery::SessionGet {
                session_id: session_id.clone(),
                timeline_after_sequence: None,
                timeline_limit: None,
            },
        };
        match self.command_host.query(envelope).await {
            Ok(response) => matches!(response.response, AppResponse::Data(_)),
            Err(_) => false,
        }
    }

    fn client_frame(&self, method: &str, id: &JsonRpcId, params: &Value) -> AdapterWireFrame {
        AdapterWireFrame {
            schema_version: CLIENT_ADAPTER_SCHEMA_VERSION,
            request_id: match id {
                Value::Null => "acp-notification".into(),
                other => other.to_string(),
            },
            method: method.into(),
            payload: params.clone(),
            extensions: Default::default(),
        }
    }
}

fn host_unavailable(error: AcpHostError) -> AdapterError {
    AdapterError::HostUnavailable(error.to_string())
}

fn canonical_response_value(
    frame: CanonicalCoreFrame,
    context: &str,
) -> Result<Value, JsonRpcError> {
    match frame {
        CanonicalCoreFrame::Response(envelope) => map::response_to_result(&envelope),
        CanonicalCoreFrame::Error(error) => Err(JsonRpcError::new(
            map::jsonrpc_code_for_frame(&error),
            error.message,
        )),
        other => Err(internal_state(
            &format!("{context} produced unexpected canonical frame"),
            json!({ "frame": format!("{other:?}") }),
        )),
    }
}

fn accepted_run_id(frame: CanonicalCoreFrame, context: &str) -> Result<RunId, JsonRpcError> {
    match frame {
        CanonicalCoreFrame::Response(envelope) => match envelope.response {
            AppResponse::Accepted {
                run_id: Some(run_id),
                ..
            } => Ok(run_id),
            AppResponse::Accepted { .. } => Err(internal_state(
                &format!("{context} was accepted but did not report a run id"),
                json!({ "context": context }),
            )),
            _ => Err(internal_state(
                &format!("{context} produced an unexpected response"),
                json!({ "context": context }),
            )),
        },
        CanonicalCoreFrame::Error(error) => Err(JsonRpcError::new(
            map::jsonrpc_code_for_frame(&error),
            error.message,
        )),
        other => Err(internal_state(
            &format!("{context} produced unexpected canonical frame"),
            json!({ "frame": format!("{other:?}") }),
        )),
    }
}

fn run_id_of(event: &AppEvent) -> Option<&RunId> {
    match event {
        AppEvent::RunChanged { run_id, .. }
        | AppEvent::AssistantDelta { run_id, .. }
        | AppEvent::ThinkingDelta { run_id, .. }
        | AppEvent::ToolStarted { run_id, .. }
        | AppEvent::ToolOutput { run_id, .. }
        | AppEvent::ToolApprovalRequired { run_id, .. }
        | AppEvent::ToolCompleted { run_id, .. } => Some(run_id),
        _ => None,
    }
}

fn tool_call_id_of(event: &AppEvent) -> Option<&ToolCallId> {
    match event {
        AppEvent::ToolApprovalRequired { tool_call_id, .. } => Some(tool_call_id),
        _ => None,
    }
}

fn terminal_state(state: &RunState) -> bool {
    matches!(
        state,
        RunState::Completed | RunState::Cancelled | RunState::Failed | RunState::Interrupted
    )
}

fn jsonrpc_error(error: &AdapterError) -> JsonRpcError {
    JsonRpcError::new(map::jsonrpc_code_for(error), error.to_string())
}

fn declared_client_capabilities(
    capabilities: &Option<ClientCapabilities>,
) -> BTreeSet<ClientCapability> {
    let mut declared = BTreeSet::new();
    let Some(capabilities) = capabilities else {
        return declared;
    };
    if let Some(fs) = &capabilities.fs {
        if fs.read_text_file {
            declared.insert(ClientCapability::new("fs.read_text_file"));
        }
        if fs.write_text_file {
            declared.insert(ClientCapability::new("fs.write_text_file"));
        }
    }
    if capabilities.terminal == Some(true) {
        declared.insert(ClientCapability::new("terminal"));
    }
    if capabilities.elicitation.is_some() {
        declared.insert(ClientCapability::new("elicitation"));
    }
    if capabilities.session.is_some() {
        declared.insert(ClientCapability::new("session.config_options"));
    }
    for extra in capabilities.extra.keys() {
        declared.insert(ClientCapability::new(extra.clone()));
    }
    declared
}

fn ensure_binding(
    context: &AdapterSessionContext,
    record: &ClientSessionRecord,
) -> Result<(), AdapterError> {
    if record.protocol != *context.adapter.protocol()
        || record.capabilities != *context.adapter.capabilities()
    {
        return Err(AdapterError::InvalidFrame(format!(
            "session binding mismatch: negotiated adapter protocol {:?} / capability snapshot              vs authoritative record protocol {:?}",
            context.adapter.protocol(),
            record.protocol
        )));
    }
    Ok(())
}

fn ensure_owner(
    context: &AdapterSessionContext,
    record: &ClientSessionRecord,
) -> Result<(), AdapterError> {
    if record.ownership_epoch == context.ownership_epoch && record.revision == context.revision {
        Ok(())
    } else {
        Err(AdapterError::StaleOwner {
            client_session_id: context.client_session_id.clone(),
            expected_epoch: record.ownership_epoch,
            expected_revision: record.revision,
            actual_epoch: context.ownership_epoch,
            actual_revision: context.revision,
        })
    }
}

fn ensure_request_handle(
    context: &AdapterSessionContext,
    client_session_id: &ClientSessionId,
    ownership_epoch: u64,
    revision: u64,
) -> Result<(), AdapterError> {
    if context.client_session_id != *client_session_id
        || context.ownership_epoch != ownership_epoch
        || context.revision != revision
    {
        return Err(AdapterError::InvalidFrame(
            "request session handle does not match negotiated context".into(),
        ));
    }
    Ok(())
}

fn serialize_value<T: serde::Serialize>(value: T, what: &str) -> Result<Value, JsonRpcError> {
    serde_json::to_value(value).map_err(|error| {
        internal_state(
            &format!("failed to serialize {what}"),
            json!({ "error": error.to_string(), "what": what }),
        )
    })
}


fn wait_std<T>(rx: std::sync::mpsc::Receiver<T>) -> Option<T> {
    rx.recv().ok()
}
fn actor_unavailable() -> JsonRpcError {
    internal_state(
        "ACP host actor is unavailable",
        json!({ "reason": "mailbox closed" }),
    )
}

fn internal_state(message: &str, details: Value) -> JsonRpcError {
    report_acp_state(message, details);
    JsonRpcError::new(crate::channels::acp::wire::ERROR_INTERNAL, message)
}

fn report_acp_state(message: &str, details: Value) {
    let event = DegradeEvent::new(
        DegradeKind::AcpState,
        DegradeSeverity::Error,
        message,
        details,
    );
    tracing::error!(
        code = %event.code(),
        message = %event.message,
        details = ?event.details,
        "acp host degrade"
    );
}
