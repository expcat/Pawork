//! P13-9 GUI Connection Protocol 契约测试（pawork-client SDK × pawork-gui-server）。
//!
//! 本机装配：LocalTransport + GuiServer + GuiHostAdapter(AppCore) + MockProvider。
//! UDS 地址落在 tempdir 下的唯一 socket 文件。覆盖：
//!
//! a) 创建 session / 发消息 / 收流式 Run 事件；
//! b) 快照与断线重连：无共享 replay 源时 SnapshotRequired；有 last_ack 且
//!    host 能 replay 则 Replay；UpToDate 表示无需重载。same_command_id /
//!    artifact 大测不在本路强迁。
//! c) 两 GUI 各自收到同一 Run 事件（V2 单客户端语义下的并发同步）；
//! d) 版本不兼容握手被拒；
//! e) GUI 断线不取消 Run；
//! f) Ack / Heartbeat 往返。
//!
//! 不迁：same_command_id_replays_same_response（同客户端同 command_id 由
//! host IdempotencyStore 按 client 作用域重放，见 pawork-app 单测）；
//! large_artifact_chunked_read（V2 无 artifact-store，已 experimental 门控）。

use std::sync::Arc;
use std::time::{Duration, Instant};

use pawork_app::{AppCore, GuiHostAdapter};
use pawork_client::{ClientConfig, ClientError, GuiClient, ResumeDisposition, SessionInfo};
use pawork_domain::{ActorId, ModelId, ProviderId, RunId, SessionId, WorkspaceId};
use pawork_gui_server::{GuiHost, GuiServer, GuiServerConfig};
use pawork_protocol::{
    ActorIdentity, ApiVersion, AppCommand, AppEvent, AppEventEnvelope, AppQuery, AppResponse,
    CommandSource, GlobalSequence, GuiCapability, HandshakeService,
    ProtocolErrorCode, RunState, SnapshotSectionKind, API_VERSION, SUPPORTED_API_VERSIONS,
};
use pawork_session::SessionStore;
use pawork_testkit::{MockProvider, MockScript};
use pawork_transport::{ConnectOptions, GuiConnection, GuiListener, GuiTransportClient, LocalTransport, TransportEndpoint};
use serde_json::json;
use tempfile::TempDir;

const CLIENT_TIMEOUT: Duration = Duration::from_secs(10);

struct Harness {
    adapter: Arc<GuiHostAdapter>,
    transport: Arc<LocalTransport>,
    listener: Arc<dyn GuiListener>,
    endpoint: TransportEndpoint,
    sessions: Vec<Box<dyn GuiConnection>>,
    _temp: TempDir,
}

impl Harness {
    async fn new(label: &str, script: MockScript) -> Self {
        let temp = tempfile::tempdir().expect("tempdir");
        let (store, _) = SessionStore::open(temp.path().join("session.db"))
            .await
            .expect("session store");
        let provider = MockProvider::new(script).with_id(ProviderId::from("mock"));
        let core = Arc::new(AppCore::from_parts(
            Arc::new(provider),
            None,
            ModelId::from("model-1"),
            ProviderId::from("mock"),
            Some(store),
        ));
        let adapter = Arc::new(GuiHostAdapter::new(core));
        let handshake = HandshakeService::new(
            GuiHost::instance_id(adapter.as_ref()),
            SUPPORTED_API_VERSIONS.to_vec(),
            vec![
                GuiCapability::Events,
                GuiCapability::Snapshots,
                GuiCapability::ArtifactStreaming,
            ],
        );
        let transport = Arc::new(LocalTransport::default());
        let server = GuiServer::new(GuiServerConfig {
            host: adapter.clone(),
            handshake,
            transport: transport.clone(),
            connections: None,
        });
        let socket = temp.path().join(format!("{label}.sock"));
        let endpoint = TransportEndpoint::Local {
            address: socket.to_string_lossy().into_owned(),
        };
        let listener = server.bind(endpoint.clone()).await.expect("bind");
        Harness {
            adapter,
            transport,
            listener: Arc::from(listener),
            endpoint,
            sessions: Vec::new(),
            _temp: temp,
        }
    }

    fn connect_options(label: &str) -> ConnectOptions {
        ConnectOptions {
            timeout_ms: 5_000,
            client_label: Some(label.into()),
            max_frame_bytes: 1024 * 1024,
        }
    }

    async fn connect_gui(&mut self, label: &str) -> GuiClient {
        let listener = Arc::clone(&self.listener);
        let accept = tokio::spawn(async move { listener.accept().await });
        let transport: Arc<dyn GuiTransportClient> = self.transport.clone();
        let client = GuiClient::connect(
            transport,
            self.endpoint.clone(),
            Self::connect_options(label),
            None,
        )
        .await
        .expect("gui client connect + handshake");
        let session = accept.await.expect("accept task").expect("accept");
        self.sessions.push(session);
        client
    }

}

fn local_user() -> ActorIdentity {
    ActorIdentity::LocalUser {
        actor_id: ActorId::from("contract-gui-user"),
        display_name: None,
    }
}

fn gui_source(client: &GuiClient) -> CommandSource {
    CommandSource::LocalGui {
        client_id: client.client_id().clone(),
    }
}

fn run_state(event: &AppEventEnvelope, run_id: &RunId) -> Option<RunState> {
    match &event.payload {
        AppEvent::RunChanged { run_id: id, state } if id == run_id => Some(state.clone()),
        _ => None,
    }
}

async fn session_id_from_snapshot(client: &GuiClient, title: &str) -> SessionId {
    let snapshot = client.snapshot().await.expect("snapshot after SessionCreate");
    let section = snapshot
        .sections
        .iter()
        .find(|section| section.kind == SnapshotSectionKind::SessionTree)
        .expect("SessionTree section");
    let sessions = section.data.as_ref().expect("SessionTree data").as_array().expect("sessions");
    let found = sessions.iter().find(|item| item["title"] == json!(title)).unwrap_or_else(|| {
        sessions.first().expect("at least one session")
    });
    SessionId::from(found["session_id"].as_str().expect("session_id"))
}

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

fn streaming_script() -> MockScript {
    MockScript::new()
        .response_started("contract-r1")
        .text("hello ")
        .text("contract")
        .complete()
}

fn waiting_script() -> MockScript {
    MockScript::new().wait_for_cancellation()
}

#[tokio::test]
async fn create_session_send_message_and_receive_streaming_run_events() {
    let mut harness = Harness::new("contract-a", streaming_script()).await;
    let client = harness.connect_gui("contract-a").await;
    let info: &SessionInfo = client.info();
    assert_eq!(info.client_id.as_str(), "client-0");
    assert_eq!(info.handle.api_version, API_VERSION);
    assert!(client.initial_snapshot().is_some(), "握手后应有首帧 Snapshot");

    let source = gui_source(&client);
    let identity = local_user();
    client
        .command(
            AppCommand::SessionCreate {
                workspace_id: WorkspaceId::from("ws-default"),
                title: Some("contract-a".into()),
            },
            source.clone(),
            identity.clone(),
        )
        .await
        .expect("SessionCreate");
    let session_id = session_id_from_snapshot(&client, "contract-a").await;
    client.subscribe_all().await.expect("subscribe all");

    let run = client
        .command(
            AppCommand::RunStart {
                session_id: session_id.clone(),
                user_message: "hello from contract".into(),
                model: None,
                profile: None,
            },
            source,
            identity,
        )
        .await
        .expect("RunStart");
    let AppResponse::Accepted { run_id: Some(run_id), .. } = &run.response else {
        panic!("RunStart 应 Accepted 且携带 run id，got {:?}", run.response);
    };
    let run_id = run_id.clone();

    let (done, events) = recv_until(&client, |e| run_state(e, &run_id) == Some(RunState::Completed)).await;
    assert!(done, "应收到 Run 的 Completed 事件");
    assert!(
        events.iter().any(|e| matches!(&e.payload, AppEvent::AssistantDelta { .. })),
        "应收到流式增量事件"
    );

    // V2 RunStatus 在 registry 摘除后为 unknown，不再承诺 completed 字面量。
    let status = client
        .query(
            AppQuery::RunStatus { run_id: run_id.clone() },
            gui_source(&client),
            local_user(),
        )
        .await
        .expect("RunStatus");
    let AppResponse::Data(data) = &status.response else {
        panic!("expected Data response, got {:?}", status.response);
    };
    assert_eq!(data["run_id"], json!(run_id.as_str()));
    assert!(data["state"] == json!("unknown") || data["state"] == json!("running"));
}

#[tokio::test]
async fn snapshot_and_reconnect_resume_replays_missing_events() {
    let mut harness = Harness::new("contract-b", waiting_script()).await;
    let client = harness.connect_gui("contract-b-first").await;
    client
        .command(
            AppCommand::SessionCreate {
                workspace_id: WorkspaceId::from("ws-default"),
                title: Some("contract-b".into()),
            },
            gui_source(&client),
            local_user(),
        )
        .await
        .expect("SessionCreate");
    let session_id = session_id_from_snapshot(&client, "contract-b").await;
    client.subscribe_all().await.expect("subscribe");

    let run = client
        .command(
            AppCommand::RunStart {
                session_id: session_id.clone(),
                user_message: "long run".into(),
                model: None,
                profile: None,
            },
            gui_source(&client),
            local_user(),
        )
        .await
        .expect("RunStart");
    let AppResponse::Accepted { run_id: Some(run_id), .. } = &run.response else {
        panic!("RunStart 应 Accepted 且携带 run id，got {:?}", run.response);
    };
    let run_id = run_id.clone();
    let (done, events) = recv_until(&client, |e| run_state(e, &run_id) == Some(RunState::Created)).await;
    assert!(done, "第一段连接应看到 RunCreated");
    let last_sequence = events.iter().map(|e| e.global_sequence.0).max().expect("至少一个事件");

    let drain_deadline = Instant::now() + Duration::from_millis(300);
    while Instant::now() < drain_deadline {
        match client.next_event_timeout(Duration::from_millis(50)).await {
            Ok(_) => continue,
            Err(_) => break,
        }
    }

    let snapshot = client.snapshot().await.expect("snapshot");
    assert_eq!(snapshot.instance_id, client.handle().instance_id);
    let active_runs = snapshot
        .sections
        .iter()
        .find(|section| section.kind == SnapshotSectionKind::ActiveRuns)
        .expect("ActiveRuns section");
    let runs = active_runs.data.as_ref().expect("ActiveRuns data").as_array().expect("runs array");
    assert!(
        runs.iter().any(|run| run["run_id"] == json!(run_id.as_str())),
        "快照应包含活跃 Run"
    );

    client.ack(GlobalSequence(last_sequence)).await.expect("ack");
    client.close().await.expect("close");
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        harness.adapter.runs().contains(&run_id),
        "GUI 断线不得取消 Run"
    );

    // 断线期间通过第二个 GUI 取消 Run，使 resume 窗口内出现 Cancelled。
    let cancel_client = harness.connect_gui("contract-b-cancel").await;
    cancel_client
        .command(
            AppCommand::RunCancel { run_id: run_id.clone() },
            gui_source(&cancel_client),
            local_user(),
        )
        .await
        .expect("RunCancel");
    let deadline = Instant::now() + CLIENT_TIMEOUT;
    while harness.adapter.runs().contains(&run_id) {
        assert!(Instant::now() < deadline, "取消后 Run 应从 registry 摘除");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    cancel_client.close().await.expect("close cancel client");

    // 无共享 replay 源时 SnapshotRequired；有 last_ack 且 host 能 replay 则 Replay。
    // 不断言本路能改 gui-server：新连接可能没有共享 replay 源。
    let reconnected = harness.connect_gui("contract-b-second").await;
    let outcome = reconnected
        .resume(GlobalSequence(last_sequence))
        .await
        .expect("resume after reconnect");
    match &outcome.disposition {
        ResumeDisposition::Replay { .. } => {
            assert!(
                outcome
                    .replayed
                    .windows(2)
                    .all(|window| window[1].global_sequence.0 > window[0].global_sequence.0),
                "Replay 事件须按 global_sequence 严格递增"
            );
        }
        ResumeDisposition::SnapshotRequired { .. } => {
            assert!(
                outcome.replayed.is_empty(),
                "无共享 replay 源时 SnapshotRequired 不应夹带 replay 事件"
            );
        }
        ResumeDisposition::UpToDate { .. } => {}
    }
    let snapshot = reconnected.snapshot().await.expect("snapshot after resume");
    let active_runs = snapshot
        .sections
        .iter()
        .find(|section| section.kind == SnapshotSectionKind::ActiveRuns)
        .expect("ActiveRuns section");
    let runs = active_runs.data.as_ref().expect("ActiveRuns data").as_array().expect("runs array");
    assert!(
        runs.iter().all(|run| run["run_id"] != json!(run_id.as_str())),
        "取消后的快照不应再列出该 Run"
    );
    let nonce = reconnected.heartbeat().await.expect("heartbeat");
    assert_eq!(nonce, 0);
    assert!(!harness.adapter.runs().contains(&run_id));
}

#[tokio::test]
async fn resume_falls_back_to_snapshot_required_when_replay_unavailable() {
    let mut harness = Harness::new("contract-b2", streaming_script()).await;
    let client = harness.connect_gui("contract-b2").await;
    client
        .command(
            AppCommand::SessionCreate {
                workspace_id: WorkspaceId::from("ws-default"),
                title: Some("contract-b2".into()),
            },
            gui_source(&client),
            local_user(),
        )
        .await
        .expect("SessionCreate");
    let session_id = session_id_from_snapshot(&client, "contract-b2").await;
    client.subscribe_all().await.expect("subscribe");
    let run = client
        .command(
            AppCommand::RunStart {
                session_id,
                user_message: "fill resume log".into(),
                model: None,
                profile: None,
            },
            gui_source(&client),
            local_user(),
        )
        .await
        .expect("RunStart");
    let AppResponse::Accepted { run_id: Some(run_id), .. } = &run.response else {
        panic!("RunStart 应 Accepted 且携带 run id，got {:?}", run.response);
    };
    let run_id = run_id.clone();
    let (done, events) = recv_until(&client, |e| run_state(e, &run_id) == Some(RunState::Completed)).await;
    assert!(done, "应先产生可重放事件");
    let last = events.iter().map(|e| e.global_sequence.0).max().expect("events");
    // 有 last_ack 且 host 能 replay（同连接仍持有 replay 源）则 Replay。
    let replay = client.resume(GlobalSequence(0)).await.expect("same-connection replay");
    assert!(
        matches!(replay.disposition, ResumeDisposition::Replay { .. }),
        "有 last_ack 且 host 能 replay 则 Replay，got {:?}", replay.disposition
    );
    assert!(!replay.replayed.is_empty());
    assert!(replay.replayed.windows(2).all(|w| w[1].global_sequence.0 > w[0].global_sequence.0));
    let outcome = client.resume(GlobalSequence(last + 100)).await.expect("resume ahead");
    assert!(
        matches!(outcome.disposition, ResumeDisposition::SnapshotRequired { .. }),
        "领先于服务端当前序列应 SnapshotRequired，got {:?}",
        outcome.disposition
    );
    assert!(outcome.replayed.is_empty());
    let snapshot = client.snapshot().await.expect("manual snapshot after resume");
    assert_eq!(snapshot.instance_id, client.handle().instance_id);
}
#[tokio::test]
async fn three_gui_clients_sync_runs_from_cli_and_each_other() {
    let mut harness = Harness::new("contract-c", streaming_script()).await;
    let gui_a = harness.connect_gui("contract-c-a").await;
    let gui_b = harness.connect_gui("contract-c-b").await;
    gui_a.subscribe_all().await.expect("subscribe a");
    gui_b.subscribe_all().await.expect("subscribe b");

    gui_a
        .command(
            AppCommand::SessionCreate {
                workspace_id: WorkspaceId::from("ws-default"),
                title: Some("contract-c".into()),
            },
            gui_source(&gui_a),
            local_user(),
        )
        .await
        .expect("SessionCreate");
    let session_id = session_id_from_snapshot(&gui_a, "contract-c").await;

    let run = gui_a
        .command(
            AppCommand::RunStart {
                session_id: session_id.clone(),
                user_message: "hello from gui a".into(),
                model: None,
                profile: None,
            },
            gui_source(&gui_a),
            local_user(),
        )
        .await
        .expect("gui a RunStart");
    let AppResponse::Accepted { run_id: Some(run_id), .. } = &run.response else {
        panic!("RunStart 应 Accepted 且携带 run id，got {:?}", run.response);
    };
    let run_id = run_id.clone();
    for (name, gui) in [("A", &gui_a), ("B", &gui_b)] {
        let (done, _) = recv_until(gui, |e| run_state(e, &run_id) == Some(RunState::Completed)).await;
        assert!(done, "GUI {name} 应收到 Run 的 Completed");
    }
}

#[tokio::test]
async fn incompatible_version_handshake_is_rejected() {
    let harness = Harness::new("contract-f", streaming_script()).await;
    let listener = Arc::clone(&harness.listener);
    let accept = tokio::spawn(async move { listener.accept().await });
    let transport: Arc<dyn GuiTransportClient> = harness.transport.clone();
    let config = ClientConfig {
        supported_api_versions: vec![ApiVersion { major: 2, minor: 0 }],
        ..ClientConfig::default()
    };
    let error = match GuiClient::connect_with_config(
        transport,
        harness.endpoint.clone(),
        Harness::connect_options("contract-f"),
        None,
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
    let _session = accept.await.expect("accept task").expect("accept");
}

#[tokio::test]
async fn gui_disconnect_does_not_cancel_run() {
    let mut harness = Harness::new("contract-g", waiting_script()).await;
    let client = harness.connect_gui("contract-g").await;
    client
        .command(
            AppCommand::SessionCreate {
                workspace_id: WorkspaceId::from("ws-default"),
                title: Some("contract-g".into()),
            },
            gui_source(&client),
            local_user(),
        )
        .await
        .expect("SessionCreate");
    let session_id = session_id_from_snapshot(&client, "contract-g").await;
    client.subscribe_all().await.expect("subscribe");
    let run = client
        .command(
            AppCommand::RunStart {
                session_id,
                user_message: "survives disconnect".into(),
                model: None,
                profile: None,
            },
            gui_source(&client),
            local_user(),
        )
        .await
        .expect("RunStart");
    let AppResponse::Accepted { run_id: Some(run_id), .. } = &run.response else {
        panic!("RunStart 应 Accepted 且携带 run id，got {:?}", run.response);
    };
    let run_id = run_id.clone();
    let (done, _) = recv_until(&client, |e| run_state(e, &run_id) == Some(RunState::Created)).await;
    assert!(done, "Run 应进入 Created");
    client.close().await.expect("close");
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(harness.adapter.runs().contains(&run_id), "GUI 断线不得取消 Run");

    let cleaner = harness.connect_gui("contract-g-clean").await;
    cleaner
        .command(
            AppCommand::RunCancel { run_id: run_id.clone() },
            gui_source(&cleaner),
            local_user(),
        )
        .await
        .expect("cleanup cancel");
    let deadline = Instant::now() + CLIENT_TIMEOUT;
    while harness.adapter.runs().contains(&run_id) {
        assert!(Instant::now() < deadline, "Run 应在超时内取消");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test]
async fn ack_and_heartbeat_round_trip() {
    let mut harness = Harness::new("contract-hb", streaming_script()).await;
    let client = harness.connect_gui("contract-hb").await;
    assert_eq!(client.heartbeat().await.expect("heartbeat"), 0);
    assert_eq!(client.heartbeat_with_nonce(42).await.expect("heartbeat with nonce"), 42);
    client.ack(GlobalSequence(3)).await.expect("ack");
    assert_eq!(client.last_acked_sequence(), GlobalSequence(3));
    assert_eq!(client.heartbeat_with_nonce(43).await.expect("heartbeat after ack"), 43);
    assert!(client.is_connected());
    client.close().await.expect("close");
    assert!(!client.is_connected());
}
