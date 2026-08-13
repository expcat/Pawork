//! ACP Host 胶水层（P17-7）：把 ACP v1 JSON-RPC 消息接到 canonical 执行面。
//!
//! 本层只做三件事：
//!
//! 1. **握手与协商**：`initialize` 校验 `protocolVersion == 1`（拒绝实验 v2），
//!    经 [`AcpClientAdapterFactory`] 生成协商 adapter，未支持能力显式降级记录。
//! 2. **会话生命周期**：`session/new`（SessionCreate → Attach）、`session/prompt`
//!    （RunStart + 等待终态）、`session/resume`（Reattach）、`session/close`
//!    （RunCancel → Disconnect）、`session/cancel` / `$/cancel_request` 通知。
//! 3. **事件回译**：轮询 `AppService::drain_events`（canonical 事件唯一出口），
//!    按 run → client session 归属路由，经 adapter 编码为 `session/update`
//!    通知或 `session/request_permission` 请求。
//!
//! 所有权/凭证/Core 一律不在这里重建：session 记录只读写
//! [`SessionRegistry`]，命令/查询全部经 [`ClientAdapterHost::dispatch`]
//! 走 `app-service` 的 authoritative 检查（protocol/capability/epoch/revision）。
//! 出站消息进入单一有序 outbox（帧 + prompt 终态 flush barrier），由传输层
//! 按序冲刷：屏障前的 `session/update` 全部写出后才释放 prompt 完成信号，
//! 保证 `session/prompt` 响应不早于本 prompt 的回译事件写出；本层不依赖
//! 任何具体传输。
//!
//! prompt 占用是原子占位：同 session 同时只允许一个 turn。idle / 终态后的
//! cancel 不落库；只有注册窗口内的 early cancel 会在 activate 时兑现。

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use agent_domain::{ConnectionId, QueryId, RunId, SessionId, ToolCallId, WorkspaceId};
use client_adapter_api::{
    AdapterError, AdapterSessionContext, CanonicalClientRequest, CanonicalCoreFrame,
    CapabilitySnapshot, ClientAdapter, ClientCapability, ClientFrame, ClientProtocol,
    ClientSessionId, ClientSessionRecord, ClientSessionState, SessionRegistry,
    CLIENT_ADAPTER_SCHEMA_VERSION,
};
use core_api::{
    ActorIdentity, AppCommand, AppCommandEnvelope, AppEvent, AppEventEnvelope, AppQuery,
    AppQueryEnvelope, AppResponse, CommandSource, EventStream, GlobalSequence, RunState,
    API_VERSION,
};
use serde_json::{json, Value};
use subscription_hub::{EventHub, HubError, HubSubscription};

use crate::adapter::{AcpClientAdapter, AcpClientAdapterFactory, CwdResolver, SessionResolver};
use crate::map;
use crate::now_timestamp;
use crate::wire::{
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
/// （组件级前缀匹配），否则显式 `HostUnavailable`。
struct HostCwdResolver {
    service: Arc<app_service::AppService>,
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
        let response = self.service.dispatch_query(envelope);
        let AppResponse::Data(value) = response.response else {
            return Err(AdapterError::HostUnavailable(
                "workspace list query failed; cannot resolve cwd".into(),
            ));
        };
        // 规范化两侧再比较：进程登记的 root 来自 `current_dir()`（macOS 上
        // 已解析 /var → /private/var），而 ACP 客户端传入的 cwd 常是原始
        // 环境变量路径（未解析 symlink / 含重复分隔符）。仅做字面前缀匹配
        // 会把同一目录误判为"不在任何 workspace 内"。
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
        let EventStream::Run(run_id) = &event.stream else {
            return None;
        };
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
///
/// 全部出站消息（`session/update` 通知 / `session/request_permission` 请求 /
/// `$/cancel_request` 通知）与 prompt 终态屏障共用同一条先进先出队列，由
/// 同一个消费方（传输层冲刷）按序处理：屏障之前的帧全部写出后，才释放
/// 对应的 prompt 完成信号——`session/prompt` 响应因此保证在该 prompt 的
/// 全部 `session/update` 写出之后才返回。
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
    service: Arc<app_service::AppService>,
    client_host: app_service::ClientAdapterHost,
    registry: Arc<SessionRegistry>,
    hub: Arc<EventHub>,
    factory: AcpClientAdapterFactory,
    session_resolver: Arc<dyn SessionResolver>,
    /// initialize 成功后设置的协商产物。
    negotiated: Mutex<Option<crate::adapter::NegotiatedAcpAdapter>>,
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
}

impl AcpHost {
    pub fn new(service: Arc<app_service::AppService>, registry: Arc<SessionRegistry>) -> Self {
        Self::with_hub(service, registry, Arc::new(EventHub::new()))
    }

    /// 以共享 Event Hub 装配（正式宿主路径）：调用方运行 EventPump 把
    /// `AppService` 事件发布到该 Hub，本宿主经 [`AcpHost::subscribe`] 订阅
    /// 同一事件流，不与 EventPump 竞争 `drain_events`。测试路径（无
    /// EventPump）仍用 [`AcpHost::new`] + [`AcpHost::drain_and_pump`]。
    pub fn with_hub(
        service: Arc<app_service::AppService>,
        registry: Arc<SessionRegistry>,
        hub: Arc<EventHub>,
    ) -> Self {
        let client_host = app_service::ClientAdapterHost::new(
            Arc::clone(&service),
            Arc::clone(&hub),
            Arc::clone(&registry),
        );
        let identity_name = format!("acp:{ACP_AGENT_NAME}");
        let cwd_resolver = Arc::new(HostCwdResolver {
            service: Arc::clone(&service),
            identity_name: identity_name.clone(),
            next_query: AtomicU64::new(0),
        });
        let run_sessions = Arc::new(Mutex::new(BTreeMap::new()));
        let session_resolver: Arc<dyn SessionResolver> = Arc::new(HostSessionResolver {
            run_sessions: Arc::clone(&run_sessions),
        });
        let factory = AcpClientAdapterFactory::new(
            crate::adapter::ACP_SUPPORTED_CAPABILITIES
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
            service,
            client_host,
            registry,
            hub,
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

    pub fn client_host(&self) -> &app_service::ClientAdapterHost {
        &self.client_host
    }

    /// 订阅共享 Hub 的 canonical 事件流（正式宿主事件源；回译入口见
    /// [`AcpHost::pump_events`]）。
    pub fn subscribe(&self) -> HubSubscription {
        self.hub.subscribe()
    }

    /// 共享 Event Hub（传输层 Lagged 后按全局序列 replay 用）。
    pub fn hub(&self) -> &Arc<EventHub> {
        &self.hub
    }

    /// 取走全部当前可读的出站条目（同步、非阻塞；传输层冲刷用，保持队列顺序）。
    pub fn drain_outbox_items(&self) -> Vec<OutboxItem> {
        std::mem::take(&mut *self.outbox.lock().expect("acp-host outbox mutex"))
            .into_iter()
            .collect()
    }

    /// 取走出站 JSON-RPC 消息（通知 + 请求），并清空 outbox；队列中的冲刷
    /// 屏障在把此前帧全部取走后就地释放（测试/宿主便捷视图：等价于传输层
    /// 把帧写出后再释放）。
    pub fn take_outbox(&self) -> Vec<Value> {
        let mut frames = Vec::new();
        for item in self.drain_outbox_items() {
            match item {
                OutboxItem::Frame(frame) => frames.push(frame),
                OutboxItem::FlushBarrier {
                    completion,
                    resolution,
                } => {
                    // 屏障释放：prompt 等待方在 recv，容量 1 的槽位必空；
                    // 等待方已 drop（取消/超时）时 try_send 失败，静默忽略。
                    let _ = completion.try_send(resolution);
                }
            }
        }
        frames
    }

    /// 传输层失败收尾：丢弃无法写出的帧，但仍释放队列中全部 prompt 屏障，
    /// 保证等待中的 `session/prompt` 调用方不会悬挂（取消/背压不丢）。
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

    /// 释放调用方已 drain 但仍未写出的剩余屏障。半写失败后必须调用，
    /// 不能再依赖宿主队列——那些条目已经离开 outbox。
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

    /// Hub Lagged 且无法可靠 replay 时 fail-closed：解除全部未决 prompt /
    /// 权限请求，避免静默丢终态或审批。
    pub fn fail_closed_all_prompts(&self, reason: &str) {
        tracing::warn!(reason, "acp host fail-closed: releasing all in-flight prompts");
        let _occupancy = std::mem::take(
            &mut *self.occupancy.lock().expect("acp-host occupancy mutex"),
        );
        let prompts = std::mem::take(
            &mut *self.pending_prompts.lock().expect("acp-host prompts mutex"),
        );
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
            // 直接投递失败：调用方可能已 drain outbox，不能再把屏障塞回队列。
            let _ = prompt.completion.try_send(PromptResolution::Failed);
        }
        self.resolve_queued_prompts();
        let _ = reason;
    }

    /// Hub Lagged 后按全局序列补回错过的事件；窗口不可用时返回错误，
    /// 由传输层 fail-closed，禁止静默丢终态 / 审批。
    pub async fn replay_missed_events(
        &self,
        last_seen: GlobalSequence,
    ) -> Result<GlobalSequence, String> {
        let from = GlobalSequence(last_seen.0.saturating_add(1));
        match self.hub.replay(from, None) {
            Ok(events) => {
                let next = events
                    .last()
                    .map(|event| event.global_sequence)
                    .unwrap_or(last_seen);
                if !events.is_empty() {
                    self.pump_events(events).await;
                }
                Ok(next)
            }
            Err(HubError::ReplayUnavailable {
                requested_from,
                earliest_available,
            }) => Err(format!(
                "hub replay unavailable from {requested_from:?}; earliest={earliest_available:?}"
            )),
            Err(error) => Err(format!("hub replay failed: {error}")),
        }
    }

    /// 当前是否有未完成 run（供事件泵循环判定退出）。
    pub fn has_active_runs(&self) -> bool {
        !self
            .occupancy
            .lock()
            .expect("acp-host occupancy mutex")
            .is_empty()
    }

    /// 指定 client session 当前绑定的 run id（取消/状态查询用；测试与宿主
    /// 观察面共用，不在私有 map 之外重建状态）。
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
        // P17-7：session/prompt 在 decode/dispense 之前的 await 之前预占 occupancy，
        // 覆盖注册窗口（decode await 及 dispatch 前的 await）内到达的 early cancel。
        // idle / 终态后的 cancel 此时无占位可命中，依旧被忽略，不会污染下一个 prompt。
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
                    self.session_new(request).await
                }
                // session_prompt 接管预占 occupancy 的完整生命周期（activate/
                // replay/释放）；此处不再触碰 occupancy。
                AppCommand::RunStart { .. } => {
                    self.session_prompt(&id, &params, request).await
                }
                other => {
                    self.release_reservation(reserved_session);
                    Err(JsonRpcError::new(
                        ERROR_METHOD_NOT_FOUND,
                        format!("method `{method}` decodes to unsupported canonical command {other:?}"),
                    ))
                }
            },
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
                    format!("method `{method}` has no host handler for canonical request {other:?}"),
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
                        JsonRpcError::new(crate::wire::ERROR_INVALID_PARAMS, error.to_string())
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
            // 未关联的响应：按 JSON-RPC 规范可忽略。
            return Ok(());
        };
        let decision = match result {
            Ok(value) => match self.adapter()?.decode_permission_response(value) {
                Ok(crate::adapter::PermissionDecision::Selected { option_id }) => {
                    map::decision_for_option(&option_id).map_err(|error| jsonrpc_error(&error))?
                }
                Ok(crate::adapter::PermissionDecision::Cancelled) => {
                    core_api::ApprovalDecision::Cancel
                }
                Err(error) => return Err(jsonrpc_error(&error)),
            },
            Err(error) if error.code == ERROR_REQUEST_CANCELLED => {
                core_api::ApprovalDecision::Cancel
            }
            Err(_) => core_api::ApprovalDecision::Deny,
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

    /// 冲刷 Core 事件并回译（canonical 事件唯一入口）。
    pub async fn drain_and_pump(&self) {
        let events = self.service.drain_events();
        self.pump_events(events).await;
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
                JsonRpcError::new(crate::wire::ERROR_INVALID_PARAMS, error.to_string())
            })?;
        params
            .reject_unknown("initialize")
            .map_err(|message| JsonRpcError::new(crate::wire::ERROR_INVALID_PARAMS, message))?;
        if params.protocol_version != PROTOCOL_VERSION {
            return Err(JsonRpcError::new(
                crate::wire::ERROR_INVALID_PARAMS,
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
            protocol: ClientProtocol::new(crate::adapter::ACP_PROTOCOL),
            protocol_version: PROTOCOL_VERSION.to_string(),
            client_version,
            revision: 1,
            capabilities: declared_client_capabilities(&params.client_capabilities),
        };
        let negotiated = self
            .factory
            .create_concrete(snapshot)
            .map_err(|error| jsonrpc_error(&error))?;
        *self.negotiated.lock().expect("acp-host negotiated mutex") = Some(negotiated.clone());
        Ok(serde_json::to_value(InitializeResult {
            protocol_version: PROTOCOL_VERSION,
            agent_capabilities: crate::wire::AgentCapabilities {
                session_capabilities: crate::wire::SessionCapabilities {
                    resume: Some(crate::wire::EmptyCapability {}),
                    close: Some(crate::wire::EmptyCapability {}),
                    ..crate::wire::SessionCapabilities::default()
                },
                ..crate::wire::AgentCapabilities::default()
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
        // SessionCreate 是引导例外：未 attach 允许，context 用占位 ownership。
        let placeholder = AdapterSessionContext {
            adapter: Arc::clone(&adapter) as Arc<dyn client_adapter_api::ClientAdapter>,
            client_session_id: ClientSessionId::new("acp-pending-session"),
            connection_id: self.connection_id.clone(),
            ownership_epoch: 0,
            revision: 0,
        };
        let response = self
            .client_host
            .dispatch(placeholder, request)
            .await
            .map_err(|error| jsonrpc_error(&error))?;
        let session = canonical_response_value(response, "session/new")?;
        let session_id = session
            .get("session_id")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                JsonRpcError::new(
                    crate::wire::ERROR_INTERNAL,
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
            adapter: Arc::clone(&adapter) as Arc<dyn client_adapter_api::ClientAdapter>,
            client_session_id: client_session_id.clone(),
            connection_id: self.connection_id.clone(),
            ownership_epoch: 1,
            revision: 1,
        };
        let response = self
            .client_host
            .dispatch(attach_context, CanonicalClientRequest::Attach(record))
            .await
            .map_err(|error| jsonrpc_error(&error))?;
        let CanonicalCoreFrame::SessionState(record) = response else {
            return Err(JsonRpcError::new(
                crate::wire::ERROR_INTERNAL,
                "session attach did not produce a session state",
            ));
        };
        self.session_contexts
            .lock()
            .expect("acp-host session contexts mutex")
            .insert(
                client_session_id.clone(),
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
                    crate::wire::ERROR_INVALID_PARAMS,
                    "session/prompt params must carry sessionId",
                )
            })?;
        let client_session_id = ClientSessionId::new(session_id);
        // occupancy 已由 handle_request 在 decode await 之前预占（覆盖注册窗口内的
        // early cancel）。并发双 prompt 已在 reserve_prompt_occupancy 拒绝；这里只
        // 保证占位存在（健壮兜底），随后 activate 时由现有 per-slot replay 兑现。
        {
            let mut occupancy = self
                .occupancy
                .lock()
                .expect("acp-host occupancy mutex");
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
        // 持有 prompt gate 跨过 dispatch + 注册：保证 run→session 映射先于
        // 任何事件泵 drain（dispatch 期间引擎可能已开始产出事件）。
        let _gate = self.prompt_gate.lock().await;
        let response = self
            .client_host
            .dispatch(context, request)
            .await
            .map_err(|error| {
                self.release_occupancy(&client_session_id, None);
                jsonrpc_error(&error)
            })?;
        // RunStart 响应携带该命令确定启动的 run id：并发 prompt 各自绑定
        // 自己的 run，不依赖全局 `last_started_run`（P17-7 评审 #3）。
        let run_id = match accepted_run_id(response, "session/prompt") {
            Ok(run_id) => run_id,
            Err(error) => {
                self.release_occupancy(&client_session_id, None);
                return Err(error);
            }
        };
        {
            let mut occupancy = self
                .occupancy
                .lock()
                .expect("acp-host occupancy mutex");
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
            let mut occupancy = self
                .occupancy
                .lock()
                .expect("acp-host occupancy mutex");
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
                crate::wire::ERROR_INTERNAL,
                "prompt turn failed in Core",
            )),
            None => Err(JsonRpcError::new(
                crate::wire::ERROR_INTERNAL,
                "prompt turn ended without a resolution",
            )),
        }
    }

    async fn session_resume(
        &self,
        params: &Value,
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
        // 跨 host/进程 resume：记录来自持久化 Session Registry（SQLite），但
        // 本 Core 实例的 aggregate 是内存态，可能没有对应的 core session。
        // 此时以 registry 权威 core_session_id 做**幂等 Core materialize**
        // （同 id 在本地 aggregate 重建会话记录，已存在即 no-op），随后直接
        // Reattach claim。不新建随机 session、不做 CAS 重绑：映射在任何
        // 时刻都稳定，并发 resume / 重试不产生 ghost session、不堆积
        // （旧实现 SessionCreate + rebind 在并发下会在败者 aggregate 留下
        // 无主随机 session，且每次重试都会再建一个）。
        let record = self.registry.get(&client_session_id).await.ok_or_else(|| {
            JsonRpcError::new(
                crate::wire::ERROR_RESOURCE_NOT_FOUND,
                format!("unknown client session `{}`", client_session_id.0),
            )
        })?;
        if !self
            .service
            .router()
            .aggregate()
            .session_exists(&record.core_session_id)
        {
            // workspace 由 resume params 的 cwd 解析（与 session/new 同一
            // 解析路径）；title 与 session/new 一致（cwd 作为标题）。
            let cwd = params.get("cwd").and_then(Value::as_str).ok_or_else(|| {
                JsonRpcError::new(
                    crate::wire::ERROR_INVALID_PARAMS,
                    "session/resume params must carry cwd",
                )
            })?;
            let workspace_id = self
                .factory
                .resolve_workspace(cwd)
                .await
                .map_err(|error| jsonrpc_error(&error))?;
            self.service
                .materialize_session(&record.core_session_id, &workspace_id, cwd)
                .map_err(|error| {
                    JsonRpcError::new(
                        crate::wire::ERROR_INTERNAL,
                        format!("cannot materialize core session: {error}"),
                    )
                })?;
        }
        let (claim_epoch, claim_revision) = (record.ownership_epoch, record.revision);
        let context = self.session_context(&client_session_id).await?;
        let response = self
            .client_host
            .dispatch(
                context,
                CanonicalClientRequest::Reattach {
                    client_session_id: client_session_id.clone(),
                    ownership_epoch: claim_epoch,
                    revision: claim_revision,
                    // resume 是重新 claim：registry 记录里的 connection_id 是
                    // 旧连接的，必须换成当前连接，否则 claim 会把会话挂回旧
                    // 连接（P17-7 评审 #2）。
                    connection_id: self.connection_id.clone(),
                    state,
                    updated_at,
                },
            )
            .await
            .map_err(|error| jsonrpc_error(&error))?;
        let CanonicalCoreFrame::SessionState(record) = response else {
            return Err(JsonRpcError::new(
                crate::wire::ERROR_INTERNAL,
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
        // ACP close 语义：先取消未完成工作（等同 session/cancel），再释放会话。
        self.cancel_session(&client_session_id).await;
        let context = self.session_context(&client_session_id).await?;
        let response = self
            .client_host
            .dispatch(
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
                crate::wire::ERROR_INTERNAL,
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

    /// `session/cancel`：取消该 session 的活跃 run，并级联取消挂起的权限请求。
    async fn cancel_session(&self, client_session_id: &ClientSessionId) {
        let run_id: RunId = {
            let mut occupancy = self
                .occupancy
                .lock()
                .expect("acp-host occupancy mutex");
            match occupancy.get_mut(client_session_id) {
                Some(slot) => {
                    if let Some(run_id) = slot.run_id.clone() {
                        slot.early_session_cancel = false;
                        run_id
                    } else {
                        // 注册窗口：只给当前占位兑现，不污染后续 prompt。
                        slot.early_session_cancel = true;
                        return;
                    }
                }
                // idle / 终态后：没有占用就不记账。
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
        // 级联：agent 发出的未决权限请求按 ACP cancellation 流程用
        // $/cancel_request 通知客户端取消，客户端以 -32800 响应确认。
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

    /// `$/cancel_request`：客户端取消自己发出的未决请求（如 session/prompt）。
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
            // 终态 RunChanged 到达后，prompt 会以 stopReason=cancelled 收尾。
            return;
        }
        // 客户端也可能取消 agent 发出的请求（非标准但可防御）：按取消处理。
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
                                decision: core_api::ApprovalDecision::Cancel,
                            },
                        ),
                    )
                    .await;
            }
            return;
        }
        // 仅当该 request 正处于注册窗口时记账；未知 / idle / 终态后一律忽略。
        let mut occupancy = self
            .occupancy
            .lock()
            .expect("acp-host occupancy mutex");
        if let Some(slot) = occupancy.values_mut().find(|slot| &slot.request_id == request_id) {
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
        // flush barrier：与 run 的全部出站帧同队列、排在其后；传输层写出
        // 这些帧后才释放本 prompt 的完成信号（prompt 响应因此不早于
        // session/update 写出）。
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
                        crate::wire::ERROR_INTERNAL,
                        "ToolApprovalRequired event without run_id",
                    )
                })?
                .clone();
            let tool_call_id = tool_call_id_of(event)
                .ok_or_else(|| {
                    JsonRpcError::new(
                        crate::wire::ERROR_INTERNAL,
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

    /// 把一帧 JSON-RPC 消息追加到单一有序 outbox（队尾）。
    fn push_frame(&self, frame: Value) {
        self.push_outbox(OutboxItem::Frame(frame));
    }

    /// 追加 outbox 条目（队尾；全部出站消息共享同一条有序队列）。
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

    /// 在 handle_request 的最早可达点（decode/dispense 之前的 await 之前）为
    /// `session/prompt` 预占 occupancy：使注册窗口内到达的 session/cancel 与
    /// `$/cancel_request` 能命中占位（记入 per-slot early flag），从而在 activate
    /// 后由现有 replay 兑现。idle / 终态后的 cancel 无占位可命中，依旧被忽略，
    /// 不污染下一个 prompt。
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
                    crate::wire::ERROR_INVALID_PARAMS,
                    "session/prompt params must carry sessionId",
                )
            })?;
        let client_session_id = ClientSessionId::new(session_id);
        let mut occupancy = self
            .occupancy
            .lock()
            .expect("acp-host occupancy mutex");
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

    /// 释放 handle_request 预占、但未进入 session_prompt 生命周期的占位
    /// （decode 失败 / canonical 命令非 RunStart 等路径）。
    fn release_reservation(&self, reserved: Option<ClientSessionId>) {
        if let Some(client_session_id) = reserved {
            self.release_occupancy(&client_session_id, None);
        }
    }

    async fn session_context(
        &self,
        client_session_id: &ClientSessionId,
    ) -> Result<AdapterSessionContext, JsonRpcError> {
        // 私有 map 是本连接 attach/reattach 的缓存；跨连接（新 AcpHost）
        // resume/close 时 map 为空，必须回退到 authoritative SessionRegistry
        // 构造 context（connection 用本连接 claim，epoch/revision 取记录现值），
        // 并把结果写回缓存供后续 prompt 复用（P17-7 评审 #2）。
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
                            crate::wire::ERROR_RESOURCE_NOT_FOUND,
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
            adapter: Arc::clone(&self.adapter()?) as Arc<dyn client_adapter_api::ClientAdapter>,
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
            .client_host
            .dispatch(context, CanonicalClientRequest::Command(envelope))
            .await
            .map_err(|error| jsonrpc_error(&error))?;
        canonical_response_value(response, "command")?;
        Ok(())
    }

    fn client_frame(&self, method: &str, id: &JsonRpcId, params: &Value) -> ClientFrame {
        ClientFrame {
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

/// 把 dispatch 结果转成 JSON-RPC result；Error frame 转为 JSON-RPC 错误。
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
            crate::wire::ERROR_INTERNAL,
            format!("{context} produced unexpected canonical frame {other:?}"),
        )),
    }
}

/// 从 RunStart dispatch 结果中取出该命令确定启动的 run id。Accepted 必须
/// 携带 run id（`AppResponse::run_id`），否则视为内部错误：并发来源各自
/// 绑定自己的 run，杜绝全局 `last_started_run` 竞态。
fn accepted_run_id(frame: CanonicalCoreFrame, context: &str) -> Result<RunId, JsonRpcError> {
    match frame {
        CanonicalCoreFrame::Response(envelope) => match envelope.response {
            AppResponse::Accepted {
                run_id: Some(run_id),
                ..
            } => Ok(run_id),
            AppResponse::Accepted { .. } => Err(JsonRpcError::new(
                crate::wire::ERROR_INTERNAL,
                format!("{context} was accepted but did not report a run id"),
            )),
            _ => Err(JsonRpcError::new(
                crate::wire::ERROR_INTERNAL,
                format!("{context} produced an unexpected response"),
            )),
        },
        CanonicalCoreFrame::Error(error) => Err(JsonRpcError::new(
            map::jsonrpc_code_for_frame(&error),
            error.message,
        )),
        other => Err(JsonRpcError::new(
            crate::wire::ERROR_INTERNAL,
            format!("{context} produced unexpected canonical frame {other:?}"),
        )),
    }
}

/// 从 ToolApprovalRequired 事件取 run id。
fn run_id_of(event: &AppEvent) -> Option<&RunId> {
    match event {
        AppEvent::ToolApprovalRequired { run_id, .. } => Some(run_id),
        _ => None,
    }
}

/// 从 ToolApprovalRequired 事件取 tool call id。
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

/// 客户端声明的能力 → 协商快照中的能力集合（与 ACP_SUPPORTED_CAPABILITIES
/// 求交后即为实际支持集；白名单外能力显式降级）。
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
