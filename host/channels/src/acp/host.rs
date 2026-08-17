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

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use pawork_domain::{ConnectionId, QueryId, RunId, SessionId, ToolCallId, WorkspaceId};
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
use tokio::sync::broadcast;

use crate::acp::adapter::{AcpClientAdapter, AcpClientAdapterFactory, CwdResolver, SessionResolver};
use crate::acp::command_host::{AcpCommandHost, AcpHostError};
use crate::acp::map;
use crate::acp::now_timestamp;
use crate::acp::wire::{
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

/// run → client session 路由（与宿主共享同一 pending 表）。
struct HostSessionResolver {
    run_sessions: Arc<Mutex<BTreeMap<RunId, ClientSessionId>>>,
}

#[async_trait::async_trait]
impl SessionResolver for HostSessionResolver {
    async fn resolve_client_session(&self, event: &AppEventEnvelope) -> Option<ClientSessionId> {
        // GuiEventBus 把 Core 事件标成 EventStream::Session；mock / 部分测试
        // 用 EventStream::Run。两种都要能回到 pending prompt，否则
        // session/prompt 会一直等不到 stopReason。
        let run_id = match &event.stream {
            EventStream::Run(run_id) => Some(run_id),
            _ => run_id_of(&event.payload),
        }?;
        self.run_sessions
            .lock()
            .expect("acp-host run map mutex")
            .get(run_id)
            .cloned()
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

/// ACP v1 宿主（in-process 胶水，无传输假设）。
pub struct AcpHost {
    command_host: Arc<dyn AcpCommandHost>,
    registry: Arc<SessionRegistry>,
    factory: AcpClientAdapterFactory,
    session_resolver: Arc<dyn SessionResolver>,
    /// initialize 成功后设置的协商产物。
    negotiated: Mutex<Option<crate::acp::adapter::NegotiatedAcpAdapter>>,
    connection_id: ConnectionId,
    /// client session → 当前 ownership (epoch, revision)，随 attach/reattach/close 更新。
    session_contexts: Mutex<BTreeMap<ClientSessionId, (u64, u64)>>,
    occupancy: Mutex<BTreeMap<ClientSessionId, PromptOccupancy>>,
    run_sessions: Arc<Mutex<BTreeMap<RunId, ClientSessionId>>>,
    pending_prompts: Mutex<HashMap<JsonRpcId, PendingPrompt>>,
    pending_permissions: Mutex<HashMap<JsonRpcId, PendingPermission>>,
    outbox: Mutex<VecDeque<OutboxItem>>,
    next_request_id: AtomicU64,
    /// 事件泵与 prompt 注册之间的互斥：保证 run→session 映射先于任何 drain。
    prompt_gate: tokio::sync::Mutex<()>,
    event_rx: tokio::sync::Mutex<broadcast::Receiver<AppEventEnvelope>>,
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
        let run_sessions = Arc::new(Mutex::new(BTreeMap::new()));
        let session_resolver: Arc<dyn SessionResolver> = Arc::new(HostSessionResolver {
            run_sessions: Arc::clone(&run_sessions),
        });
        let factory = AcpClientAdapterFactory::new(
            crate::acp::adapter::ACP_SUPPORTED_CAPABILITIES
                .iter()
                .map(|name| ClientCapability::new(*name)),
            Arc::clone(&registry),
            cwd_resolver,
            Arc::clone(&session_resolver),
            Implementation {
                name: ACP_AGENT_NAME.into(),
                title: Some("Pawork ACP Host".into()),
                version: ACP_AGENT_VERSION.into(),
            },
        );
        Self {
            command_host,
            registry,
            factory,
            session_resolver,
            negotiated: Mutex::new(None),
            connection_id: ConnectionId::from(format!(
                "acp-connection-{}-{}",
                std::process::id(),
                ACP_CONNECTION_SEQUENCE.fetch_add(1, Ordering::Relaxed)
            )),
            session_contexts: Mutex::new(BTreeMap::new()),
            occupancy: Mutex::new(BTreeMap::new()),
            run_sessions,
            pending_prompts: Mutex::new(HashMap::new()),
            pending_permissions: Mutex::new(HashMap::new()),
            outbox: Mutex::new(VecDeque::new()),
            next_request_id: AtomicU64::new(1),
            prompt_gate: tokio::sync::Mutex::new(()),
            event_rx: tokio::sync::Mutex::new(event_rx),
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
        std::mem::take(&mut *self.outbox.lock().expect("acp-host outbox mutex"))
            .into_iter()
            .collect()
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
                    let _ = completion.try_send(resolution);
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
                let _ = completion.try_send(resolution);
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
                let _ = completion.try_send(resolution);
            }
        }
    }

    /// 订阅滞后且无法可靠补事件时 fail-closed：解除全部未决 prompt / 权限请求。
    pub fn fail_closed_all_prompts(&self, reason: &str) {
        tracing::warn!(
            reason,
            "acp host fail-closed: releasing all in-flight prompts"
        );
        let _occupancy =
            std::mem::take(&mut *self.occupancy.lock().expect("acp-host occupancy mutex"));
        let prompts =
            std::mem::take(&mut *self.pending_prompts.lock().expect("acp-host prompts mutex"));
        let _permissions = std::mem::take(
            &mut *self
                .pending_permissions
                .lock()
                .expect("acp-host permissions mutex"),
        );
        self.run_sessions
            .lock()
            .expect("acp-host run sessions mutex")
            .clear();
        for prompt in prompts.into_values() {
            let _ = prompt.completion.try_send(PromptResolution::Failed);
        }
        self.resolve_queued_prompts();
        let _ = reason;
    }

    /// 当前是否有未完成 run（供事件泵循环判定退出）。
    pub fn has_active_runs(&self) -> bool {
        !self
            .occupancy
            .lock()
            .expect("acp-host occupancy mutex")
            .is_empty()
    }

    /// 指定 client session 当前绑定的 run id。
    pub fn pending_run(&self, client_session_id: &ClientSessionId) -> Option<RunId> {
        self.occupancy
            .lock()
            .expect("acp-host occupancy mutex")
            .get(client_session_id)
            .and_then(|occupancy| occupancy.run_id.clone())
    }

    /// 握手时被显式降级的客户端能力清单（协商审计）。
    pub fn degraded_capabilities(&self) -> Vec<ClientCapability> {
        self.negotiated
            .lock()
            .expect("acp-host negotiated mutex")
            .as_ref()
            .map(|negotiated| negotiated.degraded.clone())
            .unwrap_or_default()
    }

    /// 是否已完成 initialize。
    pub fn is_initialized(&self) -> bool {
        self.negotiated
            .lock()
            .expect("acp-host negotiated mutex")
            .is_some()
    }

    // ------------------------------------------------------------------
    // 入站消息入口
    // ------------------------------------------------------------------

    /// 处理 client → agent 的 JSON-RPC 请求，返回 result（或 JSON-RPC 错误）。
    /// `session/prompt` 会等待 run 终态后才返回。
    pub async fn handle_request(
        &self,
        id: JsonRpcId,
        method: &str,
        params: Option<Value>,
    ) -> Result<Value, JsonRpcError> {
        if method == "initialize" {
            return self.initialize(params).await;
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
            CanonicalClientRequest::Command(envelope) => {
                match &envelope.command {
                    AppCommand::SessionCreate { .. } => {
                        self.release_reservation(reserved_session);
                        self.session_new(request).await
                    }
                    AppCommand::RunStart { .. } => self.session_prompt(&id, &params, request).await,
                    other => {
                        self.release_reservation(reserved_session);
                        Err(JsonRpcError::new(
                        ERROR_METHOD_NOT_FOUND,
                        format!("method `{method}` decodes to unsupported canonical command {other:?}"),
                    ))
                    }
                }
            }
            CanonicalClientRequest::Reattach { .. } => {
                self.release_reservation(reserved_session);
                self.session_resume(&params, request).await
            }
            CanonicalClientRequest::Disconnect { .. } => {
                self.release_reservation(reserved_session);
                self.session_close(request).await
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

    /// 处理 client → agent 的 JSON-RPC 通知（`session/cancel`、`$/cancel_request`）。
    pub async fn handle_notification(
        &self,
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
                let params =
                    serde_json::from_value::<CancelRequestParams>(params).map_err(|error| {
                        JsonRpcError::new(crate::acp::wire::ERROR_INVALID_PARAMS, error.to_string())
                    })?;
                self.cancel_request(&params.request_id).await;
                Ok(())
            }
            other => Err(JsonRpcError::new(
                ERROR_METHOD_NOT_FOUND,
                format!("unknown ACP notification `{other}`"),
            )),
        }
    }

    /// 处理 client → agent 的 JSON-RPC 响应（当前只关联 `session/request_permission`）。
    pub async fn handle_response(
        &self,
        id: JsonRpcId,
        result: Result<Value, JsonRpcError>,
    ) -> Result<(), JsonRpcError> {
        let Some(permission) = self
            .pending_permissions
            .lock()
            .expect("acp-host permissions mutex")
            .remove(&id)
        else {
            return Ok(());
        };
        let decision = match result {
            Ok(value) => match self.adapter()?.decode_permission_response(value) {
                Ok(crate::acp::adapter::PermissionDecision::Selected { option_id }) => {
                    map::decision_for_option(&option_id).map_err(|error| jsonrpc_error(&error))?
                }
                Ok(crate::acp::adapter::PermissionDecision::Cancelled) => ApprovalDecision::Cancel,
                Err(error) => return Err(jsonrpc_error(&error)),
            },
            Err(error) if error.code == ERROR_REQUEST_CANCELLED => ApprovalDecision::Cancel,
            Err(_) => ApprovalDecision::Deny,
        };
        let envelope = self.adapter()?.command_envelope(
            &format!("permission-{}", id),
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

    /// 冲刷已订阅的 Core 事件并回译。
    pub async fn drain_and_pump(&self) {
        let mut events = Vec::new();
        let mut rx = self.event_rx.lock().await;
        loop {
            match rx.try_recv() {
                Ok(event) => events.push(event),
                Err(broadcast::error::TryRecvError::Empty) => break,
                Err(broadcast::error::TryRecvError::Closed) => break,
                Err(broadcast::error::TryRecvError::Lagged(_)) => {
                    drop(rx);
                    self.fail_closed_all_prompts("event subscription lagged");
                    return;
                }
            }
        }
        drop(rx);
        if !events.is_empty() {
            self.pump_events(events).await;
        }
    }

    /// 回译给定 canonical 事件（按 run 归属路由；非归属/无 ACP 表示的事件跳过）。
    pub async fn pump_events(&self, events: Vec<AppEventEnvelope>) {
        let _gate = self.prompt_gate.lock().await;
        for envelope in events {
            let Some(client_session_id) = self
                .session_resolver
                .resolve_client_session(&envelope)
                .await
            else {
                continue;
            };
            match &envelope.payload {
                AppEvent::RunChanged { run_id, state } => {
                    if terminal_state(state) {
                        self.resolve_prompt(run_id, state).await;
                    }
                }
                AppEvent::ToolApprovalRequired { .. } => {
                    self.emit_permission_request(client_session_id, &envelope.payload)
                        .await;
                }
                _ => self.emit_update(client_session_id, &envelope).await,
            }
        }
    }

    // ------------------------------------------------------------------
    // 握手与能力协商
    // ------------------------------------------------------------------

    async fn initialize(&self, params: Option<Value>) -> Result<Value, JsonRpcError> {
        if self.is_initialized() {
            return Err(JsonRpcError::new(
                ERROR_INVALID_REQUEST,
                "initialize was already completed; this host accepts one handshake per connection",
            ));
        }
        let params = serde_json::from_value::<InitializeParams>(params.unwrap_or(Value::Null))
            .map_err(|error| {
                JsonRpcError::new(crate::acp::wire::ERROR_INVALID_PARAMS, error.to_string())
            })?;
        params
            .reject_unknown("initialize")
            .map_err(|message| JsonRpcError::new(crate::acp::wire::ERROR_INVALID_PARAMS, message))?;
        if params.protocol_version != PROTOCOL_VERSION {
            return Err(JsonRpcError::new(
                crate::acp::wire::ERROR_INVALID_PARAMS,
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
            protocol: ClientProtocol::new(crate::acp::adapter::ACP_PROTOCOL),
            protocol_version: PROTOCOL_VERSION.to_string(),
            client_version,
            revision: 1,
            capabilities: declared_client_capabilities(&params.client_capabilities),
        };
        let negotiated = self
            .factory
            .create_concrete(snapshot)
            .map_err(|error| jsonrpc_error(&error))?;
        *self.negotiated.lock().expect("acp-host negotiated mutex") = Some(negotiated);
        Ok(serde_json::to_value(InitializeResult {
            protocol_version: PROTOCOL_VERSION,
            agent_capabilities: crate::acp::wire::AgentCapabilities {
                session_capabilities: crate::acp::wire::SessionCapabilities {
                    resume: Some(crate::acp::wire::EmptyCapability {}),
                    close: Some(crate::acp::wire::EmptyCapability {}),
                    ..crate::acp::wire::SessionCapabilities::default()
                },
                ..crate::acp::wire::AgentCapabilities::default()
            },
            agent_info: Some(Implementation {
                name: ACP_AGENT_NAME.into(),
                title: Some("Pawork ACP Host".into()),
                version: ACP_AGENT_VERSION.into(),
            }),
            auth_methods: Vec::new(),
        })
        .expect("InitializeResult always serializes"))
    }

    // ------------------------------------------------------------------
    // 会话生命周期
    // ------------------------------------------------------------------

    async fn session_new(&self, request: CanonicalClientRequest) -> Result<Value, JsonRpcError> {
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
                    crate::acp::wire::ERROR_INTERNAL,
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
                crate::acp::wire::ERROR_INTERNAL,
                "session attach did not produce a session state",
            ));
        };
        self.session_contexts
            .lock()
            .expect("acp-host session contexts mutex")
            .insert(
                client_session_id,
                (record.ownership_epoch, record.revision),
            );
        tracing::debug!(session_id, "acp session/new attached");
        Ok(serde_json::to_value(SessionNewResult { session_id })
            .expect("SessionNewResult always serializes"))
    }

    async fn session_prompt(
        &self,
        id: &JsonRpcId,
        params: &Value,
        request: CanonicalClientRequest,
    ) -> Result<Value, JsonRpcError> {
        let session_id = params
            .get("sessionId")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                JsonRpcError::new(
                    crate::acp::wire::ERROR_INVALID_PARAMS,
                    "session/prompt params must carry sessionId",
                )
            })?;
        let client_session_id = ClientSessionId::new(session_id);
        {
            let mut occupancy = self.occupancy.lock().expect("acp-host occupancy mutex");
            occupancy
                .entry(client_session_id.clone())
                .or_insert_with(|| PromptOccupancy {
                    request_id: id.clone(),
                    run_id: None,
                    early_session_cancel: false,
                    early_request_cancel: false,
                });
        }
        let (completion_tx, mut completion_rx) = tokio::sync::mpsc::channel(1);
        let context = match self.session_context(&client_session_id).await {
            Ok(context) => context,
            Err(error) => {
                self.release_occupancy(&client_session_id, None);
                return Err(error);
            }
        };
        let _gate = self.prompt_gate.lock().await;
        let response = self
            .dispatch_canonical(context, request)
            .await
            .map_err(|error| {
                self.release_occupancy(&client_session_id, None);
                jsonrpc_error(&error)
            })?;
        let run_id = match accepted_run_id(response, "session/prompt") {
            Ok(run_id) => run_id,
            Err(error) => {
                self.release_occupancy(&client_session_id, None);
                return Err(error);
            }
        };
        {
            let mut occupancy = self.occupancy.lock().expect("acp-host occupancy mutex");
            if let Some(slot) = occupancy.get_mut(&client_session_id) {
                slot.run_id = Some(run_id.clone());
            }
        }
        self.run_sessions
            .lock()
            .expect("acp-host run sessions mutex")
            .insert(run_id.clone(), client_session_id.clone());
        self.pending_prompts
            .lock()
            .expect("acp-host prompts mutex")
            .insert(
                id.clone(),
                PendingPrompt {
                    client_session_id: client_session_id.clone(),
                    run_id,
                    completion: completion_tx,
                },
            );
        let (replay_session_cancel, replay_request_cancel) = {
            let mut occupancy = self.occupancy.lock().expect("acp-host occupancy mutex");
            occupancy
                .get_mut(&client_session_id)
                .map(|slot| {
                    let session = slot.early_session_cancel;
                    let request = slot.early_request_cancel;
                    slot.early_session_cancel = false;
                    slot.early_request_cancel = false;
                    (session, request)
                })
                .unwrap_or((false, false))
        };
        drop(_gate);
        if replay_session_cancel {
            self.cancel_session(&client_session_id).await;
        } else if replay_request_cancel {
            self.cancel_request(id).await;
        }
        match completion_rx.recv().await {
            Some(PromptResolution::Stopped(reason)) => {
                Ok(serde_json::to_value(SessionPromptResult {
                    stop_reason: reason,
                })
                .expect("SessionPromptResult always serializes"))
            }
            Some(PromptResolution::Failed) => Err(JsonRpcError::new(
                crate::acp::wire::ERROR_INTERNAL,
                "prompt turn failed in Core",
            )),
            None => Err(JsonRpcError::new(
                crate::acp::wire::ERROR_INTERNAL,
                "prompt turn ended without a resolution",
            )),
        }
    }

    async fn session_resume(
        &self,
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
            unreachable!("session/resume decodes to Reattach");
        };
        let record = self.registry.get(&client_session_id).await.ok_or_else(|| {
            JsonRpcError::new(
                crate::acp::wire::ERROR_RESOURCE_NOT_FOUND,
                format!("unknown client session `{}`", client_session_id.0),
            )
        })?;
        if !self.core_session_exists(&record.core_session_id).await {
            return Err(JsonRpcError::new(
                crate::acp::wire::ERROR_RESOURCE_NOT_FOUND,
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
            return Err(JsonRpcError::new(
                crate::acp::wire::ERROR_INTERNAL,
                "session/resume did not produce a session state",
            ));
        };
        self.session_contexts
            .lock()
            .expect("acp-host session contexts mutex")
            .insert(client_session_id, (record.ownership_epoch, record.revision));
        Ok(json!({}))
    }

    async fn session_close(&self, request: CanonicalClientRequest) -> Result<Value, JsonRpcError> {
        let CanonicalClientRequest::Disconnect {
            client_session_id,
            ownership_epoch,
            revision,
            updated_at,
        } = request
        else {
            unreachable!("session/close decodes to Disconnect");
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
            return Err(JsonRpcError::new(
                crate::acp::wire::ERROR_INTERNAL,
                "session/close did not produce a session state",
            ));
        };
        self.session_contexts
            .lock()
            .expect("acp-host session contexts mutex")
            .insert(client_session_id, (record.ownership_epoch, record.revision));
        Ok(json!({}))
    }

    // ------------------------------------------------------------------
    // 取消
    // ------------------------------------------------------------------

    async fn cancel_session(&self, client_session_id: &ClientSessionId) {
        let run_id: RunId = {
            let mut occupancy = self.occupancy.lock().expect("acp-host occupancy mutex");
            match occupancy.get_mut(client_session_id) {
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
            let _ = self
                .dispatch_attached(
                    client_session_id,
                    adapter.command_envelope(
                        &format!("session-cancel-{run_id}"),
                        AppCommand::RunCancel {
                            run_id: run_id.clone(),
                        },
                    ),
                )
                .await;
        }
        let cascaded: Vec<JsonRpcId> = self
            .pending_permissions
            .lock()
            .expect("acp-host permissions mutex")
            .iter()
            .filter(|(_, pending)| &pending.client_session_id == client_session_id)
            .map(|(id, _)| id.clone())
            .collect();
        for id in cascaded {
            self.pending_permissions
                .lock()
                .expect("acp-host permissions mutex")
                .remove(&id);
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

    async fn cancel_request(&self, request_id: &JsonRpcId) {
        let prompt = self
            .pending_prompts
            .lock()
            .expect("acp-host prompts mutex")
            .get(request_id)
            .map(|prompt| (prompt.client_session_id.clone(), prompt.run_id.clone()));
        if let Some((client_session_id, run_id)) = prompt {
            if let Ok(adapter) = self.adapter() {
                let _ = self
                    .dispatch_attached(
                        &client_session_id,
                        adapter.command_envelope(
                            &format!("cancel-request-{run_id}"),
                            AppCommand::RunCancel { run_id },
                        ),
                    )
                    .await;
            }
            return;
        }
        let permission = self
            .pending_permissions
            .lock()
            .expect("acp-host permissions mutex")
            .remove(request_id);
        if let Some(permission) = permission {
            if let Ok(adapter) = self.adapter() {
                let _ = self
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
                    .await;
            }
            return;
        }
        let mut occupancy = self.occupancy.lock().expect("acp-host occupancy mutex");
        if let Some(slot) = occupancy
            .values_mut()
            .find(|slot| &slot.request_id == request_id)
        {
            slot.early_request_cancel = true;
        }
    }

    // ------------------------------------------------------------------
    // 事件回译
    // ------------------------------------------------------------------

    async fn resolve_prompt(&self, run_id: &RunId, state: &RunState) {
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
        let (client_session_id, completion) = {
            let mut prompts = self.pending_prompts.lock().expect("acp-host prompts mutex");
            let id = prompts
                .iter()
                .find(|(_, prompt)| &prompt.run_id == run_id)
                .map(|(id, _)| id.clone());
            let Some(id) = id else {
                return;
            };
            let prompt = prompts.remove(&id).expect("found pending prompt");
            (prompt.client_session_id, prompt.completion)
        };
        self.release_occupancy(&client_session_id, Some(run_id));
        self.push_outbox(OutboxItem::FlushBarrier {
            completion,
            resolution,
        });
    }

    async fn emit_permission_request(&self, client_session_id: ClientSessionId, event: &AppEvent) {
        let result = async {
            let adapter = self.adapter()?;
            let params = adapter
                .permission_request(event, &client_session_id)
                .await
                .map_err(|error| jsonrpc_error(&error))?;
            let run_id = run_id_of(event)
                .ok_or_else(|| {
                    JsonRpcError::new(
                        crate::acp::wire::ERROR_INTERNAL,
                        "ToolApprovalRequired event without run_id",
                    )
                })?
                .clone();
            let tool_call_id = tool_call_id_of(event)
                .ok_or_else(|| {
                    JsonRpcError::new(
                        crate::acp::wire::ERROR_INTERNAL,
                        "ToolApprovalRequired event without tool_call_id",
                    )
                })?
                .clone();
            let id = Value::Number(self.next_request_id.fetch_add(1, Ordering::SeqCst).into());
            self.pending_permissions
                .lock()
                .expect("acp-host permissions mutex")
                .insert(
                    id.clone(),
                    PendingPermission {
                        run_id,
                        tool_call_id,
                        client_session_id: client_session_id.clone(),
                    },
                );
            self.push_frame(
                JsonRpcRequest {
                    jsonrpc: "2.0".into(),
                    id,
                    method: "session/request_permission".into(),
                    params: Some(serde_json::to_value(params).expect("params serialize")),
                }
                .to_value(),
            );
            Ok::<(), JsonRpcError>(())
        }
        .await;
        if let Err(error) = result {
            tracing::warn!(session_id = %client_session_id.0, code = error.code, message = %error.message, "acp permission request emission failed");
        }
    }

    async fn emit_update(&self, client_session_id: ClientSessionId, envelope: &AppEventEnvelope) {
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
            tracing::warn!(session_id = %client_session_id.0, code = error.code, message = %error.message, "acp session/update emission failed");
        }
    }

    // ------------------------------------------------------------------
    // 内部工具
    // ------------------------------------------------------------------

    fn push_frame(&self, frame: Value) {
        self.push_outbox(OutboxItem::Frame(frame));
    }

    fn push_outbox(&self, item: OutboxItem) {
        self.outbox
            .lock()
            .expect("acp-host outbox mutex")
            .push_back(item);
    }

    fn adapter(&self) -> Result<Arc<AcpClientAdapter>, JsonRpcError> {
        self.negotiated
            .lock()
            .expect("acp-host negotiated mutex")
            .as_ref()
            .map(|negotiated| Arc::clone(&negotiated.adapter))
            .ok_or_else(|| {
                JsonRpcError::new(
                    ERROR_INVALID_REQUEST,
                    "host is not initialized: call initialize first",
                )
            })
    }

    fn release_occupancy(&self, client_session_id: &ClientSessionId, run_id: Option<&RunId>) {
        self.occupancy
            .lock()
            .expect("acp-host occupancy mutex")
            .remove(client_session_id);
        if let Some(run_id) = run_id {
            self.run_sessions
                .lock()
                .expect("acp-host run sessions mutex")
                .remove(run_id);
        }
    }

    fn reserve_prompt_occupancy(
        &self,
        id: &JsonRpcId,
        params: &Value,
    ) -> Result<ClientSessionId, JsonRpcError> {
        let session_id = params
            .get("sessionId")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                JsonRpcError::new(
                    crate::acp::wire::ERROR_INVALID_PARAMS,
                    "session/prompt params must carry sessionId",
                )
            })?;
        let client_session_id = ClientSessionId::new(session_id);
        let mut occupancy = self.occupancy.lock().expect("acp-host occupancy mutex");
        if occupancy.contains_key(&client_session_id) {
            return Err(JsonRpcError::new(
                ERROR_INVALID_REQUEST,
                format!(
                    "session `{session_id}` already has an active prompt turn; \
                     this host supports one prompt per session at a time"
                ),
            ));
        }
        occupancy.insert(
            client_session_id.clone(),
            PromptOccupancy {
                request_id: id.clone(),
                run_id: None,
                early_session_cancel: false,
                early_request_cancel: false,
            },
        );
        Ok(client_session_id)
    }

    fn release_reservation(&self, reserved: Option<ClientSessionId>) {
        if let Some(client_session_id) = reserved {
            self.release_occupancy(&client_session_id, None);
        }
    }

    async fn session_context(
        &self,
        client_session_id: &ClientSessionId,
    ) -> Result<AdapterSessionContext, JsonRpcError> {
        let (ownership_epoch, revision) = {
            let cached = self
                .session_contexts
                .lock()
                .expect("acp-host session contexts mutex")
                .get(client_session_id)
                .copied();
            match cached {
                Some(epoch_revision) => epoch_revision,
                None => {
                    let record = self.registry.get(client_session_id).await.ok_or_else(|| {
                        JsonRpcError::new(
                            crate::acp::wire::ERROR_RESOURCE_NOT_FOUND,
                            format!("unknown client session `{}`", client_session_id.0),
                        )
                    })?;
                    self.session_contexts
                        .lock()
                        .expect("acp-host session contexts mutex")
                        .insert(
                            client_session_id.clone(),
                            (record.ownership_epoch, record.revision),
                        );
                    (record.ownership_epoch, record.revision)
                }
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
        &self,
        client_session_id: &ClientSessionId,
        envelope: AppCommandEnvelope,
    ) -> Result<(), JsonRpcError> {
        let context = self.session_context(client_session_id).await?;
        let response = self
            .dispatch_canonical(context, CanonicalClientRequest::Command(envelope))
            .await
            .map_err(|error| jsonrpc_error(&error))?;
        canonical_response_value(response, "command")?;
        Ok(())
    }

    async fn dispatch_canonical(
        &self,
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
                let response = self
                    .command_host
                    .dispatch(envelope)
                    .await
                    .map_err(host_unavailable)?;
                Ok(CanonicalCoreFrame::Response(response))
            }
            CanonicalClientRequest::Query(envelope) => {
                self.require_attached(&context).await?;
                let response = self
                    .command_host
                    .query(envelope)
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
        if record.ownership_epoch != context.ownership_epoch || record.revision != context.revision
        {
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
        other => Err(JsonRpcError::new(
            crate::acp::wire::ERROR_INTERNAL,
            format!("{context} produced unexpected canonical frame {other:?}"),
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
            AppResponse::Accepted { .. } => Err(JsonRpcError::new(
                crate::acp::wire::ERROR_INTERNAL,
                format!("{context} was accepted but did not report a run id"),
            )),
            _ => Err(JsonRpcError::new(
                crate::acp::wire::ERROR_INTERNAL,
                format!("{context} produced an unexpected response"),
            )),
        },
        CanonicalCoreFrame::Error(error) => Err(JsonRpcError::new(
            map::jsonrpc_code_for_frame(&error),
            error.message,
        )),
        other => Err(JsonRpcError::new(
            crate::acp::wire::ERROR_INTERNAL,
            format!("{context} produced unexpected canonical frame {other:?}"),
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
            "session binding mismatch: negotiated adapter protocol {:?} / capability snapshot \
             vs authoritative record protocol {:?}",
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
