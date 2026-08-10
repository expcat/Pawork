//! P13-9 GUI Connection Protocol 契约测试（gui-client SDK × gui-server）。
//!
//! 进程内装配：MemoryTransport + GuiServer + AppService + EventHub + pump
//! （harness 参照 crates/gui-server/tests/multi_gui_runtime.rs），客户端全部
//! 经 `gui-client` SDK 连接。覆盖：
//!
//! a) 创建 session / 发消息 / 收流式 Run 事件；
//! b) 快照与断线重连（Replay 补发 + SnapshotRequired 降级重建）；
//! c) 3 个 GUI 并发同步（CLI 与 GUI 发起的 Run 同步到所有 GUI）；
//! d) 命令幂等（同 command_id 重放返回相同响应）；
//! e) 大 artifact 分片读取（100k 行 diff 文本分片重组一致）；
//! f) 版本不兼容握手被拒；
//! g) GUI 断线不取消 Run；
//! 另覆盖 Ack / Heartbeat 往返。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use agent_domain::{
    ActorId, ArtifactId, CommandId, CoreInstanceId, EventId, ProviderId, RunId, SessionId,
    Timestamp, WorkspaceId,
};
use app_service::AppService;
use artifact_store::ArtifactStore;
use client_auth::{Token, TokenAuthenticator, TokenStore};
use connection_manager::ConnectionManager;
use core_api::{
    ActorIdentity, ApiVersion, AppCommand, AppCommandEnvelope, AppEvent, AppEventEnvelope,
    AppQuery, AppResponse, CommandSource, EventSource, EventStream, GlobalSequence, RunState,
    API_VERSION, SUPPORTED_API_VERSIONS,
};
use gui_client::{ClientConfig, ClientError, GuiClient, ResumeDisposition, SessionInfo};
use gui_protocol::{GuiCapability, HandshakeService, ProtocolErrorCode};
use gui_server::{GuiServer, GuiServerConfig};
use provider_api::ModelProvider;
use serde_json::{json, Value};
use subscription_hub::EventHub;
use tempfile::TempDir;
use transport_api::{
    ConnectOptions, GuiConnection, GuiListener, GuiTransportClient, TransportEndpoint,
};
use transport_memory::MemoryTransport;

const CHANNEL: &str = "gui-client-contract";
const CLIENT_TIMEOUT: Duration = Duration::from_secs(10);

// ---------------------------------------------------------------------------
// 基础设施：Runtime（app-service + hub + pump + server）与 SDK 客户端
// ---------------------------------------------------------------------------

struct Harness {
    app_service: Arc<AppService>,
    hub: Arc<EventHub>,
    connections: Arc<ConnectionManager>,
    transport: Arc<MemoryTransport>,
    listener: Arc<dyn GuiListener>,
    token: Token,
    pump: tokio::task::JoinHandle<()>,
    /// 宿主侧连接句柄：drop 会释放 close 通道导致会话断线，须持有到测试结束。
    sessions: Vec<Box<dyn GuiConnection>>,
    _temp: TempDir,
}

impl Harness {
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

    async fn new(instance: &str, channel: &str) -> Self {
        Self::new_with(instance, channel, None).await
    }

    async fn new_with_artifact_store(
        instance: &str,
        channel: &str,
        store: Arc<ArtifactStore>,
    ) -> Self {
        let app_service = Arc::new(AppService::with_artifact_store(instance, store));
        let hub = Arc::new(EventHub::new());
        let pump = Self::spawn_pump(Arc::clone(&app_service), Arc::clone(&hub));
        let temp = tempfile::tempdir().expect("tempdir");
        let token_path = temp.path().join("gui.token");
        let token = TokenStore::new(&token_path)
            .generate()
            .expect("generate token");
        let handshake = HandshakeService::new(
            CoreInstanceId::from(instance),
            SUPPORTED_API_VERSIONS.to_vec(),
            vec![
                GuiCapability::Events,
                GuiCapability::Snapshots,
                GuiCapability::ArtifactStreaming,
            ],
        )
        .with_authenticator(Box::new(TokenAuthenticator::new(TokenStore::new(
            &token_path,
        ))));
        let transport = Arc::new(MemoryTransport::new());
        let connections = Arc::new(ConnectionManager::default());
        let server = GuiServer::new(GuiServerConfig {
            app_service: Arc::clone(&app_service),
            handshake,
            transport: Arc::clone(&transport) as Arc<dyn transport_api::GuiTransportServer>,
            hub: Arc::clone(&hub),
            connections: Some(Arc::clone(&connections)),
        });
        let listener = server
            .bind(TransportEndpoint::Memory {
                channel: channel.into(),
            })
            .await
            .expect("bind");
        Harness {
            app_service,
            hub,
            connections,
            transport,
            listener: Arc::from(listener),
            token,
            pump,
            sessions: Vec::new(),
            _temp: temp,
        }
    }

    async fn new_with(instance: &str, channel: &str, hub: Option<Arc<EventHub>>) -> Self {
        let app_service = Arc::new(AppService::new(instance));
        let hub = hub.unwrap_or_else(|| Arc::new(EventHub::new()));
        let pump = Self::spawn_pump(Arc::clone(&app_service), Arc::clone(&hub));
        let temp = tempfile::tempdir().expect("tempdir");
        let token_path = temp.path().join("gui.token");
        let token = TokenStore::new(&token_path)
            .generate()
            .expect("generate token");
        let handshake = HandshakeService::new(
            CoreInstanceId::from(instance),
            SUPPORTED_API_VERSIONS.to_vec(),
            vec![
                GuiCapability::Events,
                GuiCapability::Snapshots,
                GuiCapability::ArtifactStreaming,
            ],
        )
        .with_authenticator(Box::new(TokenAuthenticator::new(TokenStore::new(
            &token_path,
        ))));
        let transport = Arc::new(MemoryTransport::new());
        let connections = Arc::new(ConnectionManager::default());
        let server = GuiServer::new(GuiServerConfig {
            app_service: Arc::clone(&app_service),
            handshake,
            transport: Arc::clone(&transport) as Arc<dyn transport_api::GuiTransportServer>,
            hub: Arc::clone(&hub),
            connections: Some(Arc::clone(&connections)),
        });
        let listener = server
            .bind(TransportEndpoint::Memory {
                channel: channel.into(),
            })
            .await
            .expect("bind");
        Harness {
            app_service,
            hub,
            connections,
            transport,
            listener: Arc::from(listener),
            token,
            pump,
            sessions: Vec::new(),
            _temp: temp,
        }
    }

    /// 注册 MockProvider（CLI 侧注册，Agent 运行依赖）。
    fn register_provider(&self, provider: Arc<dyn ModelProvider>) -> ProviderId {
        self.app_service.register_provider(provider)
    }

    /// 建 workspace + session（CLI 来源），返回 session_id。
    fn prepare_session(&self) -> SessionId {
        let dir = std::env::temp_dir().join(format!("pawork-contract-{}", std::process::id()));
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
                    .and_then(Value::as_str)
                    .expect("workspace id"),
            ),
            other => panic!("expected workspace data, got {other:?}"),
        };
        let response = self.app_service.dispatch_envelope(command(
            cli_source(),
            cli_identity(),
            AppCommand::SessionCreate {
                workspace_id,
                title: Some("contract".into()),
            },
        ));
        match &response.response {
            AppResponse::Data(value) => SessionId::from(
                value
                    .get("session_id")
                    .and_then(Value::as_str)
                    .expect("session id"),
            ),
            other => panic!("expected session data, got {other:?}"),
        }
    }

    fn connect_options(label: &str) -> ConnectOptions {
        ConnectOptions {
            timeout_ms: 2_000,
            client_label: Some(label.into()),
            max_frame_bytes: 1024 * 1024,
        }
    }

    /// 经 SDK 连接一个新 GUI：accept 与 connect 并行，持有宿主侧句柄。
    async fn connect_gui(&mut self, label: &str) -> GuiClient {
        let listener = Arc::clone(&self.listener);
        let accept = tokio::spawn(async move { listener.accept().await });
        let transport: Arc<dyn GuiTransportClient> = self.transport.clone();
        let client = GuiClient::connect(
            transport,
            TransportEndpoint::Memory {
                channel: CHANNEL.into(),
            },
            Self::connect_options(label),
            &self.token,
        )
        .await
        .expect("gui client connect + handshake");
        let session = accept.await.expect("accept task").expect("accept");
        self.sessions.push(session);
        client
    }

    /// 经 SDK 重连（connect_with_resume 辅助），持有宿主侧句柄。
    async fn reconnect_gui(
        &mut self,
        label: &str,
        last_global_sequence: Option<GlobalSequence>,
    ) -> (GuiClient, Option<gui_client::ResumeOutcome>) {
        let listener = Arc::clone(&self.listener);
        let accept = tokio::spawn(async move { listener.accept().await });
        let transport: Arc<dyn GuiTransportClient> = self.transport.clone();
        let (client, outcome) = GuiClient::connect_with_resume(
            transport,
            TransportEndpoint::Memory {
                channel: CHANNEL.into(),
            },
            Self::connect_options(label),
            &self.token,
            last_global_sequence,
        )
        .await
        .expect("gui client reconnect");
        let session = accept.await.expect("accept task").expect("accept");
        self.sessions.push(session);
        (client, outcome)
    }

    /// CLI 发起的 RunStart（不依赖任何 GUI 连接）。
    fn start_run_cli(&self, session_id: &SessionId, message: &str) -> RunId {
        let response = self.app_service.dispatch_envelope(command(
            cli_source(),
            cli_identity(),
            AppCommand::RunStart {
                session_id: session_id.clone(),
                user_message: message.into(),
                model: None,
            },
        ));
        assert!(
            matches!(response.response, AppResponse::Accepted { .. }),
            "RunStart 应 Accepted，got {:?}",
            response.response
        );
        self.app_service
            .router()
            .last_started_run()
            .expect("run id")
    }

    fn cancel_run_cli(&self, run_id: &RunId) {
        self.app_service.dispatch_envelope(command(
            cli_source(),
            cli_identity(),
            AppCommand::RunCancel {
                run_id: run_id.clone(),
            },
        ));
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        self.pump.abort();
    }
}

// ---------------------------------------------------------------------------
// 事件 / 命令构造与断言辅助
// ---------------------------------------------------------------------------

fn cli_source() -> CommandSource {
    CommandSource::LocalCli {
        terminal_session_id: Some("terminal-1".into()),
    }
}

fn cli_identity() -> ActorIdentity {
    ActorIdentity::LocalUser {
        actor_id: ActorId::from("contract-tester"),
        display_name: None,
    }
}

fn local_user() -> ActorIdentity {
    ActorIdentity::LocalUser {
        actor_id: ActorId::from("contract-gui-user"),
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

fn run_state(event: &AppEventEnvelope, run_id: &RunId) -> Option<RunState> {
    match &event.payload {
        AppEvent::RunChanged { run_id: id, state } if id == run_id => Some(state.clone()),
        _ => None,
    }
}

fn data_field(response: &core_api::AppResponseEnvelope, field: &str) -> String {
    match &response.response {
        AppResponse::Data(value) => value
            .get(field)
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("response 应含字段 {field:?}: {value}"))
            .to_string(),
        other => panic!("expected Data response, got {other:?}"),
    }
}

fn synthetic_event(run_id: &RunId, sequence_hint: u64, state: RunState) -> AppEventEnvelope {
    AppEventEnvelope {
        api_version: API_VERSION,
        instance_id: CoreInstanceId::from("contract-synthetic"),
        event_id: EventId::from(format!("synth-{sequence_hint}")),
        global_sequence: GlobalSequence(sequence_hint),
        stream: EventStream::Run(run_id.clone()),
        stream_sequence: sequence_hint,
        timestamp: Timestamp::from_unix_millis(sequence_hint),
        source: EventSource::Core,
        payload: AppEvent::RunChanged {
            run_id: run_id.clone(),
            state,
        },
    }
}

fn diff_payload(lines: usize) -> Vec<u8> {
    let mut out = String::new();
    out.push_str("diff --git a/src/main.rs b/src/main.rs\n");
    out.push_str("--- a/src/main.rs\n");
    out.push_str("+++ b/src/main.rs\n");
    out.push_str(&format!("@@ -1,{lines} +1,{lines} @@\n"));
    for i in 0..lines {
        out.push_str(&format!(
            "+pub fn line_{i}(value: usize) -> usize {{ value + {i} }}\n"
        ));
    }
    out.into_bytes()
}

static NEXT_ID: AtomicU64 = AtomicU64::new(0);

fn next_id() -> u64 {
    NEXT_ID.fetch_add(1, Ordering::SeqCst)
}

/// 在超时内读取事件直到谓词满足，返回 (是否满足, 期间收到的全部事件)。
async fn recv_until<F: Fn(&AppEventEnvelope) -> bool>(
    client: &GuiClient,
    predicate: F,
) -> (bool, Vec<AppEventEnvelope>) {
    let deadline = Instant::now() + CLIENT_TIMEOUT;
    let mut received = Vec::new();
    while Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(Instant::now());
        match client.next_event_timeout(remaining).await {
            Ok(event) => {
                if predicate(&event) {
                    received.push(event);
                    return (true, received);
                }
                received.push(event);
            }
            Err(error) => panic!("等待事件失败: {error}"),
        }
    }
    (false, received)
}

// ---------------------------------------------------------------------------
// a) 创建 session / 发消息 / 收流式 Run 事件
// ---------------------------------------------------------------------------

#[tokio::test]
async fn create_session_send_message_and_receive_streaming_run_events() {
    let mut harness = Harness::new("contract-a", CHANNEL).await;
    harness.register_provider(streaming_provider());
    let client = harness.connect_gui("contract-a").await;
    let info: &SessionInfo = client.info();
    assert_eq!(info.client_id.as_str(), "client-0");
    assert_eq!(info.handle.api_version, API_VERSION);
    assert!(
        client.initial_snapshot().is_some(),
        "握手后应有首帧 Snapshot"
    );

    let source = CommandSource::LocalGui {
        client_id: client.client_id().clone(),
    };
    let identity = local_user();
    let dir = std::env::temp_dir().join(format!("pawork-contract-a-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create workspace dir");
    let workspace = client
        .command(
            AppCommand::WorkspaceAdd {
                root_path: dir.to_string_lossy().into_owned(),
            },
            source.clone(),
            identity.clone(),
        )
        .await
        .expect("WorkspaceAdd");
    let workspace_id = WorkspaceId::from(data_field(&workspace, "id"));

    let session = client
        .command(
            AppCommand::SessionCreate {
                workspace_id,
                title: Some("contract-a".into()),
            },
            source.clone(),
            identity.clone(),
        )
        .await
        .expect("SessionCreate");
    let session_id = SessionId::from(data_field(&session, "session_id"));
    client.subscribe_all().await.expect("subscribe all");

    let run = client
        .command(
            AppCommand::RunStart {
                session_id: session_id.clone(),
                user_message: "hello from contract".into(),
                model: None,
            },
            source,
            identity,
        )
        .await
        .expect("RunStart");
    assert!(matches!(run.response, AppResponse::Accepted { .. }));
    let run_id = harness
        .app_service
        .router()
        .last_started_run()
        .expect("run id");

    let (done, events) = recv_until(&client, |e| {
        run_state(e, &run_id) == Some(RunState::Completed)
    })
    .await;
    assert!(done, "应收到 Run 的 Completed 事件");
    assert!(
        events
            .iter()
            .any(|e| run_state(e, &run_id) == Some(RunState::StreamingResponse)),
        "应收到流式响应状态事件"
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(&e.payload, AppEvent::AssistantDelta { .. })),
        "应收到流式增量事件"
    );
    assert!(
        events
            .iter()
            .all(|e| e.stream == EventStream::Run(run_id.clone())),
        "Run 事件应属于该 Run 流"
    );

    // Query 路径：RunStatus 返回 completed。
    let status = client
        .query(
            AppQuery::RunStatus {
                run_id: run_id.clone(),
            },
            CommandSource::LocalGui {
                client_id: client.client_id().clone(),
            },
            local_user(),
        )
        .await
        .expect("RunStatus");
    let AppResponse::Data(data) = &status.response else {
        panic!("expected Data response, got {:?}", status.response);
    };
    assert_eq!(data["state"], json!("completed"));
}

// ---------------------------------------------------------------------------
// b) 快照与断线重连：Replay 补发 / SnapshotRequired 重建
// ---------------------------------------------------------------------------

#[tokio::test]
async fn snapshot_and_reconnect_resume_replays_missing_events() {
    let mut harness = Harness::new("contract-b", CHANNEL).await;
    harness.register_provider(Arc::new(
        test_support::MockProvider::new(test_support::MockScript::new().wait_for_cancellation())
            .with_id(ProviderId::from("mock")),
    ));
    let session_id = harness.prepare_session();

    let client = harness.connect_gui("contract-b-first").await;
    client.subscribe_all().await.expect("subscribe");
    let run_id = harness.start_run_cli(&session_id, "long run");
    let (done, events) = recv_until(&client, |e| {
        run_state(e, &run_id) == Some(RunState::StreamingResponse)
    })
    .await;
    assert!(done, "第一段连接应进入 StreamingResponse");
    let last_sequence = events
        .iter()
        .map(|e| e.global_sequence.0)
        .max()
        .expect("至少一个事件");

    // SnapshotRequest：快照应重建 Run 的活跃状态。
    let snapshot = client.snapshot().await.expect("snapshot");
    assert_eq!(snapshot.instance_id.as_str(), "contract-b");
    let active_runs = snapshot
        .sections
        .iter()
        .find(|section| section.kind == gui_protocol::SnapshotSectionKind::ActiveRuns)
        .expect("ActiveRuns section");
    let runs = active_runs.data.as_ref().expect("ActiveRuns data")["runs"]
        .as_array()
        .expect("runs array");
    assert!(
        runs.iter().any(|run| {
            run["run_id"] == json!(run_id.as_str()) && run["state"] == json!("streaming_response")
        }),
        "快照应重建 Run 的活跃状态"
    );

    client
        .ack(GlobalSequence(last_sequence))
        .await
        .expect("ack");
    client.close().await.expect("close");

    // 断线后 Run 继续存活（Agent 不被 GUI 断线影响）。
    tokio::time::sleep(Duration::from_millis(100)).await;

    // 断线期间产生新事件：CLI 取消 Run。
    harness.cancel_run_cli(&run_id);
    let deadline = Instant::now() + CLIENT_TIMEOUT;
    while harness.hub.current().0 <= last_sequence {
        assert!(Instant::now() < deadline, "取消后应产生新事件");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    // 重连：connect_with_resume 按 last_global_sequence Replay 补齐缺失事件。
    let (reconnected, outcome) = harness
        .reconnect_gui("contract-b-second", Some(GlobalSequence(last_sequence)))
        .await;
    let outcome = outcome.expect("resume outcome");
    assert!(
        matches!(outcome.disposition, ResumeDisposition::Replay { .. }),
        "Ring 窗口内应 Replay，got {:?}",
        outcome.disposition
    );
    let (from, through) = match outcome.disposition {
        ResumeDisposition::Replay {
            from_sequence,
            through_sequence,
        } => (from_sequence.0, through_sequence.0),
        _ => unreachable!(),
    };
    assert_eq!(from, last_sequence + 1);
    assert_eq!(through, harness.hub.current().0);
    assert!(!outcome.replayed.is_empty(), "断线期间应有缺失事件被补发");
    assert!(
        outcome.replayed.iter().any(|e| matches!(
            e.payload,
            AppEvent::RunChanged {
                ref state,
                ..
            } if *state == RunState::Cancelled
        )),
        "重放应包含断线期间的取消事件"
    );
    let sequences: Vec<u64> = outcome
        .replayed
        .iter()
        .map(|e| e.global_sequence.0)
        .collect();
    assert!(sequences.windows(2).all(|w| w[1] > w[0]), "序列应严格递增");

    // 重连后的客户端可用：心跳往返。
    let nonce = reconnected.heartbeat().await.expect("heartbeat");
    assert_eq!(nonce, 0);
    assert!(!harness.app_service.router().supervisor().is_active(&run_id));
}

#[tokio::test]
async fn resume_falls_back_to_snapshot_required_when_replay_unavailable() {
    // 容量 2 的 Hub：发布大量合成事件使 earliest_available 越过客户端 last。
    let hub = Arc::new(EventHub::with_capacity(2));
    let mut harness = Harness::new_with("contract-b2", CHANNEL, Some(hub.clone())).await;
    let run_id = RunId::from("synth-run");
    for i in 1..=20u64 {
        harness
            .hub
            .publish(synthetic_event(&run_id, i, RunState::StreamingResponse));
    }
    assert!(
        harness.hub.earliest_available().expect("earliest").0 > 2,
        "ring 应已淘汰早期事件"
    );

    let client = harness.connect_gui("contract-b2").await;
    let outcome = client.resume(GlobalSequence(1)).await.expect("resume");
    assert!(
        matches!(
            outcome.disposition,
            ResumeDisposition::SnapshotRequired { .. }
        ),
        "窗口不可用应降级 SnapshotRequired，got {:?}",
        outcome.disposition
    );
    let snapshot = outcome.snapshot.expect("降级后应补发 Snapshot");
    assert_eq!(snapshot.instance_id.as_str(), "contract-b2");
    assert!(outcome.replayed.is_empty());
}

// ---------------------------------------------------------------------------
// c) 3 GUI 并发同步
// ---------------------------------------------------------------------------

#[tokio::test]
async fn three_gui_clients_sync_runs_from_cli_and_each_other() {
    let mut harness = Harness::new("contract-c", CHANNEL).await;
    harness.register_provider(streaming_provider());
    let session_id = harness.prepare_session();

    let gui_a = harness.connect_gui("contract-c-a").await;
    let gui_b = harness.connect_gui("contract-c-b").await;
    let gui_c = harness.connect_gui("contract-c-c").await;
    assert_eq!(
        harness.connections.count(),
        3,
        "一个 CLI/Core 应同时登记 3 个 GUI"
    );
    gui_a.subscribe_all().await.expect("subscribe a");
    gui_b.subscribe_all().await.expect("subscribe b");
    gui_c.subscribe_all().await.expect("subscribe c");

    // CLI 发起的 Run 同步到所有 GUI。
    let run_id = harness.start_run_cli(&session_id, "hello from cli");
    for (name, gui) in [("A", &gui_a), ("B", &gui_b), ("C", &gui_c)] {
        let (done, _) =
            recv_until(gui, |e| run_state(e, &run_id) == Some(RunState::Completed)).await;
        assert!(done, "GUI {name} 应收到 CLI Run 的 Completed");
    }

    // GUI A 发起的 Run 同步到 CLI 与 GUI B / C。
    let mut cli_observer = harness.hub.subscribe();
    let source = CommandSource::LocalGui {
        client_id: gui_a.client_id().clone(),
    };
    gui_a
        .command(
            AppCommand::RunStart {
                session_id: session_id.clone(),
                user_message: "run from gui a".into(),
                model: None,
            },
            source,
            local_user(),
        )
        .await
        .expect("gui a RunStart");
    let gui_run_id = harness
        .app_service
        .router()
        .last_started_run()
        .expect("gui run id");
    for (name, gui) in [("B", &gui_b), ("C", &gui_c)] {
        let (done, _) = recv_until(gui, |e| {
            run_state(e, &gui_run_id) == Some(RunState::Completed)
        })
        .await;
        assert!(done, "GUI {name} 应收到 GUI A Run 的 Completed");
    }
    let deadline = Instant::now() + CLIENT_TIMEOUT;
    let mut cli_saw_completed = false;
    while Instant::now() < deadline {
        match tokio::time::timeout(
            deadline.saturating_duration_since(Instant::now()),
            cli_observer.recv(),
        )
        .await
        {
            Ok(Ok(event)) => {
                if run_state(&event, &gui_run_id) == Some(RunState::Completed) {
                    cli_saw_completed = true;
                    break;
                }
            }
            _ => break,
        }
    }
    assert!(cli_saw_completed, "CLI 观察者应收到 GUI A Run 的 Completed");
}

// ---------------------------------------------------------------------------
// d) 命令幂等：同 command_id 重放返回相同响应
// ---------------------------------------------------------------------------

#[tokio::test]
async fn same_command_id_replays_same_response() {
    let mut harness = Harness::new("contract-d", CHANNEL).await;
    let client = harness.connect_gui("contract-d").await;
    let source = CommandSource::LocalGui {
        client_id: client.client_id().clone(),
    };
    let identity = local_user();
    let dir = std::env::temp_dir().join(format!("pawork-contract-d-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create workspace dir");
    let workspace = client
        .command(
            AppCommand::WorkspaceAdd {
                root_path: dir.to_string_lossy().into_owned(),
            },
            source.clone(),
            identity.clone(),
        )
        .await
        .expect("WorkspaceAdd");
    let workspace_id = WorkspaceId::from(data_field(&workspace, "id"));

    let envelope = AppCommandEnvelope {
        api_version: API_VERSION,
        command_id: CommandId::from("idem-session-1"),
        source,
        identity,
        expected_revision: None,
        idempotency_key: None,
        issued_at: Timestamp::from_unix_millis(7),
        command: AppCommand::SessionCreate {
            workspace_id,
            title: Some("idempotent".into()),
        },
    };
    let first = client
        .command_envelope(envelope.clone())
        .await
        .expect("首次执行");
    let replayed = client.command_envelope(envelope).await.expect("重放");
    assert_eq!(
        replayed, first,
        "同 command_id 重放必须返回完全相同的响应（不重复执行）"
    );
    let session_id = SessionId::from(data_field(&replayed, "session_id"));
    let status = client
        .query(
            AppQuery::SessionGet {
                session_id: session_id.clone(),
            },
            CommandSource::LocalGui {
                client_id: client.client_id().clone(),
            },
            local_user(),
        )
        .await
        .expect("SessionGet");
    assert!(
        matches!(status.response, AppResponse::Data(_)),
        "幂等重放创建的 session 应可查询"
    );
}

// ---------------------------------------------------------------------------
// e) 大 artifact 分片读取
// ---------------------------------------------------------------------------

#[tokio::test]
async fn large_artifact_chunked_read_reassembles_consistently() {
    let diff = diff_payload(100_000);
    assert!(
        diff.len() > 5 * 1024 * 1024,
        "diff 应约 5MiB，实际 {}",
        diff.len()
    );
    let temp = tempfile::tempdir().expect("tempdir");
    let store = Arc::new(
        ArtifactStore::open(temp.path().join("store"))
            .await
            .expect("open store"),
    );
    let outcome = store.put(&diff).await.expect("put blob");
    let artifact_id = ArtifactId::from(outcome.id.as_str());

    let mut harness =
        Harness::new_with_artifact_store("contract-e", CHANNEL, Arc::clone(&store)).await;
    harness
        .app_service
        .router()
        .aggregate()
        .put_artifact(artifact_id.clone(), diff.len() as u64, "text/x-diff".into())
        .expect("register artifact");
    let client = harness.connect_gui("contract-e").await;

    // 全量读取：分片重组必须与 store.put 的原始 payload 一致。
    let assembled = client
        .read_artifact(&artifact_id, 0, 0)
        .await
        .expect("read artifact");
    assert_eq!(assembled, diff, "分片重组必须与原始 payload 一致");

    // 部分读取：limit 精确截断。
    let partial = client
        .read_artifact(&artifact_id, 64 * 1024, 70 * 1024)
        .await
        .expect("read partial");
    assert_eq!(partial, diff[64 * 1024..64 * 1024 + 70 * 1024]);

    // 缺失 artifact → RequestNotFound 结构化错误。
    let error = client
        .read_artifact(&ArtifactId::from("art-missing"), 0, 0)
        .await
        .expect_err("缺失 artifact 应报错");
    assert!(error.is_request_not_found(), "got {error:?}");
}

// ---------------------------------------------------------------------------
// f) 版本不兼容握手被拒
// ---------------------------------------------------------------------------

#[tokio::test]
async fn incompatible_version_handshake_is_rejected() {
    let harness = Harness::new("contract-f", CHANNEL).await;
    let listener = Arc::clone(&harness.listener);
    let accept = tokio::spawn(async move { listener.accept().await });
    let transport: Arc<dyn GuiTransportClient> = harness.transport.clone();
    let config = ClientConfig {
        supported_api_versions: vec![ApiVersion { major: 2, minor: 0 }],
        ..ClientConfig::default()
    };
    let error = match GuiClient::connect_with_config(
        transport,
        TransportEndpoint::Memory {
            channel: CHANNEL.into(),
        },
        Harness::connect_options("contract-f"),
        &harness.token,
        config,
    )
    .await
    {
        Err(error) => error,
        Ok(_) => panic!("版本不兼容必须拒绝"),
    };
    assert!(error.is_incompatible_version(), "got {error:?}");
    match error {
        ClientError::HandshakeRejected(protocol_error) => {
            assert_eq!(protocol_error.code, ProtocolErrorCode::IncompatibleVersion);
        }
        other => panic!("expected HandshakeRejected, got {other:?}"),
    }
    let session = accept.await.expect("accept task").expect("accept");
    let _ = session;
}

// ---------------------------------------------------------------------------
// g) GUI 断线不取消 Run
// ---------------------------------------------------------------------------

#[tokio::test]
async fn gui_disconnect_does_not_cancel_run() {
    let mut harness = Harness::new("contract-g", CHANNEL).await;
    harness.register_provider(Arc::new(
        test_support::MockProvider::new(test_support::MockScript::new().wait_for_cancellation())
            .with_id(ProviderId::from("mock")),
    ));
    let session_id = harness.prepare_session();

    let client = harness.connect_gui("contract-g").await;
    client.subscribe_all().await.expect("subscribe");
    let run_id = harness.start_run_cli(&session_id, "survives disconnect");
    let (done, _) = recv_until(&client, |e| {
        run_state(e, &run_id) == Some(RunState::StreamingResponse)
    })
    .await;
    assert!(done, "Run 应进入 StreamingResponse");

    client.close().await.expect("close");
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        harness.app_service.router().supervisor().is_active(&run_id),
        "GUI 断线不得取消 Run"
    );

    // 清理：CLI 显式取消。
    harness.cancel_run_cli(&run_id);
    let deadline = Instant::now() + CLIENT_TIMEOUT;
    while harness.app_service.router().supervisor().is_active(&run_id) {
        assert!(Instant::now() < deadline, "Run 应在超时内取消");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

// ---------------------------------------------------------------------------
// 辅助：Ack / Heartbeat 往返
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ack_and_heartbeat_round_trip() {
    let mut harness = Harness::new("contract-hb", CHANNEL).await;
    let client = harness.connect_gui("contract-hb").await;

    assert_eq!(client.heartbeat().await.expect("heartbeat"), 0);
    assert_eq!(
        client
            .heartbeat_with_nonce(42)
            .await
            .expect("heartbeat with nonce"),
        42
    );
    client.ack(GlobalSequence(3)).await.expect("ack");
    assert_eq!(client.last_acked_sequence(), GlobalSequence(3));
    // Ack 无回复：随后 Heartbeat 的 Pong 顺序到达，证明中间无错误帧。
    assert_eq!(
        client
            .heartbeat_with_nonce(43)
            .await
            .expect("heartbeat after ack"),
        43
    );
    assert!(client.is_connected());
    client.close().await.expect("close");
    assert!(!client.is_connected());
}

// ---------------------------------------------------------------------------
// Provider 辅助
// ---------------------------------------------------------------------------

fn streaming_provider() -> Arc<dyn ModelProvider> {
    Arc::new(
        test_support::MockProvider::new(
            test_support::MockScript::new()
                .response_started("contract-r1")
                .text("hello ")
                .text("contract")
                .complete(),
        )
        .with_id(ProviderId::from("mock")),
    )
}
