//! `IdeHostAdapter`：IDE 扩展 ↔ `pawork` Host 的连接器（P17-9 步骤 1/3/4/6）。
//!
//! 职责（只做协议翻译，不做业务决策）：
//! - 能力协商：IDE 能力 → `client-adapter-api` capability snapshot，经
//!   [`IdeClientAdapterFactory`] fail-closed 校验，并核对 Host 经
//!   SDK/Headless 握手授予的能力；
//! - session/run/event：复用 `SessionRegistry`（client/core session 绑定 +
//!   ownership epoch/revision）与 [`SdkChannel`]（`agent-sdk` / Headless）；
//! - 取消、重连（ownership reattach + 流重订阅）、IDE 诊断反向记录；
//! - 边界：不构造第二 Core、不消费 GUI Connection Protocol 帧。

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use agent_domain::{CommandId, ConnectionId, QueryId, RunId, SessionId, Timestamp, WorkspaceId};
use agent_sdk::{EventSubscription, PaworkClient, PaworkOptions, SdkError};
use client_adapter_api::{
    CanonicalClientRequest, CapabilitySnapshot, ClientAdapter, ClientAdapterFactory,
    ClientCapability, ClientFrame, ClientProtocol, ClientSessionId, ClientSessionRecord,
    ClientSessionState, InMemorySessionRegistryStore, SessionRegistry,
    CLIENT_ADAPTER_SCHEMA_VERSION,
};
use core_api::{
    ActorIdentity, AppCommand, AppCommandEnvelope, AppEvent, AppEventEnvelope, AppQuery,
    AppQueryEnvelope, AppResponse, AppResponseEnvelope, ClientContextSnapshot, CommandSource,
    EventStream, RunState, WorkspaceRelativePath, API_VERSION,
};
use headless_json::SdkCapability;
use lsp_runtime::DocumentDiagnostic;
use tokio::sync::{mpsc, Mutex};

use crate::adapter::{
    IdeClientAdapterFactory, METHOD_ATTACH, METHOD_COMMAND, METHOD_DISCONNECT, METHOD_QUERY,
    METHOD_REATTACH,
};
use crate::contract::{IdeCapability, IdeEvent, IdeRequest, IDE_PROTOCOL, IDE_PROTOCOL_VERSION};
use crate::diagnostics::{DiagnosticBoard, IdeDiagnosticSet};
use crate::error::IdeAdapterError;
use crate::lifecycle::{EditorContext, EditorLifecycleEvent, IdeLifecycle, LifecycleMapper};
use crate::lsp_output::LspResultProvider;
use crate::sdk_channel::{PaworkSdkChannel, SdkChannel};

/// 连接 `pawork` Host 的选项。
#[derive(Clone, Debug)]
pub struct IdeHostOptions {
    /// SDK/Headless 进程选项（`binary` / `args` / `env` / 握手身份）。
    pub sdk: PaworkOptions,
    /// 扩展契约协议版本。
    pub protocol_version: String,
    /// 握手与命令身份声明的客户端名。
    pub client_name: String,
    pub client_version: String,
    /// 请求的 IDE 能力（Host 授予的 SDK 能力不足时 fail-closed）。
    pub capabilities: Vec<IdeCapability>,
    /// 事件总线容量。
    pub event_capacity: usize,
}

impl Default for IdeHostOptions {
    fn default() -> Self {
        Self {
            sdk: PaworkOptions {
                client_name: "ide-host".into(),
                client_version: "0.0.0".into(),
                capabilities: vec![
                    SdkCapability::Sessions,
                    SdkCapability::Runs,
                    SdkCapability::Streaming,
                ],
                ..PaworkOptions::default()
            },
            protocol_version: IDE_PROTOCOL_VERSION.into(),
            client_name: "ide-host".into(),
            client_version: "0.0.0".into(),
            capabilities: vec![
                IdeCapability::Lifecycle,
                IdeCapability::Diagnostics,
                IdeCapability::Interaction,
                IdeCapability::Reconnect,
            ],
            event_capacity: 256,
        }
    }
}

/// IDE Host Adapter 连接器。
pub struct IdeHostAdapter {
    options: IdeHostOptions,
    requested: Vec<IdeCapability>,
    negotiated: CapabilitySnapshot,
    adapter: Arc<dyn ClientAdapter>,
    registry: SessionRegistry,
    channel: Mutex<Box<dyn SdkChannel>>,
    bus: mpsc::Sender<IdeEvent>,
    receiver: std::sync::Mutex<Option<mpsc::Receiver<IdeEvent>>>,
    context: Mutex<EditorContext>,
    board: Mutex<DiagnosticBoard>,
    /// 生命周期/诊断快照的构建与发送单飞，保证 revision 与通道写入顺序一致。
    context_sync: Mutex<()>,
    runs: Arc<Mutex<HashMap<RunId, SessionId>>>,
    subscribed: Arc<Mutex<Vec<EventStream>>>,
    /// 转发任务代际：重连/关闭时递增并取消旧代际任务。
    generation: AtomicU64,
    /// 当前代际的订阅转发任务句柄（旧代际在重连/关闭时取消，防泄漏）。
    forward_tasks: Mutex<Vec<(u64, tokio::task::JoinHandle<()>)>>,
    sessions: Mutex<Vec<ClientSessionId>>,
    /// 最近激活/打开的 Core session；IDE 生命周期与诊断快照绑定到该 session。
    active_session: Mutex<Option<SessionId>>,
    context_revision: AtomicU64,
    ownership: Mutex<HashMap<ClientSessionId, (u64, u64)>>,
    lsp: Mutex<Option<Arc<dyn LspResultProvider>>>,
    spawn_options: Option<PaworkOptions>,
    connection_id: ConnectionId,
    next_id: AtomicU64,
    lifecycle: Arc<dyn IdeLifecycle>,
}

impl IdeHostAdapter {
    /// 启动 `pawork headless --json-stdio` 并建立连接（真实进程入口）。
    pub async fn connect(options: IdeHostOptions) -> Result<Self, IdeAdapterError> {
        let spawn_options = options.sdk.clone();
        let client = PaworkClient::spawn(options.sdk.clone()).await?;
        Self::create_inner(
            options,
            Box::new(PaworkSdkChannel::new(client)),
            Some(spawn_options),
        )
        .await
    }

    /// 从任意 [`SdkChannel`] 建立连接（mock/测试入口；不支持进程重连）。
    pub async fn create(
        options: IdeHostOptions,
        channel: Box<dyn SdkChannel>,
    ) -> Result<Self, IdeAdapterError> {
        Self::create_inner(options, channel, None).await
    }

    async fn create_inner(
        options: IdeHostOptions,
        channel: Box<dyn SdkChannel>,
        spawn_options: Option<PaworkOptions>,
    ) -> Result<Self, IdeAdapterError> {
        let requested = options.capabilities.clone();
        let snapshot = negotiate_snapshot(&options)?;
        let factory = IdeClientAdapterFactory::new();
        let adapter = factory.create(snapshot.clone())?;

        // Host 能力核对：IDE 能力 → 所需 SDK 能力 → Host 握手授予，fail-closed。
        let granted = channel.capabilities().await;
        for need in required_sdk(&requested) {
            if !granted.contains(&need) {
                return Err(IdeAdapterError::HostUnavailable(format!(
                    "host did not grant SDK capability {need:?}"
                )));
            }
        }

        let (bus, receiver) = mpsc::channel(options.event_capacity.max(1));
        let instance_id = channel.instance_id().await;
        let registry = SessionRegistry::new(Arc::new(InMemorySessionRegistryStore::default()))
            .await
            .map_err(IdeAdapterError::Adapter)?;
        let connection_id = ConnectionId::new(format!("ide:{}", std::process::id()));

        let adapter = Self {
            options,
            requested: requested.clone(),
            negotiated: snapshot,
            adapter,
            registry,
            channel: Mutex::new(channel),
            bus,
            receiver: std::sync::Mutex::new(Some(receiver)),
            context: Mutex::new(EditorContext::new()),
            board: Mutex::new(DiagnosticBoard::new()),
            context_sync: Mutex::new(()),
            runs: Arc::new(Mutex::new(HashMap::new())),
            subscribed: Arc::new(Mutex::new(Vec::new())),
            generation: AtomicU64::new(1),
            forward_tasks: Mutex::new(Vec::new()),
            sessions: Mutex::new(Vec::new()),
            active_session: Mutex::new(None),
            context_revision: AtomicU64::new(0),
            ownership: Mutex::new(HashMap::new()),
            lsp: Mutex::new(None),
            spawn_options,
            connection_id,
            next_id: AtomicU64::new(1),
            lifecycle: Arc::new(LifecycleMapper),
        };
        adapter
            .bus
            .send(IdeEvent::Ready {
                protocol_version: adapter.options.protocol_version.clone(),
                negotiated: requested,
                instance_id,
            })
            .await
            .map_err(|_| IdeAdapterError::EventBusClosed)?;
        Ok(adapter)
    }

    /// 取出事件总线接收端（一次）。
    pub fn take_events(&self) -> Option<mpsc::Receiver<IdeEvent>> {
        self.receiver.lock().unwrap().take()
    }

    /// 注入可选 LSP 输出结果提供方（宿主侧用 `lsp-runtime::LanguageClient` 实现）。
    pub async fn set_lsp_provider(&self, provider: Arc<dyn LspResultProvider>) {
        *self.lsp.lock().await = Some(provider);
    }

    /// 宿主把 P17-4 LSP Client 聚合快照注入诊断看板，并向事件总线发出
    /// DiagnosticsChanged（IDE 展示）；返回发生变化的文档数。
    ///
    /// 只映射与记录，不写文件、不绕过 Policy；相同快照幂等，不重复发事件。
    pub async fn publish_lsp_snapshot(
        &self,
        snapshots: &[DocumentDiagnostic],
    ) -> Result<usize, IdeAdapterError> {
        let events = self.board.lock().await.apply_lsp_snapshot(snapshots);
        let changed = events.len();
        for event in events {
            self.bus
                .send(event)
                .await
                .map_err(|_| IdeAdapterError::EventBusClosed)?;
        }
        if changed > 0 {
            self.sync_client_context().await?;
        }
        Ok(changed)
    }

    /// 诊断看板当前全量快照（只读观察）。
    pub async fn diagnostic_snapshot(&self) -> Vec<IdeDiagnosticSet> {
        self.board.lock().await.snapshot()
    }

    /// 当前转发任务代际（重连/关闭时 +1；供宿主观察旧代际已取消）。
    pub fn connection_generation(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }

    /// 已记录的订阅流（幂等去重；供宿主观察重连不会翻倍订阅）。
    pub async fn subscribed_streams(&self) -> Vec<EventStream> {
        self.subscribed.lock().await.clone()
    }

    pub fn negotiated_capabilities(&self) -> Vec<IdeCapability> {
        self.requested.clone()
    }

    pub async fn instance_id(&self) -> Option<String> {
        self.channel.lock().await.instance_id().await
    }

    pub async fn is_connected(&self) -> bool {
        self.channel.lock().await.is_open()
    }

    /// registry 观察点（客户端会话绑定状态）。
    pub async fn session(
        &self,
        client_session_id: &ClientSessionId,
    ) -> Option<ClientSessionRecord> {
        self.registry.get(client_session_id).await
    }

    // ---------- 扩展契约入口 ----------

    /// 处理一个扩展请求，返回立即产生的事件（Core 流事件走事件总线）。
    pub async fn handle_request(
        &self,
        request: IdeRequest,
    ) -> Result<Vec<IdeEvent>, IdeAdapterError> {
        match request {
            IdeRequest::Hello {
                client_name,
                client_version,
                protocol_version,
                capabilities,
            } => {
                if client_name.trim().is_empty() || client_version.trim().is_empty() {
                    return Err(IdeAdapterError::InvalidFrame(
                        "client_name and client_version must be non-empty".into(),
                    ));
                }
                if protocol_version != IDE_PROTOCOL_VERSION {
                    return Err(IdeAdapterError::InvalidFrame(format!(
                        "protocol version {protocol_version} is unsupported (expected {IDE_PROTOCOL_VERSION})"
                    )));
                }
                for capability in &capabilities {
                    if !self.requested.contains(capability) {
                        return Err(IdeAdapterError::CapabilityUnsupported(
                            ClientCapability::new(capability.as_str()),
                        ));
                    }
                }
                Ok(vec![IdeEvent::Ready {
                    protocol_version,
                    negotiated: capabilities,
                    instance_id: self.instance_id().await,
                }])
            }
            IdeRequest::EditorDidOpen {
                document_uri,
                language_id,
                text,
            } => {
                self.apply_lifecycle(EditorLifecycleEvent::DocumentOpened {
                    uri: document_uri,
                    language_id,
                    text,
                })
                .await
            }
            IdeRequest::EditorDidClose { document_uri } => {
                self.apply_lifecycle(EditorLifecycleEvent::DocumentClosed { uri: document_uri })
                    .await
            }
            IdeRequest::EditorDidActivate { document_uri } => {
                self.apply_lifecycle(EditorLifecycleEvent::DocumentActivated { uri: document_uri })
                    .await
            }
            IdeRequest::EditorDidChangeSelection {
                document_uri,
                selection,
            } => {
                self.apply_lifecycle(EditorLifecycleEvent::SelectionChanged {
                    uri: document_uri,
                    selection,
                })
                .await
            }
            IdeRequest::EditorDidChangeVisibleRange {
                document_uri,
                range,
            } => {
                self.apply_lifecycle(EditorLifecycleEvent::VisibleRangeChanged {
                    uri: document_uri,
                    range,
                })
                .await
            }
            IdeRequest::EditorDidSave { document_uri } => {
                self.apply_lifecycle(EditorLifecycleEvent::DocumentSaved { uri: document_uri })
                    .await
            }
            IdeRequest::DiagnosticsPublish {
                document_uri,
                version,
                diagnostics,
            } => {
                let set = IdeDiagnosticSet {
                    document_uri,
                    version,
                    diagnostics,
                };
                let events = self.board.lock().await.apply_ide_publish(set);
                if !events.is_empty() {
                    self.sync_client_context().await?;
                }
                Ok(events)
            }
            IdeRequest::LspQuery { query_id, query } => {
                let provider = self.lsp.lock().await.clone().ok_or_else(|| {
                    IdeAdapterError::LspProvider("no LspResultProvider configured".into())
                })?;
                let result = provider
                    .resolve(&query)
                    .await
                    .map_err(IdeAdapterError::LspProvider)?;
                Ok(vec![IdeEvent::LspResult { query_id, result }])
            }
            other => {
                let canonical = self.to_canonical(other)?;
                self.handle_canonical(canonical).await
            }
        }
    }

    /// 原始 `ClientFrame` 入口（复用 `IdeClientAdapter` 协议翻译层）。
    pub async fn handle_client_frame(
        &self,
        frame: ClientFrame,
    ) -> Result<Vec<IdeEvent>, IdeAdapterError> {
        let canonical = self.adapter.decode(frame).await?;
        self.dispatch(canonical).await
    }

    async fn apply_lifecycle(
        &self,
        event: EditorLifecycleEvent,
    ) -> Result<Vec<IdeEvent>, IdeAdapterError> {
        let mut context = self.context.lock().await;
        self.lifecycle.apply(&event, &mut context);
        let changed = context.context_changed_event();
        drop(context);
        self.sync_client_context().await?;
        Ok(vec![changed])
    }

    /// 把当前 IDE 生命周期 + 诊断状态以全量快照推给 Core。未绑定 session 时
    /// 保留本地状态，下一次 attach 会立即同步；失败不会回退 revision，避免
    /// 重试把旧快照覆盖新快照。
    async fn sync_client_context(&self) -> Result<(), IdeAdapterError> {
        let _sync = self.context_sync.lock().await;
        let Some(session_id) = self.active_session.lock().await.clone() else {
            return Ok(());
        };
        let (active_document, open_documents) = {
            let context = self.context.lock().await;
            (
                context.active_uri().map(ToOwned::to_owned),
                context.client_documents(),
            )
        };
        let diagnostics = self.board.lock().await.client_diagnostics();
        // 空观察无需占用协议和 prompt 预算；一旦 IDE 有文档或诊断，attach/
        // reconnect 会立即发送最新全量快照。
        if active_document.is_none() && open_documents.is_empty() && diagnostics.is_empty() {
            return Ok(());
        }
        // P17-9 审查阻塞：先验证上限/URI scheme，失败不污染 revision（超限回滚）。
        // context_sync 单飞锁保证此刻无并发修改 context_revision。
        let next_revision = self.context_revision.load(Ordering::SeqCst) + 1;
        let snapshot = ClientContextSnapshot {
            revision: next_revision,
            active_document,
            open_documents,
            diagnostics,
        };
        snapshot.validate().map_err(IdeAdapterError::InvalidFrame)?;
        let revision = self.context_revision.fetch_add(1, Ordering::SeqCst) + 1;
        debug_assert_eq!(
            revision, next_revision,
            "context_sync single-flight guarantees no concurrent revision bump"
        );
        let response = self
            .channel
            .lock()
            .await
            .command(AppCommand::SessionClientContextReplace {
                session_id,
                snapshot,
            })
            .await?;
        data_of(&response)?;
        Ok(())
    }

    // ---------- canonical 翻译与分发 ----------

    fn to_canonical(&self, request: IdeRequest) -> Result<CanonicalClientRequest, IdeAdapterError> {
        Ok(match request {
            IdeRequest::WorkspaceAdd { root_path } => CanonicalClientRequest::Command(
                self.command_envelope(AppCommand::WorkspaceAdd { root_path }),
            ),
            IdeRequest::SessionCreate {
                workspace_id,
                title,
            } => {
                CanonicalClientRequest::Command(self.command_envelope(AppCommand::SessionCreate {
                    workspace_id,
                    title,
                }))
            }
            IdeRequest::SessionOpen { session_id } => CanonicalClientRequest::Command(
                self.command_envelope(AppCommand::SessionOpen { session_id }),
            ),
            IdeRequest::SessionReattach {
                client_session_id,
                ownership_epoch,
                revision,
            } => CanonicalClientRequest::Reattach {
                client_session_id,
                ownership_epoch,
                revision,
                connection_id: self.connection_id.clone(),
                state: ClientSessionState::Loaded,
                updated_at: now(),
            },
            IdeRequest::RunStart {
                session_id,
                user_message,
                model,
            } => CanonicalClientRequest::Command(self.command_envelope(AppCommand::RunStart {
                session_id,
                user_message,
                model,
                profile: None,
            })),
            IdeRequest::RunCancel { run_id } => CanonicalClientRequest::Command(
                self.command_envelope(AppCommand::RunCancel { run_id }),
            ),
            IdeRequest::RunStatus { run_id } => {
                CanonicalClientRequest::Query(self.query_envelope(AppQuery::RunStatus { run_id }))
            }
            IdeRequest::RunTool {
                run_id,
                tool_name,
                input,
            } => CanonicalClientRequest::Command(self.command_envelope(AppCommand::RunTool {
                run_id,
                tool_name,
                input,
            })),
            IdeRequest::ToolApprove {
                run_id,
                tool_call_id,
                decision,
            } => CanonicalClientRequest::Command(self.command_envelope(AppCommand::ToolApprove {
                run_id,
                tool_call_id,
                decision,
            })),
            IdeRequest::DiffList { workspace_id } => CanonicalClientRequest::Query(
                self.query_envelope(AppQuery::DiffListFiles { workspace_id }),
            ),
            IdeRequest::DiffGet {
                workspace_id,
                path,
                cursor,
            } => {
                let path = WorkspaceRelativePath::new(path)
                    .map_err(|error| IdeAdapterError::InvalidFrame(error.to_string()))?;
                CanonicalClientRequest::Query(self.query_envelope(AppQuery::DiffGet {
                    workspace_id,
                    path,
                    cursor,
                }))
            }
            IdeRequest::Disconnect {
                client_session_id,
                ownership_epoch,
                revision,
            } => CanonicalClientRequest::Disconnect {
                client_session_id,
                ownership_epoch,
                revision,
                updated_at: now(),
            },
            other => {
                return Err(IdeAdapterError::ProtocolUnsupported(format!(
                    "ide request {other:?} is not dispatchable"
                )))
            }
        })
    }

    async fn handle_canonical(
        &self,
        canonical: CanonicalClientRequest,
    ) -> Result<Vec<IdeEvent>, IdeAdapterError> {
        // 复用 IdeClientAdapter 的 ClientFrame ↔ canonical 翻译层做一致性校验。
        let frame = self.to_client_frame(canonical)?;
        let canonical = self.adapter.decode(frame).await?;
        self.dispatch(canonical).await
    }

    fn to_client_frame(
        &self,
        canonical: CanonicalClientRequest,
    ) -> Result<ClientFrame, IdeAdapterError> {
        let method = match &canonical {
            CanonicalClientRequest::Command(_) => METHOD_COMMAND,
            CanonicalClientRequest::Query(_) => METHOD_QUERY,
            CanonicalClientRequest::Attach(_) => METHOD_ATTACH,
            CanonicalClientRequest::Reattach { .. } => METHOD_REATTACH,
            CanonicalClientRequest::Disconnect { .. } => METHOD_DISCONNECT,
        };
        Ok(ClientFrame {
            schema_version: CLIENT_ADAPTER_SCHEMA_VERSION,
            request_id: self.next_request_id(),
            method: method.into(),
            payload: serde_json::to_value(&canonical)
                .map_err(|error| IdeAdapterError::InvalidFrame(error.to_string()))?,
            extensions: BTreeMap::new(),
        })
    }

    async fn dispatch(
        &self,
        request: CanonicalClientRequest,
    ) -> Result<Vec<IdeEvent>, IdeAdapterError> {
        match request {
            CanonicalClientRequest::Command(envelope) => {
                self.dispatch_command(envelope.command).await
            }
            CanonicalClientRequest::Query(envelope) => self.dispatch_query(envelope.query).await,
            CanonicalClientRequest::Attach(record) => {
                // 复用 attach_session：registry + 订阅 + active_session +
                // 上下文同步 + SessionState。已绑定的 session 直接返回当前记录，
                // 避免二次 register 冲突。
                let event = self.attach_session(record.core_session_id).await?;
                Ok(vec![event])
            }
            CanonicalClientRequest::Reattach {
                client_session_id,
                ownership_epoch,
                revision,
                connection_id,
                state,
                updated_at,
            } => {
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
                self.ownership.lock().await.insert(
                    client_session_id.clone(),
                    (record.ownership_epoch, record.revision),
                );
                Ok(vec![IdeEvent::SessionState {
                    client_session_id,
                    core_session_id: record.core_session_id,
                    state: record.state,
                    revision: record.revision,
                }])
            }
            CanonicalClientRequest::Disconnect {
                client_session_id,
                ownership_epoch,
                revision,
                updated_at: _,
            } => {
                self.registry
                    .remove(&client_session_id, ownership_epoch, revision)
                    .await?;
                self.sessions
                    .lock()
                    .await
                    .retain(|id| id != &client_session_id);
                self.ownership.lock().await.remove(&client_session_id);
                Ok(vec![])
            }
        }
    }

    async fn dispatch_command(
        &self,
        command: AppCommand,
    ) -> Result<Vec<IdeEvent>, IdeAdapterError> {
        match command {
            AppCommand::WorkspaceAdd { root_path } => {
                let envelope = self
                    .channel
                    .lock()
                    .await
                    .command(AppCommand::WorkspaceAdd { root_path })
                    .await?;
                let data = data_of(&envelope)?;
                let workspace_id: WorkspaceId = serde_json::from_value(
                    data.get("id").cloned().unwrap_or(serde_json::Value::Null),
                )
                .map_err(|error| IdeAdapterError::InvalidFrame(error.to_string()))?;
                Ok(vec![IdeEvent::WorkspaceAdded { workspace_id }])
            }
            AppCommand::SessionCreate {
                workspace_id,
                title,
            } => {
                let view = self
                    .channel
                    .lock()
                    .await
                    .create_session(workspace_id, title)
                    .await?;
                let event = self.attach_session(view.session_id).await?;
                Ok(vec![event])
            }
            AppCommand::SessionOpen { session_id } => {
                let view = self
                    .channel
                    .lock()
                    .await
                    .open_session(session_id.clone())
                    .await?;
                let event = self.attach_session(view.session_id).await?;
                Ok(vec![event])
            }
            AppCommand::RunStart {
                session_id,
                user_message,
                model,
                ..
            } => {
                let view = self
                    .channel
                    .lock()
                    .await
                    .run_start(session_id.clone(), user_message, model)
                    .await?;
                let client_session_id =
                    ClientSessionId::new(format!("ide:{}", session_id.as_str()));
                let ownership = self.ownership.lock().await.get(&client_session_id).copied();
                if let Some((epoch, revision)) = ownership {
                    let record = self
                        .registry
                        .transition(
                            &client_session_id,
                            epoch,
                            revision,
                            ClientSessionState::Executing,
                            now(),
                        )
                        .await?;
                    self.ownership
                        .lock()
                        .await
                        .insert(client_session_id, (record.ownership_epoch, record.revision));
                }
                self.runs
                    .lock()
                    .await
                    .insert(view.run_id.clone(), session_id);
                self.subscribe_stream(EventStream::Run(view.run_id.clone()))
                    .await?;
                Ok(vec![IdeEvent::RunChanged {
                    run_id: view.run_id,
                    state: view.state,
                }])
            }
            AppCommand::RunCancel { run_id } => {
                let outcome = self.channel.lock().await.cancel(run_id.clone()).await?;
                let mut events = Vec::new();
                if outcome.cancelled || outcome.already_cancelled {
                    self.forget_run(&run_id).await;
                    events.push(IdeEvent::RunChanged {
                        run_id,
                        state: RunState::Cancelled,
                    });
                }
                Ok(events)
            }
            AppCommand::RunTool {
                run_id,
                tool_name,
                input,
            } => {
                self.channel
                    .lock()
                    .await
                    .command(AppCommand::RunTool {
                        run_id,
                        tool_name,
                        input,
                    })
                    .await?;
                Ok(vec![])
            }
            AppCommand::ToolApprove {
                run_id,
                tool_call_id,
                decision,
            } => {
                self.channel
                    .lock()
                    .await
                    .command(AppCommand::ToolApprove {
                        run_id,
                        tool_call_id,
                        decision,
                    })
                    .await?;
                Ok(vec![])
            }
            other => Err(IdeAdapterError::ProtocolUnsupported(format!(
                "app command {other:?} is not part of the IDE adapter subset"
            ))),
        }
    }

    /// 绑定 client session 到 registry + 订阅 session 流 + 上报状态事件。
    async fn attach_session(&self, session_id: SessionId) -> Result<IdeEvent, IdeAdapterError> {
        let client_session_id = ClientSessionId::new(format!("ide:{}", session_id.as_str()));
        if let Some(existing) = self.registry.get(&client_session_id).await {
            *self.active_session.lock().await = Some(session_id.clone());
            self.sync_client_context().await?;
            return Ok(IdeEvent::SessionState {
                client_session_id,
                core_session_id: session_id,
                state: existing.state,
                revision: existing.revision,
            });
        }
        let record = ClientSessionRecord {
            schema_version: CLIENT_ADAPTER_SCHEMA_VERSION,
            protocol: ClientProtocol::new(IDE_PROTOCOL),
            client_session_id: client_session_id.clone(),
            core_session_id: session_id.clone(),
            connection_id: self.connection_id.clone(),
            ownership_epoch: 1,
            revision: 1,
            state: ClientSessionState::Loaded,
            capabilities: self.negotiated.clone(),
            updated_at: now(),
        };
        self.registry.register(record).await?;
        self.sessions.lock().await.push(client_session_id.clone());
        self.ownership
            .lock()
            .await
            .insert(client_session_id.clone(), (1, 1));
        self.subscribe_stream(EventStream::Session(session_id.clone()))
            .await?;
        *self.active_session.lock().await = Some(session_id.clone());
        self.sync_client_context().await?;
        let record = self
            .registry
            .transition(
                &client_session_id,
                1,
                1,
                ClientSessionState::Subscribed,
                now(),
            )
            .await?;
        self.ownership.lock().await.insert(
            client_session_id.clone(),
            (record.ownership_epoch, record.revision),
        );
        Ok(IdeEvent::SessionState {
            client_session_id,
            core_session_id: session_id,
            state: record.state,
            revision: record.revision,
        })
    }

    async fn dispatch_query(&self, query: AppQuery) -> Result<Vec<IdeEvent>, IdeAdapterError> {
        match query {
            AppQuery::RunStatus { run_id } => {
                let view = self.channel.lock().await.run_status(run_id.clone()).await?;
                Ok(vec![IdeEvent::RunChanged {
                    run_id,
                    state: view.state,
                }])
            }
            AppQuery::DiffListFiles { workspace_id } => {
                let envelope = self
                    .channel
                    .lock()
                    .await
                    .query(AppQuery::DiffListFiles {
                        workspace_id: workspace_id.clone(),
                    })
                    .await?;
                let payload = data_of(&envelope)?;
                Ok(vec![IdeEvent::DiffResult {
                    workspace_id,
                    payload,
                }])
            }
            AppQuery::DiffGet {
                workspace_id,
                path,
                cursor,
            } => {
                let path_text = path.as_str().to_string();
                let envelope = self
                    .channel
                    .lock()
                    .await
                    .query(AppQuery::DiffGet {
                        workspace_id: workspace_id.clone(),
                        path,
                        cursor,
                    })
                    .await?;
                let payload = data_of(&envelope)?;
                Ok(vec![IdeEvent::DiffContent {
                    workspace_id,
                    path: path_text,
                    payload,
                }])
            }
            other => Err(IdeAdapterError::ProtocolUnsupported(format!(
                "app query {other:?} is not part of the IDE adapter subset"
            ))),
        }
    }

    // ---------- 事件订阅与重连 ----------

    async fn subscribe_stream(&self, stream: EventStream) -> Result<(), IdeAdapterError> {
        let channel = self.channel.lock().await;
        if !channel.is_open() {
            return Err(IdeAdapterError::NotConnected);
        }
        let subscription = channel
            .subscribe(stream.clone(), self.options.event_capacity)
            .await?;
        let generation = self.generation.load(Ordering::SeqCst);
        {
            // 幂等保存：同一 EventStream 只记录一次，重连重订阅不翻倍。
            let mut subscribed = self.subscribed.lock().await;
            if !subscribed.contains(&stream) {
                subscribed.push(stream);
            }
        }
        let handle = tokio::spawn(forward_task(
            subscription,
            self.bus.clone(),
            self.runs.clone(),
            self.subscribed.clone(),
        ));
        // 与重连的代际替换共享通道锁顺序：任务登记要么发生在代际递增前
        // （随后被取消），要么发生在换通道后（属新代际、保留）。
        let mut tasks = self.forward_tasks.lock().await;
        tasks.retain(|(_, handle)| !handle.is_finished());
        tasks.push((generation, handle));
        Ok(())
    }

    async fn forget_run(&self, run_id: &RunId) {
        forget_run_locked(&self.runs, &self.subscribed, run_id).await;
    }

    /// 断线重连：重新 spawn `pawork`，按 ownership epoch/revision 重挂所有
    /// client session，重订阅流并发出 `ConnectionRestored`。
    pub async fn reconnect(&self) -> Result<(), IdeAdapterError> {
        let spawn_options = self
            .spawn_options
            .clone()
            .ok_or(IdeAdapterError::NotConnected)?;
        let client = PaworkClient::spawn(spawn_options).await?;
        self.reattach_with(Box::new(PaworkSdkChannel::new(client)))
            .await
    }

    /// 用新通道重挂（mock/测试与进程重连共用路径）。
    pub async fn reattach_with(&self, channel: Box<dyn SdkChannel>) -> Result<(), IdeAdapterError> {
        let granted = channel.capabilities().await;
        for need in required_sdk(&self.requested) {
            if !granted.contains(&need) {
                return Err(IdeAdapterError::HostUnavailable(format!(
                    "host did not grant SDK capability {need:?}"
                )));
            }
        }
        {
            // 可观测：旧连接先宣告丢失（ConnectionLost 先于 SessionState
            // 与 ConnectionRestored）。
            let current = self.channel.lock().await;
            let reason = if current.is_open() {
                "channel replaced for reconnect"
            } else {
                "connection lost"
            };
            self.bus
                .send(IdeEvent::ConnectionLost {
                    reason: reason.into(),
                })
                .await
                .map_err(|_| IdeAdapterError::EventBusClosed)?;
            // 代际替换：递增 generation 并取消旧代际转发任务，旧订阅不再
            // 向总线转发重复事件；随后换入新通道。
            let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
            let stale: Vec<(u64, tokio::task::JoinHandle<()>)> =
                std::mem::take(&mut *self.forward_tasks.lock().await);
            for (task_generation, handle) in stale {
                if task_generation != generation {
                    handle.abort();
                }
            }
            drop(current);
            *self.channel.lock().await = channel;
        }

        let sessions = self.sessions.lock().await.clone();
        for client_session_id in sessions {
            let (epoch, revision) = self
                .ownership
                .lock()
                .await
                .get(&client_session_id)
                .copied()
                .unwrap_or((0, 0));
            let record = self
                .registry
                .claim(
                    &client_session_id,
                    epoch,
                    revision,
                    self.connection_id.clone(),
                    ClientSessionState::Loaded,
                    now(),
                )
                .await?;
            self.ownership.lock().await.insert(
                client_session_id.clone(),
                (record.ownership_epoch, record.revision),
            );
            let view = self
                .channel
                .lock()
                .await
                .open_session(record.core_session_id.clone())
                .await?;
            self.bus
                .send(IdeEvent::SessionState {
                    client_session_id,
                    core_session_id: view.session_id.clone(),
                    state: ClientSessionState::Loaded,
                    revision: record.revision,
                })
                .await
                .map_err(|_| IdeAdapterError::EventBusClosed)?;
            *self.active_session.lock().await = Some(view.session_id);
        }

        self.sync_client_context().await?;

        let streams = self.subscribed.lock().await.clone();
        for stream in streams {
            self.subscribe_stream(stream).await?;
        }
        let instance_id = self.channel.lock().await.instance_id().await;
        self.bus
            .send(IdeEvent::ConnectionRestored { instance_id })
            .await
            .map_err(|_| IdeAdapterError::EventBusClosed)?;
        Ok(())
    }

    /// 关闭：按 ownership 移除 registry 记录并关闭 SDK 通道。
    pub async fn close(&self) -> Result<(), IdeAdapterError> {
        let sessions = self.sessions.lock().await.clone();
        for client_session_id in sessions {
            let (epoch, revision) = self
                .ownership
                .lock()
                .await
                .get(&client_session_id)
                .copied()
                .unwrap_or((0, 0));
            // 拆除期尽力而为：记录已不存在时忽略。
            let _ = self
                .registry
                .remove(&client_session_id, epoch, revision)
                .await;
        }
        self.sessions.lock().await.clear();
        self.ownership.lock().await.clear();
        *self.active_session.lock().await = None;
        {
            // 代际替换：取消全部转发任务，旧订阅不再向总线转发。
            let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
            let stale: Vec<(u64, tokio::task::JoinHandle<()>)> =
                std::mem::take(&mut *self.forward_tasks.lock().await);
            for (task_generation, handle) in stale {
                if task_generation != generation {
                    handle.abort();
                }
            }
        }
        // 可观测：关闭时宣告连接关闭（消费端可能已不在，忽略发送失败）。
        let _ = self
            .bus
            .send(IdeEvent::ConnectionLost {
                reason: "adapter closed".into(),
            })
            .await;
        self.channel.lock().await.close().await?;
        Ok(())
    }

    fn command_envelope(&self, command: AppCommand) -> AppCommandEnvelope {
        AppCommandEnvelope {
            api_version: API_VERSION,
            command_id: CommandId::from(self.next_request_id()),
            source: CommandSource::Automation,
            identity: ActorIdentity::Automation {
                name: self.options.client_name.clone(),
            },
            expected_revision: None,
            idempotency_key: None,
            issued_at: now(),
            command,
        }
    }

    fn query_envelope(&self, query: AppQuery) -> AppQueryEnvelope {
        AppQueryEnvelope {
            api_version: API_VERSION,
            request_id: QueryId::from(self.next_request_id()),
            source: CommandSource::Automation,
            identity: ActorIdentity::Automation {
                name: self.options.client_name.clone(),
            },
            issued_at: now(),
            query,
        }
    }

    fn next_request_id(&self) -> String {
        format!("ide-{:05}", self.next_id.fetch_add(1, Ordering::SeqCst))
    }
}

/// Core 事件 → 契约事件子集映射（`None` = 不在最小契约子集内，显式不转发）。
pub fn map_app_event(envelope: &AppEventEnvelope) -> Option<IdeEvent> {
    match &envelope.payload {
        AppEvent::RunChanged { run_id, state } => Some(IdeEvent::RunChanged {
            run_id: run_id.clone(),
            state: state.clone(),
        }),
        AppEvent::AssistantDelta {
            run_id,
            message_id,
            delta,
        } => Some(IdeEvent::AssistantDelta {
            run_id: run_id.clone(),
            message_id: message_id.clone(),
            delta: delta.clone(),
        }),
        AppEvent::ThinkingDelta {
            run_id,
            message_id,
            delta,
        } => Some(IdeEvent::ThinkingDelta {
            run_id: run_id.clone(),
            message_id: message_id.clone(),
            delta: delta.clone(),
        }),
        AppEvent::ToolStarted {
            run_id,
            tool_call_id,
            name,
        } => Some(IdeEvent::ToolStarted {
            run_id: run_id.clone(),
            tool_call_id: tool_call_id.clone(),
            name: name.clone(),
        }),
        AppEvent::ToolOutput {
            run_id,
            tool_call_id,
            delta,
            truncated,
            ..
        } => Some(IdeEvent::ToolOutput {
            run_id: run_id.clone(),
            tool_call_id: tool_call_id.clone(),
            delta: delta.clone(),
            truncated: *truncated,
        }),
        AppEvent::ToolApprovalRequired {
            run_id,
            tool_call_id,
            reason,
        } => Some(IdeEvent::ToolApprovalRequired {
            run_id: run_id.clone(),
            tool_call_id: tool_call_id.clone(),
            reason: reason.clone(),
        }),
        AppEvent::ToolCompleted {
            run_id,
            tool_call_id,
            success,
        } => Some(IdeEvent::ToolCompleted {
            run_id: run_id.clone(),
            tool_call_id: tool_call_id.clone(),
            success: *success,
        }),
        AppEvent::DiffChanged { workspace_id } => Some(IdeEvent::DiffChanged {
            workspace_id: workspace_id.clone(),
        }),
        _ => None,
    }
}

/// 订阅转发任务：Core 事件 → 契约事件 → 总线。
async fn forward_task(
    mut subscription: EventSubscription,
    bus: mpsc::Sender<IdeEvent>,
    runs: Arc<Mutex<HashMap<RunId, SessionId>>>,
    subscribed: Arc<Mutex<Vec<EventStream>>>,
) {
    while let Ok(envelope) = subscription.next_event().await {
        if let Some(event) = map_app_event(&envelope) {
            if let IdeEvent::RunChanged { run_id, state } = &event {
                if is_terminal_run_state(state) {
                    forget_run_locked(&runs, &subscribed, run_id).await;
                }
            }
            if bus.send(event).await.is_err() {
                break;
            }
        }
    }
}

fn is_terminal_run_state(state: &RunState) -> bool {
    matches!(
        state,
        RunState::Completed | RunState::Cancelled | RunState::Failed | RunState::Interrupted
    )
}

async fn forget_run_locked(
    runs: &Mutex<HashMap<RunId, SessionId>>,
    subscribed: &Mutex<Vec<EventStream>>,
    run_id: &RunId,
) {
    runs.lock().await.remove(run_id);
    subscribed
        .lock()
        .await
        .retain(|stream| stream != &EventStream::Run(run_id.clone()));
}

fn negotiate_snapshot(options: &IdeHostOptions) -> Result<CapabilitySnapshot, IdeAdapterError> {
    let snapshot = CapabilitySnapshot {
        schema_version: CLIENT_ADAPTER_SCHEMA_VERSION,
        protocol: ClientProtocol::new(IDE_PROTOCOL),
        protocol_version: options.protocol_version.clone(),
        client_version: options.client_version.clone(),
        revision: 1,
        capabilities: options
            .capabilities
            .iter()
            .map(|capability| ClientCapability::new(capability.as_str()))
            .collect(),
    };
    snapshot.validate()?;
    Ok(snapshot)
}

fn required_sdk(capabilities: &[IdeCapability]) -> Vec<SdkCapability> {
    let mut out = Vec::new();
    for capability in capabilities {
        for need in capability.requires_sdk() {
            if !out.contains(need) {
                out.push(*need);
            }
        }
    }
    out
}

fn data_of(envelope: &AppResponseEnvelope) -> Result<serde_json::Value, IdeAdapterError> {
    match &envelope.response {
        AppResponse::Data(value) => Ok(value.clone()),
        AppResponse::Error(context) => Err(SdkError::RequestFailed(context.clone()).into()),
        other => Err(SdkError::UnknownResponseType(format!("expected data, got {other:?}")).into()),
    }
}

fn now() -> Timestamp {
    use std::time::{SystemTime, UNIX_EPOCH};
    Timestamp::from_unix_millis(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_millis() as u64)
            .unwrap_or(0),
    )
}
