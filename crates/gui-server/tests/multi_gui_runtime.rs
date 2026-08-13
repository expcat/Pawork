//! P13-5 多 GUI 运行时集成测试（transport-memory）。
//!
//! 覆盖验收：
//! - 一个 CLI/Core 同时连接 ≥3 个 GUI；
//! - CLI 发起的 Run 同步到所有 GUI；
//! - GUI A 发起的 Run 同步到 CLI 与 GUI B；
//! - 任一 GUI 审批同步到其他 GUI 与 CLI；
//! - 断线重连按 last_global_sequence Replay / Snapshot 重建完整状态；
//! - 慢客户端（不消费事件）不阻塞 Agent 或其他 GUI。
//!
//! 事件路径：app-service limiter → drain_events（本测试的 pump 任务模拟
//! CLI 侧 EventPump）→ EventHub.publish → 每连接 forwarder → 有界队列 →
//! 会话帧循环 → 传输层。

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use agent_domain::{
    ActorId, CancellationToken, CommandId, EventId, ProviderId, RunId, SessionId, StopReason,
    Timestamp, TokenUsage, ToolCallId, WorkspaceId,
};
use app_service::AppService;
use async_trait::async_trait;
use client_auth::{Token, TokenAuthenticator, TokenStore};
use connection_manager::ConnectionManager;
use core_api::{
    ActorIdentity, AppCommand, AppCommandEnvelope, AppEvent, AppEventEnvelope, AppResponse,
    ApprovalDecision, CommandSource, EventSource, EventStream, GlobalSequence, RunState,
    API_VERSION,
};
use gui_protocol::{
    decode_server_frame, encode_client_frame, ClientAuthentication, ClientFrame, GuiCapability,
    HandshakeRequest, HandshakeResponse, ResumeRequest, ServerFrame, SubscribeRequest,
};
use provider_api::{
    CanonicalModelRequest, ModelDefinition, ModelProvider, ModelResponseSummary, ProviderError,
    ProviderEventSink, ProviderStreamEvent, ResolvedCredential,
};
use serde_json::{json, Value};
use subscription_hub::EventHub;
use transport_api::{
    ConnectOptions, GuiConnection, GuiListener, GuiTransportClient, GuiTransportServer,
    TransportEndpoint, TransportError, TransportFrame,
};
use transport_memory::MemoryTransport;

const CHANNEL: &str = "multi-gui-runtime";

// ---------------------------------------------------------------------------
// 基础设施：Runtime（app-service + hub + pump + server）与 TestClient
// ---------------------------------------------------------------------------

struct Runtime {
    app_service: Arc<AppService>,
    hub: Arc<EventHub>,
    connections: Arc<ConnectionManager>,
    listener: Arc<dyn GuiListener>,
    client_transport: Arc<dyn GuiTransportClient>,
    token: Token,
    pump: tokio::task::JoinHandle<()>,
    _temp: tempfile::TempDir,
}

impl Runtime {
    /// pump 模拟 CLI 侧 EventPump：限流器 → Hub 发布（Hub 重写 global_sequence）。
    fn spawn_pump(app_service: Arc<AppService>, hub: Arc<EventHub>) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            loop {
                for event in app_service.drain_events() {
                    hub.publish(event);
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
    }

    async fn new(
        hub: Arc<EventHub>,
        server_transport: Arc<dyn GuiTransportServer>,
        client_transport: Arc<dyn GuiTransportClient>,
    ) -> Self {
        Self::new_with(hub, server_transport, client_transport, None).await
    }

    async fn new_with(
        hub: Arc<EventHub>,
        server_transport: Arc<dyn GuiTransportServer>,
        client_transport: Arc<dyn GuiTransportClient>,
        connections: Option<Arc<ConnectionManager>>,
    ) -> Self {
        let app_service = Arc::new(AppService::new("multi-gui-runtime"));
        let pump = Self::spawn_pump(Arc::clone(&app_service), Arc::clone(&hub));
        let temp = tempfile::tempdir().expect("tempdir");
        let token_path = temp.path().join("gui.token");
        let token = TokenStore::new(&token_path)
            .generate()
            .expect("generate token");
        let handshake = gui_protocol::HandshakeService::new(
            agent_domain::CoreInstanceId::from("multi-gui-instance"),
            core_api::SUPPORTED_API_VERSIONS.to_vec(),
            vec![GuiCapability::Events, GuiCapability::Snapshots],
        )
        .with_authenticator(Box::new(TokenAuthenticator::new(TokenStore::new(
            &token_path,
        ))));
        let connections = connections.unwrap_or_else(|| Arc::new(ConnectionManager::default()));
        let server = gui_server::GuiServer::new(gui_server::GuiServerConfig {
            app_service: Arc::clone(&app_service),
            handshake,
            transport: server_transport,
            hub: Arc::clone(&hub),
            connections: Some(Arc::clone(&connections)),
        });
        let listener = server
            .bind(TransportEndpoint::Memory {
                channel: CHANNEL.into(),
            })
            .await
            .expect("bind");
        Runtime {
            app_service,
            hub,
            connections,
            listener: Arc::from(listener),
            client_transport,
            token,
            pump,
            _temp: temp,
        }
    }

    /// 注册 MockProvider（CLI 侧注册，Agent 运行依赖）。
    fn register_provider(&self, provider: Arc<dyn ModelProvider>) -> ProviderId {
        self.app_service.register_provider(provider)
    }

    /// 建 workspace + session（CLI 来源），返回 session_id。
    fn prepare_session(&self) -> SessionId {
        let dir = std::env::temp_dir().join(format!("pawork-multi-gui-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create workspace dir");
        let response = self.app_service.dispatch_envelope(command(
            cli_source(),
            cli_identity(),
            AppCommand::WorkspaceAdd {
                root_path: dir.to_string_lossy().into_owned(),
            },
        ));
        let workspace_id = match &response.response {
            AppResponse::Data(value) => WorkspaceId::from(
                value
                    .get("id")
                    .and_then(serde_json::Value::as_str)
                    .expect("workspace id"),
            ),
            other => panic!("expected workspace data, got {other:?}"),
        };
        let response = self.app_service.dispatch_envelope(command(
            cli_source(),
            cli_identity(),
            AppCommand::SessionCreate {
                workspace_id,
                title: Some("multi-gui".into()),
            },
        ));
        match &response.response {
            AppResponse::Data(value) => SessionId::from(
                value
                    .get("session_id")
                    .and_then(serde_json::Value::as_str)
                    .expect("session id"),
            ),
            other => panic!("expected session data, got {other:?}"),
        }
    }

    /// CLI 发起的 RunStart（同步到所有 GUI）。
    fn start_run_cli(&self, session_id: &SessionId, message: &str) -> RunId {
        self.start_run(cli_source(), session_id, message)
    }

    /// GUI 发起的 RunStart。
    fn start_run_gui(&self, client_id: &str, session_id: &SessionId, message: &str) -> RunId {
        self.start_run(
            CommandSource::LocalGui {
                client_id: agent_domain::GuiClientId::from(client_id),
            },
            session_id,
            message,
        )
    }

    fn start_run(&self, source: CommandSource, session_id: &SessionId, message: &str) -> RunId {
        let response = self.app_service.dispatch_envelope(command(
            source,
            cli_identity(),
            AppCommand::RunStart {
                session_id: session_id.clone(),
                user_message: message.into(),
                model: None,
                profile: None,
            },
        ));
        match &response.response {
            AppResponse::Accepted {
                run_id: Some(run_id),
                ..
            } => run_id.clone(),
            other => panic!("RunStart 应 Accepted 且携带 run id，got {other:?}"),
        }
    }

    /// GUI 发起的审批。
    fn approve_gui(
        &self,
        client_id: &str,
        run_id: &RunId,
        tool_call_id: &ToolCallId,
    ) -> AppResponse {
        let response = self.app_service.dispatch_envelope(command(
            CommandSource::LocalGui {
                client_id: agent_domain::GuiClientId::from(client_id),
            },
            cli_identity(),
            AppCommand::ToolApprove {
                run_id: run_id.clone(),
                tool_call_id: tool_call_id.clone(),
                decision: ApprovalDecision::ApproveOnce,
            },
        ));
        response.response
    }

    /// 注册并 accept 一个新 GUI 连接，返回测试客户端与首帧 Snapshot。
    async fn connect_gui(&self) -> (TestClient, gui_protocol::Snapshot) {
        let listener = Arc::clone(&self.listener);
        let accept = tokio::spawn(async move { listener.accept().await });
        let conn = self
            .client_transport
            .connect(
                TransportEndpoint::Memory {
                    channel: CHANNEL.into(),
                },
                ConnectOptions {
                    timeout_ms: 1_000,
                    client_label: Some("multi-gui-test".into()),
                    max_frame_bytes: 1024 * 1024,
                },
            )
            .await
            .expect("connect");
        let session = accept.await.expect("accept task").expect("accept");
        // 持有宿主侧句柄：drop 会释放 close 通道导致会话任务断线。
        let client = TestClient {
            conn,
            _session: session,
        };
        let (response, snapshot) = client
            .handshake(&self.token)
            .await
            .expect("handshake accepted");
        let _ = response;
        (client, snapshot)
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
        self.pump.abort();
    }
}

struct TestClient {
    conn: Box<dyn GuiConnection>,
    _session: Box<dyn GuiConnection>,
}

impl TestClient {
    async fn send(&self, frame: &ClientFrame) {
        let bytes = encode_client_frame(frame).expect("encode client frame");
        self.conn
            .send(TransportFrame::new(bytes))
            .await
            .expect("send frame");
    }

    async fn recv(&self) -> ServerFrame {
        let bytes = self.conn.receive().await.expect("receive frame");
        decode_server_frame(bytes.as_bytes()).expect("decode server frame")
    }

    /// 握手 + 消费首帧 Snapshot；失败返回错误信息。
    async fn handshake(
        &self,
        token: &Token,
    ) -> Result<(HandshakeResponse, gui_protocol::Snapshot), String> {
        self.send(&ClientFrame::Handshake(HandshakeRequest {
            request_id: "hs".into(),
            client_name: "multi-gui-test".into(),
            client_version: "0.0.1".into(),
            supported_api_versions: vec![API_VERSION],
            capabilities: vec![GuiCapability::Events, GuiCapability::Snapshots],
            authentication: Some(ClientAuthentication {
                scheme: client_auth::TOKEN_SCHEME.into(),
                proof: token.as_str().into(),
            }),
        }))
        .await;
        match self.recv().await {
            ServerFrame::Handshake(response @ HandshakeResponse::Accepted { .. }) => {
                match self.recv().await {
                    ServerFrame::Snapshot(snapshot) => Ok((response, snapshot)),
                    other => Err(format!("expected initial snapshot, got {other:?}")),
                }
            }
            other => Err(format!("expected accepted handshake, got {other:?}")),
        }
    }

    /// 订阅全部事件流（streams 为空 = 全量）。
    async fn subscribe_all(&self) {
        self.send(&ClientFrame::Subscribe(SubscribeRequest {
            request_id: "sub".into(),
            subscription_id: "all".into(),
            streams: vec![],
        }))
        .await;
    }

    /// 在超时内接收事件直到谓词满足，返回 (满足前已收事件, 是否满足)。
    async fn recv_until<F: Fn(&AppEventEnvelope) -> bool>(
        &self,
        predicate: F,
        timeout: Duration,
    ) -> (Vec<AppEventEnvelope>, bool) {
        let deadline = Instant::now() + timeout;
        let mut received = Vec::new();
        while Instant::now() < deadline {
            match tokio::time::timeout(
                deadline.saturating_duration_since(Instant::now()),
                self.recv(),
            )
            .await
            {
                Ok(ServerFrame::Event(envelope)) => {
                    if predicate(&envelope) {
                        received.push(envelope);
                        return (received, true);
                    }
                    received.push(envelope);
                }
                Ok(other) => panic!("unexpected frame while awaiting events: {other:?}"),
                Err(_) => break,
            }
        }
        (received, false)
    }
}

// ---------------------------------------------------------------------------
// 事件/命令构造
// ---------------------------------------------------------------------------

fn cli_source() -> CommandSource {
    CommandSource::LocalCli {
        terminal_session_id: Some("terminal-1".into()),
    }
}

fn cli_identity() -> ActorIdentity {
    ActorIdentity::LocalUser {
        actor_id: ActorId::from("multi-gui-tester"),
        display_name: None,
    }
}

fn command(
    source: CommandSource,
    identity: ActorIdentity,
    command: AppCommand,
) -> AppCommandEnvelope {
    AppCommandEnvelope {
        api_version: API_VERSION,
        command_id: CommandId::from(format!("cmd-{}", next_id())),
        source,
        identity,
        expected_revision: None,
        idempotency_key: None,
        issued_at: Timestamp::from_unix_millis(1),
        command,
    }
}

fn synthetic_event(run_id: &RunId, sequence_hint: u64, state: RunState) -> AppEventEnvelope {
    AppEventEnvelope {
        api_version: API_VERSION,
        instance_id: agent_domain::CoreInstanceId::from("multi-gui-instance"),
        event_id: EventId::from(format!("synth-{sequence_hint}")),
        global_sequence: GlobalSequence(sequence_hint), // Hub 发布时强制重写
        stream: EventStream::Run(run_id.clone()),
        stream_sequence: sequence_hint,
        timestamp: Timestamp::from_unix_millis(sequence_hint),
        source: EventSource::Command {
            command_id: CommandId::from("synth-cmd"),
            source: cli_source(),
        },
        payload: AppEvent::RunChanged {
            run_id: run_id.clone(),
            state,
        },
    }
}

static NEXT_ID: AtomicU64 = AtomicU64::new(0);

fn next_id() -> u64 {
    NEXT_ID.fetch_add(1, Ordering::SeqCst)
}

/// 两轮脚本 Provider：第一轮请求工具 `echo`（触发审批等待），第二轮直接完成。
/// MockScript 逐轮重放同一脚本，无法表达「工具后完成」，故自建（同 run_lifecycle 测试）。
struct TwoTurnProvider {
    id: ProviderId,
    turns: Mutex<u32>,
}

impl TwoTurnProvider {
    fn new(id: ProviderId) -> Self {
        Self {
            id,
            turns: Mutex::new(0),
        }
    }
}

#[async_trait]
impl ModelProvider for TwoTurnProvider {
    fn id(&self) -> ProviderId {
        self.id.clone()
    }

    async fn list_models(
        &self,
        _credential: Option<&ResolvedCredential>,
    ) -> Result<Vec<ModelDefinition>, ProviderError> {
        Ok(Vec::new())
    }

    async fn stream(
        &self,
        _request: CanonicalModelRequest,
        sink: &dyn ProviderEventSink,
        _cancel: CancellationToken,
    ) -> Result<ModelResponseSummary, ProviderError> {
        let turn = {
            let mut turns = self.turns.lock().expect("turns mutex");
            *turns += 1;
            *turns
        };
        let tool_call_id = ToolCallId::from("mock-tool-call-0");
        if turn == 1 {
            sink.emit(ProviderStreamEvent::ToolCallStarted {
                id: tool_call_id.clone(),
                name: "echo".into(),
            })
            .await?;
            sink.emit(ProviderStreamEvent::ToolCallArgumentsDelta {
                id: tool_call_id.clone(),
                json: "{}".into(),
            })
            .await?;
            sink.emit(ProviderStreamEvent::ToolCallCompleted { id: tool_call_id })
                .await?;
            sink.emit(ProviderStreamEvent::ResponseCompleted(StopReason::ToolUse))
                .await?;
        } else {
            sink.emit(ProviderStreamEvent::TextDelta("done".into()))
                .await?;
            sink.emit(ProviderStreamEvent::ResponseCompleted(
                StopReason::Completed,
            ))
            .await?;
        }
        Ok(ModelResponseSummary {
            stop_reason: StopReason::Completed,
            usage: TokenUsage::default(),
            response_id: None,
            provider_metadata: Value::Null,
        })
    }
}

// ---------------------------------------------------------------------------
// 慢客户端模拟：包装 MemoryTransport，服务端 send 在 N 次后永久阻塞
// ---------------------------------------------------------------------------

struct SlowTransport {
    inner: MemoryTransport,
    block_first: Arc<AtomicUsize>,
    block_after: Arc<AtomicUsize>,
    released: Arc<AtomicBool>,
    unblock: Arc<tokio::sync::Notify>,
}

impl SlowTransport {
    /// `block_first` 个连接的 send 在 `block_after` 次后永久阻塞；
    /// 其余连接不受影响（模拟单一慢 GUI）。
    fn new(inner: MemoryTransport, block_first: usize, block_after: usize) -> Self {
        Self {
            inner,
            block_first: Arc::new(AtomicUsize::new(block_first)),
            block_after: Arc::new(AtomicUsize::new(block_after)),
            released: Arc::new(AtomicBool::new(false)),
            unblock: Arc::new(tokio::sync::Notify::new()),
        }
    }

    /// 释放全部阻塞的 send（测试收尾：让慢会话能观察到断线并注销）。
    fn release(&self) {
        self.released.store(true, Ordering::SeqCst);
        self.unblock.notify_waiters();
    }
}

#[async_trait]
impl GuiTransportServer for SlowTransport {
    async fn bind(
        &self,
        endpoint: TransportEndpoint,
    ) -> Result<Box<dyn GuiListener>, TransportError> {
        let listener = self.inner.bind(endpoint).await?;
        Ok(Box::new(SlowListener {
            inner: listener,
            accepted: AtomicUsize::new(0),
            block_first: Arc::clone(&self.block_first),
            block_after: Arc::clone(&self.block_after),
            released: Arc::clone(&self.released),
            unblock: Arc::clone(&self.unblock),
        }))
    }
}

struct SlowListener {
    inner: Box<dyn GuiListener>,
    accepted: AtomicUsize,
    block_first: Arc<AtomicUsize>,
    block_after: Arc<AtomicUsize>,
    released: Arc<AtomicBool>,
    unblock: Arc<tokio::sync::Notify>,
}

#[async_trait]
impl GuiListener for SlowListener {
    async fn accept(&self) -> Result<Box<dyn GuiConnection>, TransportError> {
        let connection = self.inner.accept().await?;
        let index = self.accepted.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(SlowConnection {
            inner: connection,
            sends: AtomicUsize::new(0),
            blocked: index < self.block_first.load(Ordering::SeqCst),
            block_after: Arc::clone(&self.block_after),
            released: Arc::clone(&self.released),
            unblock: Arc::clone(&self.unblock),
        }))
    }

    async fn close(&self) -> Result<(), TransportError> {
        self.inner.close().await
    }
}

/// 前 `block_after` 次 send 正常，之后永久阻塞（模拟 GUI 不消费事件）。
struct SlowConnection {
    inner: Box<dyn GuiConnection>,
    sends: AtomicUsize,
    blocked: bool,
    block_after: Arc<AtomicUsize>,
    released: Arc<AtomicBool>,
    unblock: Arc<tokio::sync::Notify>,
}

#[async_trait]
impl GuiConnection for SlowConnection {
    async fn send(&self, frame: TransportFrame) -> Result<(), TransportError> {
        let send_index = self.sends.fetch_add(1, Ordering::SeqCst);
        if self.blocked
            && !self.released.load(Ordering::SeqCst)
            && send_index >= self.block_after.load(Ordering::SeqCst)
        {
            self.unblock.notified().await;
        }
        self.inner.send(frame).await
    }

    async fn receive(&self) -> Result<TransportFrame, TransportError> {
        self.inner.receive().await
    }

    async fn close(&self) -> Result<(), TransportError> {
        self.inner.close().await
    }

    fn info(&self) -> transport_api::ConnectionInfo {
        self.inner.info()
    }
}

// ---------------------------------------------------------------------------
// 验收测试
// ---------------------------------------------------------------------------

fn mock_provider() -> Arc<dyn ModelProvider> {
    Arc::new(
        test_support::MockProvider::new(
            test_support::MockScript::new()
                .text("hello ")
                .text("world")
                .complete(),
        )
        .with_id(ProviderId::from("mock")),
    )
}

fn run_state(event: &AppEventEnvelope, run_id: &RunId) -> Option<RunState> {
    match &event.payload {
        AppEvent::RunChanged { run_id: id, state } if id == run_id => Some(state.clone()),
        _ => None,
    }
}

/// 验收 1 + 2：一个 CLI/Core 同时连接 3 个 GUI；CLI 发起的 Run 同步到所有 GUI。
#[tokio::test]
async fn three_guis_receive_cli_run_events() {
    let transport = Arc::new(MemoryTransport::new());
    let runtime = Runtime::new(
        Arc::new(EventHub::new()),
        transport.clone(),
        transport.clone(),
    )
    .await;
    runtime.register_provider(mock_provider());
    let session_id = runtime.prepare_session();

    let (gui_a, _) = runtime.connect_gui().await;
    let (gui_b, _) = runtime.connect_gui().await;
    let (gui_c, _) = runtime.connect_gui().await;
    assert_eq!(
        runtime.connections.count(),
        3,
        "一个 CLI/Core 应同时登记 3 个 GUI"
    );
    gui_a.subscribe_all().await;
    gui_b.subscribe_all().await;
    gui_c.subscribe_all().await;

    let run_id = runtime.start_run_cli(&session_id, "hello from cli");

    let (_events_a, a_done) = gui_a
        .recv_until(
            |e| run_state(e, &run_id) == Some(RunState::Completed),
            Duration::from_secs(10),
        )
        .await;
    let (_events_b, b_done) = gui_b
        .recv_until(
            |e| run_state(e, &run_id) == Some(RunState::Completed),
            Duration::from_secs(10),
        )
        .await;
    let (events_c, c_done) = gui_c
        .recv_until(
            |e| run_state(e, &run_id) == Some(RunState::Completed),
            Duration::from_secs(10),
        )
        .await;
    assert!(a_done, "GUI A 应收到 CLI Run 的 Completed");
    assert!(b_done, "GUI B 应收到 CLI Run 的 Completed");
    assert!(c_done, "GUI C 应收到 CLI Run 的 Completed");
    assert!(
        events_c
            .iter()
            .any(|e| e.stream == EventStream::Run(run_id.clone())),
        "GUI C 的事件应属于该 Run 流"
    );

    // CLI 侧：聚合状态同步（同进程事实源）。
    let run = runtime
        .app_service
        .router()
        .aggregate()
        .get_run(
            &run_id,
            &agent_domain::TenantId::new(core_api::DEFAULT_CONTROL_PLANE_TENANT),
        )
        .expect("run");
    assert_eq!(run.state, RunState::Completed);
}

/// 验收 3：GUI A 发起的 Run 同步到 CLI 与 GUI B。
#[tokio::test]
async fn gui_a_run_syncs_to_cli_and_gui_b() {
    let transport = Arc::new(MemoryTransport::new());
    let runtime = Runtime::new(
        Arc::new(EventHub::new()),
        transport.clone(),
        transport.clone(),
    )
    .await;
    runtime.register_provider(mock_provider());
    let session_id = runtime.prepare_session();

    let (gui_a, _) = runtime.connect_gui().await;
    let (gui_b, _) = runtime.connect_gui().await;
    gui_a.subscribe_all().await;
    gui_b.subscribe_all().await;

    // CLI 观察者：直接订阅 Hub（CLI 进程内事件消费面）。
    let mut cli_observer = runtime.hub.subscribe();

    // GUI A 发起 Run。
    let run_id = runtime.start_run_gui("client-0", &session_id, "run from gui a");

    // GUI B 收到 RunChanged（含 Completed）。
    let (events_b, b_done) = gui_b
        .recv_until(
            |e| run_state(e, &run_id) == Some(RunState::Completed),
            Duration::from_secs(10),
        )
        .await;
    assert!(b_done, "GUI B 应收到 GUI A Run 的 Completed");
    assert!(
        events_b.iter().any(|e| matches!(
            &e.source,
            EventSource::Command {
                source: CommandSource::LocalGui { .. },
                ..
            }
        )),
        "事件来源应标记为 LocalGui"
    );

    // CLI 观察者同样收到同一 Run 的完整事件流。
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut cli_saw_completed = false;
    let mut cli_saw_run = false;
    while Instant::now() < deadline {
        match tokio::time::timeout(
            deadline.saturating_duration_since(Instant::now()),
            cli_observer.recv(),
        )
        .await
        {
            Ok(Ok(event)) => {
                if event.stream == EventStream::Run(run_id.clone()) {
                    cli_saw_run = true;
                    if run_state(&event, &run_id) == Some(RunState::Completed) {
                        cli_saw_completed = true;
                        break;
                    }
                }
            }
            Ok(Err(_)) | Err(_) => break,
        }
    }
    assert!(
        cli_saw_run && cli_saw_completed,
        "CLI 观察者应收到 GUI A Run 的 Completed"
    );

    // GUI A 自身也收到事件（不阻塞、无重复投递问题）。
    let (_events_a, a_done) = gui_a
        .recv_until(
            |e| run_state(e, &run_id) == Some(RunState::Completed),
            Duration::from_secs(10),
        )
        .await;
    assert!(a_done, "GUI A 也应收到自己发起的 Run 事件");
}

/// 验收 4：任一 GUI 审批同步到其他 GUI 与 CLI。
#[tokio::test]
async fn gui_b_approval_syncs_to_other_guis_and_cli() {
    let transport = Arc::new(MemoryTransport::new());
    let runtime = Runtime::new(
        Arc::new(EventHub::new()),
        transport.clone(),
        transport.clone(),
    )
    .await;
    runtime.register_provider(Arc::new(TwoTurnProvider::new(ProviderId::from("mock"))));
    let session_id = runtime.prepare_session();

    let (gui_a, _) = runtime.connect_gui().await;
    let (gui_b, _) = runtime.connect_gui().await;
    let (gui_c, _) = runtime.connect_gui().await;
    gui_a.subscribe_all().await;
    gui_b.subscribe_all().await;
    gui_c.subscribe_all().await;
    let mut cli_observer = runtime.hub.subscribe();

    let run_id = runtime.start_run_cli(&session_id, "run with tool");

    // 所有 GUI 看到 ToolApprovalRequired（等待审批）。
    let (_events, saw_required) = gui_a
        .recv_until(
            |e| matches!(&e.payload, AppEvent::ToolApprovalRequired { .. }),
            Duration::from_secs(10),
        )
        .await;
    assert!(saw_required, "GUI A 应收到审批请求事件");

    // 等待 Pending 审批落库（聚合记录），GUI B 审批。
    let deadline = Instant::now() + Duration::from_secs(10);
    let tool_call_id = loop {
        if let Some(approval) = runtime
            .app_service
            .router()
            .aggregate()
            .approvals()
            .into_iter()
            .find(|approval| approval.run_id == run_id)
        {
            break approval.tool_call_id.clone();
        }
        assert!(Instant::now() < deadline, "审批应在 10s 内落库");
        tokio::time::sleep(Duration::from_millis(10)).await;
    };
    let response = runtime.approve_gui("client-1", &run_id, &tool_call_id);
    assert!(matches!(response, AppResponse::Data(_)));

    // 其他 GUI（A/C）与 CLI 观察者收到审批后的执行推进（ExecutingTools → Completed）。
    let (_events_a, a_done) = gui_a
        .recv_until(
            |e| run_state(e, &run_id) == Some(RunState::Completed),
            Duration::from_secs(10),
        )
        .await;
    let (_events_c, c_done) = gui_c
        .recv_until(
            |e| run_state(e, &run_id) == Some(RunState::Completed),
            Duration::from_secs(10),
        )
        .await;
    assert!(a_done, "GUI A 应收到审批后的 Completed");
    assert!(c_done, "GUI C 应收到审批后的 Completed");

    let deadline = Instant::now() + Duration::from_secs(10);
    let mut cli_done = false;
    while Instant::now() < deadline {
        match tokio::time::timeout(
            deadline.saturating_duration_since(Instant::now()),
            cli_observer.recv(),
        )
        .await
        {
            Ok(Ok(event)) => {
                if run_state(&event, &run_id) == Some(RunState::Completed) {
                    cli_done = true;
                    break;
                }
            }
            Ok(Err(_)) | Err(_) => break,
        }
    }
    assert!(cli_done, "CLI 观察者应收到审批后的 Completed");

    // 审批状态在聚合中落地。
    assert_eq!(runtime.app_service.router().approvals().pending_count(), 0);
    assert!(
        runtime
            .app_service
            .router()
            .aggregate()
            .approvals()
            .iter()
            .any(|approval| approval.run_id == run_id
                && approval.status
                    == app_service::ApprovalStatus::Decided(ApprovalDecision::ApproveOnce)),
        "聚合审批记录应为 Decided"
    );
}

/// 验收 5：断线重连——初始 Snapshot 重建状态；Resume 按 last_global_sequence
/// Replay 补齐断线期间缺失事件。
#[tokio::test]
async fn reconnect_replays_missing_events_after_snapshot_rebuild() {
    let transport = Arc::new(MemoryTransport::new());
    let runtime = Runtime::new(
        Arc::new(EventHub::new()),
        transport.clone(),
        transport.clone(),
    )
    .await;
    runtime.register_provider(Arc::new(
        test_support::MockProvider::new(test_support::MockScript::new().wait_for_cancellation())
            .with_id(ProviderId::from("mock")),
    ));
    let session_id = runtime.prepare_session();

    // 第一段连接：订阅并消费事件，记录已确认序列。
    let (gui_old, _) = runtime.connect_gui().await;
    gui_old.subscribe_all().await;
    let run_id = runtime.start_run_cli(&session_id, "long run");
    let (events, saw_started) = gui_old
        .recv_until(
            |e| run_state(e, &run_id) == Some(RunState::StreamingResponse),
            Duration::from_secs(10),
        )
        .await;
    assert!(saw_started, "第一段连接应进入 StreamingResponse");
    let last_sequence = events
        .iter()
        .map(|e| e.global_sequence.0)
        .max()
        .expect("至少一个事件");
    gui_old
        .send(&ClientFrame::Ack {
            global_sequence: GlobalSequence(last_sequence),
        })
        .await;
    gui_old.conn.close().await.expect("close");

    // 断线后 Run 继续存活（Agent 不被 GUI 断线影响）。
    tokio::time::sleep(Duration::from_millis(100)).await;

    // 重连：初始 Snapshot 已包含 Run 当前状态（ActiveRuns section）。
    let (gui_new, snapshot) = runtime.connect_gui().await;
    let active_runs = snapshot
        .sections
        .iter()
        .find(|section| section.kind == gui_protocol::SnapshotSectionKind::ActiveRuns)
        .expect("ActiveRuns section");
    let data = active_runs.data.as_ref().expect("ActiveRuns data");
    let runs = data["runs"].as_array().expect("runs array");
    assert!(
        runs.iter()
            .any(|run| run["run_id"] == json!(run_id.as_str())
                && run["state"] == json!("streaming_response")),
        "快照应重建 Run 的活跃状态"
    );

    // 断线期间产生新事件：CLI 取消 Run（不依赖任何 GUI 连接）。
    runtime.app_service.dispatch_envelope(command(
        cli_source(),
        cli_identity(),
        AppCommand::RunCancel {
            run_id: run_id.clone(),
        },
    ));
    // 等待 Cancelled 事件真正发布进 Hub 再 Resume：断线窗口内可能有其它
    // 异步事件先落地（如能力协商 Diagnostic），仅凭序列推进会提前退出、
    // 使 resume 窗口错过取消事件（重放断言要求取消事件落在重放窗口内）。
    let deadline = Instant::now() + Duration::from_secs(10);
    while runtime
        .app_service
        .router()
        .aggregate()
        .get_run(
            &run_id,
            &agent_domain::TenantId::new(core_api::DEFAULT_CONTROL_PLANE_TENANT),
        )
        .is_none_or(|run| run.state != RunState::Cancelled)
    {
        assert!(Instant::now() < deadline, "取消后 Run 应进入 Cancelled");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let publish_deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let cancellation_published = runtime
            .hub
            .replay(GlobalSequence(last_sequence + 1), None)
            .expect("cancellation replay window")
            .iter()
            .any(|event| {
                matches!(
                    event.payload,
                    AppEvent::RunChanged {
                        run_id: ref event_run_id,
                        ref state,
                    } if event_run_id == &run_id && *state == RunState::Cancelled
                )
            });
        if cancellation_published {
            break;
        }
        assert!(
            Instant::now() < publish_deadline,
            "取消状态应发布到 EventHub 后再发起 Resume"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    // Resume：按客户端 last_global_sequence 补发断线期间缺失事件。
    // `ResumeDisposition::through_sequence` 是服务端处理请求时捕获的 Hub 序列；
    // 响应送达客户端前后台 Run 仍可能发布后续事件，因此不能与读取响应时的
    // `hub.current()` 做瞬时相等断言，只能断言它落在发送前后的序列区间内。
    let current_before_resume = runtime.hub.current().0;
    gui_new
        .send(&ClientFrame::Resume(ResumeRequest {
            request_id: "resume-1".into(),
            last_global_sequence: GlobalSequence(last_sequence),
        }))
        .await;
    let mut replayed = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        match tokio::time::timeout(
            deadline.saturating_duration_since(Instant::now()),
            gui_new.recv(),
        )
        .await
        {
            Ok(ServerFrame::Resume(response)) => {
                assert_eq!(response.request_id, "resume-1");
                assert!(
                    matches!(
                        response.disposition,
                        gui_protocol::ResumeDisposition::Replay {
                            from_sequence: _,
                            through_sequence: _,
                        }
                    ),
                    "Ring 窗口内应 Replay，got {:?}",
                    response.disposition
                );
                if let gui_protocol::ResumeDisposition::Replay {
                    from_sequence,
                    through_sequence,
                } = response.disposition
                {
                    assert_eq!(from_sequence.0, last_sequence + 1);
                    assert!(through_sequence.0 >= current_before_resume);
                    assert!(through_sequence.0 <= runtime.hub.current().0);
                }
            }
            Ok(ServerFrame::Event(envelope)) => {
                assert!(
                    envelope.global_sequence.0 > last_sequence,
                    "重放事件应晚于 last_global_sequence"
                );
                replayed.push(envelope);
            }
            Ok(other) => panic!("unexpected frame during replay: {other:?}"),
            Err(_) => break,
        }
    }
    assert!(!replayed.is_empty(), "断线期间应有缺失事件被 Replay 补发");
    assert!(
        replayed
            .iter()
            .any(|e| e.stream == EventStream::Run(run_id.clone())),
        "重放事件应覆盖 Run 流"
    );
    assert!(
        replayed.iter().any(
            |e| matches!(e.payload, AppEvent::RunChanged { ref state, .. }
                if *state == RunState::Cancelled)
        ),
        "重放应包含断线期间的取消事件"
    );
    let sequences: Vec<u64> = replayed.iter().map(|e| e.global_sequence.0).collect();
    for window in sequences.windows(2) {
        assert!(window[1] > window[0], "重放事件序列应严格递增");
    }

    // 断线清理：旧连接已注销；Run 由显式 RunCancel 取消。
    assert!(
        !runtime.app_service.router().supervisor().is_active(&run_id),
        "Run 应已被显式取消"
    );
}

/// 验收 5b：窗口不可用（ring 已淘汰）时 Resume 降级 SnapshotRequired + Snapshot。
#[tokio::test]
async fn resume_falls_back_to_snapshot_when_replay_unavailable() {
    // 容量 2 的 Hub：发布大量合成事件使 earliest_available 越过客户端 last。
    let transport = Arc::new(MemoryTransport::new());
    let runtime = Runtime::new(
        Arc::new(EventHub::with_capacity(2)),
        transport.clone(),
        transport.clone(),
    )
    .await;
    let run_id = RunId::from("synth-run");
    for i in 1..=20u64 {
        runtime
            .hub
            .publish(synthetic_event(&run_id, i, RunState::StreamingResponse));
    }
    let earliest = runtime.hub.earliest_available().expect("earliest").0;
    assert!(earliest > 2, "ring 应已淘汰早期事件（earliest={earliest}）");

    let (gui, _) = runtime.connect_gui().await;
    gui.send(&ClientFrame::Resume(ResumeRequest {
        request_id: "resume-fallback".into(),
        last_global_sequence: GlobalSequence(1),
    }))
    .await;
    let mut saw_fallback = false;
    let mut saw_snapshot = false;
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        match tokio::time::timeout(
            deadline.saturating_duration_since(Instant::now()),
            gui.recv(),
        )
        .await
        {
            Ok(ServerFrame::Resume(response)) => {
                assert_eq!(response.request_id, "resume-fallback");
                assert!(
                    matches!(
                        response.disposition,
                        gui_protocol::ResumeDisposition::SnapshotRequired { .. }
                    ),
                    "窗口不可用应降级 SnapshotRequired，got {:?}",
                    response.disposition
                );
                saw_fallback = true;
            }
            Ok(ServerFrame::Snapshot(_)) => saw_snapshot = true,
            Ok(other) => panic!("unexpected frame: {other:?}"),
            Err(_) => break,
        }
    }
    assert!(saw_fallback, "应收到 SnapshotRequired 降级响应");
    assert!(saw_snapshot, "降级后应补发 Snapshot");
}

/// 验收 6：慢客户端（不消费事件）不阻塞 Agent 或其他 GUI。
#[tokio::test]
async fn slow_client_does_not_block_agent_or_other_guis() {
    let transport = Arc::new(MemoryTransport::new());
    let slow_transport = Arc::new(SlowTransport::new(
        (*transport).clone(),
        1, // 仅第一条连接（慢 GUI）阻塞
        2, // 握手响应 + Snapshot 后阻塞 send
    ));
    let runtime = Runtime::new(Arc::new(EventHub::new()), slow_transport.clone(), transport).await;
    runtime.register_provider(mock_provider());
    let session_id = runtime.prepare_session();

    let (slow, _) = runtime.connect_gui().await;
    slow.subscribe_all().await;
    let (fast, _) = runtime.connect_gui().await;
    fast.subscribe_all().await;

    let run_id = runtime.start_run_cli(&session_id, "flood");

    // 快客户端照常收全事件（包括 Completed）。
    let (_events, done) = fast
        .recv_until(
            |e| run_state(e, &run_id) == Some(RunState::Completed),
            Duration::from_secs(10),
        )
        .await;
    assert!(done, "快客户端应收到 Completed，不被慢客户端阻塞");

    // Agent 不被阻塞：Run 完成（独立于 GUI 会话任务）。
    assert!(
        runtime
            .app_service
            .router()
            .aggregate()
            .get_run(
                &run_id,
                &agent_domain::TenantId::new(core_api::DEFAULT_CONTROL_PLANE_TENANT),
            )
            .is_some_and(|run| run.state == RunState::Completed),
        "Run 应正常完成"
    );

    // 灌事件淹没慢客户端（默认队列容量 1024，分批发布让快客户端帧循环跟得上）：
    // 慢客户端帧循环阻塞在 send，队列只进不出 → 溢出 → Lagged。
    for batch in 0..13u64 {
        for i in 0..100u64 {
            runtime.hub.publish(synthetic_event(
                &RunId::from(format!("flood-{batch}")),
                i,
                RunState::StreamingResponse,
            ));
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }

    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if runtime
            .connections
            .session(&agent_domain::GuiClientId::from("client-0"))
            .is_some_and(|session| session.lagged)
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let session = runtime
        .connections
        .session(&agent_domain::GuiClientId::from("client-0"))
        .expect("slow session 仍在登记");
    assert!(session.lagged, "慢客户端应被标记 Lagged");
    // 发布者（Hub）不受影响：订阅者计数与最新序列正常。
    assert!(runtime.hub.subscriber_count() >= 2);
    assert!(runtime.hub.current().0 > 1300);

    // 慢客户端断线清理（不取消 Run）：释放阻塞后会话观察到断线并注销。
    slow.conn.close().await.expect("close slow");
    slow_transport.release();
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if runtime.connections.count() == 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(runtime.connections.count(), 1, "慢客户端断线后应注销");
}
