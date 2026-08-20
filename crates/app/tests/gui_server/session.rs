use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use pawork_domain::{
    CommandId, CoreInstanceId, EventId, QueryId, RunId, SessionId, Timestamp, ToolCallId,
    WorkspaceId,
};
use pawork_app::gui_server::{
    ConnectionManager, ConnectionManagerConfig, GuiHost, GuiHostError, GuiServer, GuiServerConfig,
};
use pawork_protocol::{
    decode_server_frame, encode_client_frame, ActorIdentity, ApiVersion, AppCommand,
    AppCommandEnvelope, AppEvent, AppEventEnvelope, AppQuery, AppQueryEnvelope, AppResponse,
    ClientContextSnapshot, ClientFrame, CommandSource, EventSource, EventStream, GlobalSequence,
    ApprovalDecision, GuiCapability,
    HandshakeRequest, HandshakeResponse, HandshakeService, ProtocolErrorCode, ResumeDisposition,
    ResumeRequest, ServerFrame, Snapshot, SnapshotSection, SnapshotSectionKind, SubscribeRequest,
    TimelineItem,
    TimelineItemKind, TimelinePage, API_VERSION, SUPPORTED_API_VERSIONS,
};
use pawork_transport::{
    ConnectOptions, GuiConnection, GuiListener, GuiTransportClient, LocalTransport,
    TransportEndpoint, TransportError, TransportErrorKind, TransportFrame,
};
use tokio::sync::broadcast;

#[derive(Clone)]
struct RecordedCommand {
    source: CommandSource,
    identity: ActorIdentity,
    command: AppCommand,
}

#[derive(Clone)]
struct RecordedQuery {
    query: AppQuery,
    source: CommandSource,
    identity: ActorIdentity,
}

struct MockHost {
    instance_id: CoreInstanceId,
    events: broadcast::Sender<AppEventEnvelope>,
    ring: Mutex<VecDeque<AppEventEnvelope>>,
    ring_capacity: usize,
    commands: Mutex<Vec<RecordedCommand>>,
    queries: Mutex<Vec<RecordedQuery>>,
    timelines: Mutex<Vec<(SessionId, Option<u64>, Option<u32>)>>,
    snapshot_seq: AtomicU64,
}

impl MockHost {
    fn new() -> Arc<Self> {
        Self::with_ring_capacity(1024)
    }

    fn with_ring_capacity(ring_capacity: usize) -> Arc<Self> {
        let (events, _) = broadcast::channel(256);
        Arc::new(Self {
            instance_id: CoreInstanceId::from("gui-server-test"),
            events,
            ring: Mutex::new(VecDeque::new()),
            ring_capacity: ring_capacity.max(1),
            commands: Mutex::new(Vec::new()),
            queries: Mutex::new(Vec::new()),
            timelines: Mutex::new(Vec::new()),
            snapshot_seq: AtomicU64::new(1),
        })
    }

    fn publish(&self, envelope: AppEventEnvelope) {
        {
            let mut ring = self.ring.lock().expect("ring");
            if ring.len() == self.ring_capacity {
                ring.pop_front();
            }
            ring.push_back(envelope.clone());
        }
        let _ = self.events.send(envelope);
    }

    fn recorded_commands(&self) -> Vec<RecordedCommand> {
        self.commands.lock().expect("commands").clone()
    }

    fn recorded_timelines(&self) -> Vec<(SessionId, Option<u64>, Option<u32>)> {
        self.timelines.lock().expect("timelines").clone()
    }

    fn recorded_queries(&self) -> Vec<RecordedQuery> {
        self.queries.lock().expect("queries").clone()
    }
}

#[async_trait]
impl GuiHost for MockHost {
    fn instance_id(&self) -> CoreInstanceId {
        self.instance_id.clone()
    }

    async fn snapshot(&self) -> Result<Snapshot, GuiHostError> {
        let seq = self.snapshot_seq.fetch_add(1, Ordering::Relaxed);
        Ok(Snapshot {
            instance_id: self.instance_id.clone(),
            snapshot_sequence: GlobalSequence(seq),
            generated_at: Timestamp::from_unix_millis(seq),
            sections: vec![
                SnapshotSection {
                kind: SnapshotSectionKind::ActiveRuns,
                revision: 1,
                data: Some(serde_json::json!({"run_ids": []})),
                artifact_id: None,
                },
                SnapshotSection {
                    kind: SnapshotSectionKind::TerminalSessions,
                    revision: 2,
                    data: Some(serde_json::json!([{
                        "terminal_session_id": "terminal-secret",
                        "owner_session": "session-secret",
                        "state": "running"
                    }])),
                    artifact_id: None,
                },
            ],
        })
    }

    async fn timeline(
        &self,
        session_id: &SessionId,
        after: Option<u64>,
        limit: Option<u32>,
    ) -> Result<TimelinePage, GuiHostError> {
        self.timelines.lock().expect("timelines").push((
            session_id.clone(),
            after,
            limit,
        ));
        Ok(TimelinePage {
            items: vec![TimelineItem {
                sequence: after.unwrap_or(0) + 1,
                event_id: "event-page".into(),
                kind: TimelineItemKind::UserMessage,
                run_id: None,
                text: Some("hello".into()),
                tool_name: None,
                status: None,
                detail: None,
                timestamp: "2026-01-01T00:00:00Z".into(),
            }],
            next_sequence: Some(after.unwrap_or(0) + 2),
            head_sequence: 20,
            complete: false,
        })
    }

    async fn query(&self, envelope: &AppQueryEnvelope) -> Result<AppResponse, GuiHostError> {
        self.queries.lock().expect("queries").push(RecordedQuery {
            query: envelope.query.clone(),
            source: envelope.source.clone(),
            identity: envelope.identity.clone(),
        });
        // 镜像真实 GuiHost::query：SessionGet 带分页参数时由 query 内部执行
        // timeline 分页（R3 波 C 移除了 server 层丢弃结果的预调用）。
        match &envelope.query {
            AppQuery::SessionGet {
                session_id,
                timeline_after_sequence,
                timeline_limit,
            } if timeline_after_sequence.is_some() || timeline_limit.is_some() => {
                let page = self
                    .timeline(session_id, *timeline_after_sequence, *timeline_limit)
                    .await?;
                let mut data = serde_json::json!({"ok": true});
                data["timeline_page"] = serde_json::to_value(page).expect("timeline page json");
                Ok(AppResponse::Data(data))
            }
            _ => Ok(AppResponse::Data(serde_json::json!({"ok": true}))),
        }
    }

    async fn command(&self, envelope: &AppCommandEnvelope) -> Result<AppResponse, GuiHostError> {
        self.commands.lock().expect("commands").push(RecordedCommand {
            source: envelope.source.clone(),
            identity: envelope.identity.clone(),
            command: envelope.command.clone(),
        });
        Ok(AppResponse::Accepted {
            command_id: envelope.command_id.clone(),
            run_id: None,
        })
    }

    fn subscribe_events(&self) -> broadcast::Receiver<AppEventEnvelope> {
        self.events.subscribe()
    }

    fn current_sequence(&self) -> GlobalSequence {
        self.ring
            .lock()
            .expect("ring")
            .back()
            .map(|event| event.global_sequence)
            .unwrap_or(GlobalSequence(0))
    }

    fn earliest_available(&self) -> Option<GlobalSequence> {
        self.ring
            .lock()
            .expect("ring")
            .front()
            .map(|event| event.global_sequence)
    }

    fn replay(
        &self,
        from: GlobalSequence,
        through: Option<GlobalSequence>,
    ) -> Result<Vec<AppEventEnvelope>, GuiHostError> {
        let ring = self.ring.lock().expect("ring");
        let through = through.unwrap_or_else(|| {
            ring.back()
                .map(|event| event.global_sequence)
                .unwrap_or(GlobalSequence(0))
        });
        if let Some(earliest) = ring.front() {
            if from < earliest.global_sequence {
                return Err(GuiHostError {
                    code: "replay_unavailable".into(),
                    message: format!(
                        "replay from {} is before earliest {}",
                        from.0, earliest.global_sequence.0
                    ),
                    retryable: false,
                });
            }
        }
        Ok(ring
            .iter()
            .filter(|event| event.global_sequence >= from && event.global_sequence <= through)
            .cloned()
            .collect())
    }
}

struct Client {
    conn: Box<dyn GuiConnection>,
}

impl Client {
    async fn send(&self, frame: &ClientFrame) {
        self.conn
            .send(TransportFrame::new(
                encode_client_frame(frame).expect("encode"),
            ))
            .await
            .expect("send");
    }

    async fn recv(&self) -> ServerFrame {
        decode_server_frame(self.conn.receive().await.expect("recv").as_bytes()).expect("decode")
    }
}

struct Harness {
    host: Arc<MockHost>,
    client: Client,
    _listener: Box<dyn GuiListener>,
    _session: Box<dyn GuiConnection>,
}

async fn open_harness(label: &str) -> Harness {
    open_harness_with_connections(label, None).await
}

async fn open_harness_with_connections(
    label: &str,
    connections: Option<Arc<ConnectionManager>>,
) -> Harness {
    open_harness_with_capabilities(
        label,
        connections,
        vec![GuiCapability::Events, GuiCapability::Snapshots],
    )
    .await
}

async fn open_harness_with_capabilities(
    label: &str,
    connections: Option<Arc<ConnectionManager>>,
    supported_capabilities: Vec<GuiCapability>,
) -> Harness {
    let host = MockHost::new();
    let handshake = HandshakeService::new(
        host.instance_id(),
        SUPPORTED_API_VERSIONS.to_vec(),
        supported_capabilities,
    );
    let transport = Arc::new(LocalTransport::default());
    let server = GuiServer::new(GuiServerConfig {
        host: host.clone(),
        handshake,
        transport: transport.clone(),
        connections,
    });
    let temp = tempfile::tempdir().expect("tempdir");
    let socket = temp.path().join(format!("{label}.sock"));
    let endpoint = TransportEndpoint::Local {
        address: socket.to_string_lossy().into_owned(),
    };
    let listener = server.bind(endpoint.clone()).await.expect("bind");
    let accept = tokio::spawn({
        // keep listener alive across accept by moving boxed listener into task? we'll accept then keep.
        async move { listener.accept().await }
    });
    let conn = transport
        .connect(
            endpoint,
            ConnectOptions {
                timeout_ms: 5_000,
                client_label: None,
                max_frame_bytes: 1024 * 1024,
            },
        )
        .await
        .expect("connect");
    let session = accept.await.expect("accept task").expect("accept");
    // leak tempdir until process ends for this test process lifetime
    std::mem::forget(temp);
    Harness {
        host,
        client: Client { conn },
        _listener: {
            // listener already moved; create dummy closed? We don't need it after accept.
            // reconstruct is hard; use a no-op listener stub.
            Box::new(ClosedListener)
        },
        _session: session,
    }
}

struct ClosedListener;

#[async_trait]
impl GuiListener for ClosedListener {
    async fn accept(&self) -> Result<Box<dyn GuiConnection>, TransportError> {
        Err(TransportError {
            kind: TransportErrorKind::ConnectionClosed,
            message: "test listener stub".into(),
            retryable: false,
        })
    }
    async fn close(&self) -> Result<(), TransportError> {
        Ok(())
    }
}

fn handshake_frame() -> ClientFrame {
    handshake_frame_with_capabilities(vec![GuiCapability::Events, GuiCapability::Snapshots])
}

fn handshake_frame_with_capabilities(capabilities: Vec<GuiCapability>) -> ClientFrame {
    ClientFrame::Handshake(HandshakeRequest {
        request_id: "hs-1".into(),
        client_name: "test-gui".into(),
        client_version: "0.1.0".into(),
        supported_api_versions: vec![API_VERSION],
        capabilities,
        authentication: None,
    })
}

fn subscribe_all() -> ClientFrame {
    ClientFrame::Subscribe(SubscribeRequest {
        request_id: "sub".into(),
        subscription_id: "all".into(),
        streams: vec![],
    })
}

async fn handshake_and_snapshot(client: &Client) -> (HandshakeResponse, Snapshot) {
    client.send(&handshake_frame()).await;
    let ServerFrame::Handshake(response) = client.recv().await else {
        panic!("expected handshake");
    };
    let ServerFrame::Snapshot(snapshot) = client.recv().await else {
        panic!("expected snapshot after handshake");
    };
    client.send(&subscribe_all()).await;
    client.send(&ClientFrame::Heartbeat { nonce: 1 }).await;
    loop {
        match client.recv().await {
            ServerFrame::Pong { nonce } => {
                assert_eq!(nonce, 1);
                break;
            }
            ServerFrame::Event(_) => continue,
            other => panic!("unexpected frame while awaiting subscribe ack: {other:?}"),
        }
    }
    (response, snapshot)
}

fn event(seq: u64) -> AppEventEnvelope {
    AppEventEnvelope {
        api_version: API_VERSION,
        instance_id: CoreInstanceId::from("gui-server-test"),
        event_id: EventId::from(format!("event-{seq}")),
        global_sequence: GlobalSequence(seq),
        stream: EventStream::Run(RunId::from("run-1")),
        stream_sequence: seq,
        timestamp: Timestamp::from_unix_millis(seq),
        source: EventSource::Core,
        payload: AppEvent::RunChanged {
            run_id: RunId::from("run-1"),
            state: pawork_protocol::RunState::StreamingResponse,
        },
    }
}

#[cfg(unix)]
mod unix_tests {
    use super::*;

    #[tokio::test]
    async fn handshake_round_trip_then_snapshot() {
        let harness = open_harness("hs").await;
        let (response, snapshot) = handshake_and_snapshot(&harness.client).await;
        assert!(matches!(response, HandshakeResponse::Accepted { selected_api_version, .. } if selected_api_version == API_VERSION));
        assert_eq!(snapshot.instance_id.as_str(), "gui-server-test");
    }

    #[tokio::test]
    async fn non_handshake_first_frame_is_rejected_and_closed() {
        let harness = open_harness("bad-first").await;
        harness.client.send(&ClientFrame::Heartbeat { nonce: 1 }).await;
        let ServerFrame::Error(envelope) = harness.client.recv().await else {
            panic!("expected error");
        };
        assert_eq!(envelope.error.code, ProtocolErrorCode::InvalidFrame);
        let error = harness
            .client
            .conn
            .receive()
            .await
            .expect_err("server should close");
        assert_eq!(error.kind, TransportErrorKind::ConnectionClosed);
    }

    #[tokio::test]
    async fn command_is_stamped_and_version_checked() {
        let harness = open_harness("stamp").await;
        let _ = handshake_and_snapshot(&harness.client).await;
        harness
            .client
            .send(&ClientFrame::Command(AppCommandEnvelope {
                api_version: API_VERSION,
                command_id: CommandId::from("cmd-1"),
                source: CommandSource::Automation,
                identity: ActorIdentity::System,
                expected_revision: None,
                idempotency_key: None,
                issued_at: Timestamp::from_unix_millis(1),
                command: AppCommand::SessionOpen {
                    session_id: SessionId::from("session-1"),
                },
            }))
            .await;
        let ServerFrame::Response(envelope) = harness.client.recv().await else {
            panic!("expected response");
        };
        assert!(matches!(envelope.response, AppResponse::Accepted { .. }));
        let recorded = harness.host.recorded_commands();
        assert_eq!(recorded.len(), 1);
        assert!(matches!(
            recorded[0].source,
            CommandSource::LocalGui { ref client_id } if client_id.as_str() == "client-0"
        ));
        assert!(matches!(
            recorded[0].identity,
            ActorIdentity::LocalUser { ref actor_id, .. } if actor_id.as_str() == "client-0"
        ));

        harness
            .client
            .send(&ClientFrame::Command(AppCommandEnvelope {
                api_version: ApiVersion::new(2, 0),
                command_id: CommandId::from("cmd-bad"),
                source: CommandSource::Automation,
                identity: ActorIdentity::System,
                expected_revision: None,
                idempotency_key: None,
                issued_at: Timestamp::from_unix_millis(2),
                command: AppCommand::SessionOpen {
                    session_id: SessionId::from("session-1"),
                },
            }))
            .await;
        let ServerFrame::Error(error) = harness.client.recv().await else {
            panic!("expected version error");
        };
        assert_eq!(error.error.code, ProtocolErrorCode::IncompatibleVersion);
    }

    #[tokio::test]
    async fn session_get_timeline_fields_are_forwarded() {
        let harness = open_harness("timeline").await;
        let _ = handshake_and_snapshot(&harness.client).await;
        harness
            .client
            .send(&ClientFrame::Query(AppQueryEnvelope {
                api_version: API_VERSION,
                request_id: QueryId::from("q-1"),
                source: CommandSource::Automation,
                identity: ActorIdentity::System,
                issued_at: Timestamp::from_unix_millis(1),
                query: AppQuery::SessionGet {
                    session_id: SessionId::from("session-1"),
                    timeline_after_sequence: Some(10),
                    timeline_limit: Some(25),
                },
            }))
            .await;
        let ServerFrame::Response(_) = harness.client.recv().await else {
            panic!("expected query response");
        };
        let timelines = harness.host.recorded_timelines();
        assert_eq!(timelines.len(), 1);
        assert_eq!(timelines[0].0.as_str(), "session-1");
        assert_eq!(timelines[0].1, Some(10));
        assert_eq!(timelines[0].2, Some(25));
    }

    #[tokio::test]
    async fn resume_three_states_and_ack_influence() {
        let harness = open_harness("resume").await;
        let _ = handshake_and_snapshot(&harness.client).await;

        harness
            .client
            .send(&ClientFrame::Resume(ResumeRequest {
                request_id: "r-empty".into(),
                last_global_sequence: GlobalSequence(0),
            }))
            .await;
        let ServerFrame::Resume(resume) = harness.client.recv().await else {
            panic!("expected resume");
        };
        assert!(matches!(resume.disposition, ResumeDisposition::UpToDate { .. }));

        for seq in 1..=3 {
            harness.host.publish(event(seq));
            let ServerFrame::Event(envelope) = harness.client.recv().await else {
                panic!("expected live event {seq}");
            };
            assert_eq!(envelope.global_sequence, GlobalSequence(seq));
        }

        harness
            .client
            .send(&ClientFrame::Resume(ResumeRequest {
                request_id: "r-replay".into(),
                last_global_sequence: GlobalSequence(1),
            }))
            .await;
        let ServerFrame::Resume(resume) = harness.client.recv().await else {
            panic!("expected replay resume");
        };
        assert!(matches!(
            resume.disposition,
            ResumeDisposition::Replay {
                from_sequence,
                through_sequence
            } if from_sequence == GlobalSequence(2) && through_sequence == GlobalSequence(3)
        ));
        for expected in [2, 3] {
            let ServerFrame::Event(envelope) = harness.client.recv().await else {
                panic!("expected replayed event");
            };
            assert_eq!(envelope.global_sequence, GlobalSequence(expected));
        }

        harness
            .client
            .send(&ClientFrame::Ack {
                global_sequence: GlobalSequence(3),
            })
            .await;
        harness.client.send(&ClientFrame::Heartbeat { nonce: 9 }).await;
        assert_eq!(harness.client.recv().await, ServerFrame::Pong { nonce: 9 });
        harness
            .client
            .send(&ClientFrame::Resume(ResumeRequest {
                request_id: "r-ack".into(),
                last_global_sequence: GlobalSequence(0),
            }))
            .await;
        let ServerFrame::Resume(resume) = harness.client.recv().await else {
            panic!("expected ack-influenced resume");
        };
        assert!(matches!(resume.disposition, ResumeDisposition::UpToDate { .. }));

        harness
            .client
            .send(&ClientFrame::Resume(ResumeRequest {
                request_id: "r-snap".into(),
                last_global_sequence: GlobalSequence(99),
            }))
            .await;
        let ServerFrame::Resume(resume) = harness.client.recv().await else {
            panic!("expected snapshot-required resume");
        };
        assert!(matches!(
            resume.disposition,
            ResumeDisposition::SnapshotRequired { .. }
        ));
    }

    #[tokio::test]
    async fn heartbeat_gets_pong() {
        let harness = open_harness("hb").await;
        let _ = handshake_and_snapshot(&harness.client).await;
        harness.client.send(&ClientFrame::Heartbeat { nonce: 42 }).await;
        assert_eq!(harness.client.recv().await, ServerFrame::Pong { nonce: 42 });
    }

    #[tokio::test]
    async fn disconnect_does_not_cancel_run() {
        let harness = open_harness("disc").await;
        let _ = handshake_and_snapshot(&harness.client).await;
        harness.client.conn.close().await.expect("client close");
        tokio::time::sleep(Duration::from_millis(50)).await;
        let commands = harness.host.recorded_commands();
        assert!(
            commands.iter().all(|item| !matches!(item.command, AppCommand::RunCancel { .. })),
            "disconnect must not issue RunCancel"
        );
    }

    #[tokio::test]
    async fn lagged_queue_sends_replay_unavailable() {
        let connections = Arc::new(ConnectionManager::with_config(ConnectionManagerConfig {
            heartbeat_timeout: Duration::from_secs(30),
            queue_capacity: 2,
        }));
        let harness = open_harness_with_connections("lagged", Some(connections)).await;
        let _ = handshake_and_snapshot(&harness.client).await;
        for seq in 1..=128u64 {
            harness.host.publish(event(seq));
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        let mut saw_replay_unavailable = false;
        let mut disconnected = false;
        while tokio::time::Instant::now() < deadline {
            match tokio::time::timeout(Duration::from_millis(200), harness.client.conn.receive())
                .await
            {
                Ok(Ok(bytes)) => match decode_server_frame(bytes.as_bytes()).expect("decode") {
                    ServerFrame::Error(envelope) => {
                        assert_eq!(envelope.error.code, ProtocolErrorCode::ReplayUnavailable);
                        saw_replay_unavailable = true;
                        break;
                    }
                    ServerFrame::Event(_) => continue,
                    other => panic!("unexpected frame while waiting for Lagged: {other:?}"),
                },
                Ok(Err(error)) => {
                    disconnected = error.kind == TransportErrorKind::ConnectionClosed;
                    break;
                }
                Err(_) => continue,
            }
        }
        assert!(
            saw_replay_unavailable || disconnected,
            "expected ReplayUnavailable or connection close after overflowing queue_capacity=2"
        );
    }

    #[tokio::test]
    async fn slow_consumer_does_not_block_host() {
        let harness = open_harness("slow").await;
        let _ = handshake_and_snapshot(&harness.client).await;
        for seq in 1..=80u64 {
            harness.host.publish(event(seq));
        }
        harness.host.publish(event(81));
        // host publish must return immediately even if client is not reading.
        let started = std::time::Instant::now();
        harness.host.publish(event(82));
        assert!(started.elapsed() < Duration::from_millis(100));
        harness.client.send(&ClientFrame::Heartbeat { nonce: 1 }).await;
        // drain until pong arrives; live events may have been dropped from the bounded queue.
        loop {
            match harness.client.recv().await {
                ServerFrame::Pong { nonce } => {
                    assert_eq!(nonce, 1);
                    break;
                }
                ServerFrame::Event(_) => continue,
                ServerFrame::Error(envelope)
                    if envelope.error.code == ProtocolErrorCode::ReplayUnavailable =>
                {
                    break;
                }
                other => panic!("unexpected {other:?}"),
            }
        }
    }

    #[tokio::test]
    async fn session_client_context_replace_is_rejected() {
        let harness = open_harness("ctx").await;
        let _ = handshake_and_snapshot(&harness.client).await;
        harness
            .client
            .send(&ClientFrame::Command(AppCommandEnvelope {
                api_version: API_VERSION,
                command_id: CommandId::from("ctx-1"),
                source: CommandSource::Automation,
                identity: ActorIdentity::System,
                expected_revision: None,
                idempotency_key: None,
                issued_at: Timestamp::from_unix_millis(1),
                command: AppCommand::SessionClientContextReplace {
                    session_id: SessionId::from("session-1"),
                    snapshot: ClientContextSnapshot {
                        revision: 1,
                        active_document: None,
                        open_documents: Vec::new(),
                        diagnostics: Vec::new(),
                    },
                },
            }))
            .await;
        let ServerFrame::Error(error) = harness.client.recv().await else {
            panic!("expected structured error");
        };
        assert_eq!(error.error.code, ProtocolErrorCode::PermissionDenied);
        assert!(harness.host.recorded_commands().is_empty());
    }

    /// R3 波 A 授权门（未授予分支）：本 harness 握手请求能力为空，服务端
    /// supported 集亦为空，granted 集必然为空。Terminal*（TerminalStreaming）、
    /// ToolApprove（Approvals）、SnapshotFetch（Snapshots）必须在进入 host 前
    /// 被 PermissionDenied 拒绝，host 侧零记录（fail-closed）。
    #[tokio::test]
    async fn capability_required_requests_are_denied_before_host_when_not_granted() {
        let harness = open_harness_with_capabilities("caps", None, vec![]).await;
        harness
            .client
            .send(&handshake_frame_with_capabilities(vec![]))
            .await;
        let ServerFrame::Handshake(handshake) = harness.client.recv().await else {
            panic!("expected handshake accepted");
        };
        let HandshakeResponse::Accepted {
            capabilities: granted, ..
        } = handshake
        else {
            panic!("expected handshake accepted");
        };
        assert!(granted.is_empty(), "precondition: no capabilities granted");
        assert!(!granted.contains(&GuiCapability::TerminalStreaming));
        assert!(!granted.contains(&GuiCapability::Approvals));
        assert!(!granted.contains(&GuiCapability::Snapshots));

        // Events / Snapshots 是连接层内禀能力；未授予时专用帧也必须拒绝，
        // 且握手后不得先泄漏一帧 Snapshot。
        harness.client.send(&subscribe_all()).await;
        let ServerFrame::Error(subscribe) = harness.client.recv().await else {
            panic!("expected PermissionDenied for event subscription");
        };
        assert_eq!(subscribe.error.code, ProtocolErrorCode::PermissionDenied);
        harness
            .client
            .send(&ClientFrame::SnapshotRequest {
                request_id: "snapshot-frame-denied".into(),
            })
            .await;
        let ServerFrame::Error(snapshot_frame) = harness.client.recv().await else {
            panic!("expected PermissionDenied for snapshot request");
        };
        assert_eq!(
            snapshot_frame.error.code,
            ProtocolErrorCode::PermissionDenied
        );

        // terminal_create：需要 TerminalStreaming。
        harness
            .client
            .send(&ClientFrame::Command(AppCommandEnvelope {
                api_version: API_VERSION,
                command_id: CommandId::from("terminal-denied"),
                source: CommandSource::Automation,
                identity: ActorIdentity::System,
                expected_revision: None,
                idempotency_key: None,
                issued_at: Timestamp::from_unix_millis(1),
                command: AppCommand::TerminalCreate {
                    workspace_id: WorkspaceId::from("ws-1"),
                    working_directory: None,
                },
            }))
            .await;
        let ServerFrame::Error(terminal) = harness.client.recv().await else {
            panic!("expected PermissionDenied for terminal_create");
        };
        assert_eq!(terminal.error.code, ProtocolErrorCode::PermissionDenied);

        // tool_approve：需要 Approvals。
        harness
            .client
            .send(&ClientFrame::Command(AppCommandEnvelope {
                api_version: API_VERSION,
                command_id: CommandId::from("approve-denied"),
                source: CommandSource::Automation,
                identity: ActorIdentity::System,
                expected_revision: None,
                idempotency_key: None,
                issued_at: Timestamp::from_unix_millis(2),
                command: AppCommand::ToolApprove {
                    run_id: RunId::from("run-1"),
                    tool_call_id: ToolCallId::from("call-1"),
                    decision: ApprovalDecision::ApproveOnce,
                },
            }))
            .await;
        let ServerFrame::Error(approve) = harness.client.recv().await else {
            panic!("expected PermissionDenied for tool_approve");
        };
        assert_eq!(approve.error.code, ProtocolErrorCode::PermissionDenied);

        // snapshot_fetch（query）：需要 Snapshots。
        harness
            .client
            .send(&ClientFrame::Query(AppQueryEnvelope {
                api_version: API_VERSION,
                request_id: QueryId::from("snapshot-denied"),
                source: CommandSource::Automation,
                identity: ActorIdentity::System,
                issued_at: Timestamp::from_unix_millis(3),
                query: AppQuery::SnapshotFetch,
            }))
            .await;
        let ServerFrame::Error(snapshot) = harness.client.recv().await else {
            panic!("expected PermissionDenied for snapshot_fetch");
        };
        assert_eq!(snapshot.error.code, ProtocolErrorCode::PermissionDenied);

        // 授权门在进入 host 之前生效：命令与查询均零记录。
        assert!(harness.host.recorded_commands().is_empty());
        assert!(harness.host.recorded_queries().is_empty());
    }

    #[tokio::test]
    async fn terminal_snapshot_sections_require_terminal_streaming_capability_on_all_paths() {
        let without = open_harness("snapshot-terminal-denied").await;
        without.client.send(&handshake_frame()).await;
        let ServerFrame::Handshake(_) = without.client.recv().await else {
            panic!("expected handshake accepted");
        };
        let ServerFrame::Snapshot(initial) = without.client.recv().await else {
            panic!("expected initial snapshot");
        };
        assert!(!initial
            .sections
            .iter()
            .any(|section| section.kind == SnapshotSectionKind::TerminalSessions));

        without
            .client
            .send(&ClientFrame::SnapshotRequest {
                request_id: "snapshot-filtered".into(),
            })
            .await;
        let ServerFrame::Snapshot(requested) = without.client.recv().await else {
            panic!("expected requested snapshot");
        };
        assert!(!requested
            .sections
            .iter()
            .any(|section| section.kind == SnapshotSectionKind::TerminalSessions));

        without
            .client
            .send(&ClientFrame::Resume(ResumeRequest {
                request_id: "resume-filtered".into(),
                last_global_sequence: GlobalSequence(99),
            }))
            .await;
        let ServerFrame::Resume(resume) = without.client.recv().await else {
            panic!("expected resume response");
        };
        assert!(matches!(
            resume.disposition,
            ResumeDisposition::SnapshotRequired { .. }
        ));
        let ServerFrame::Snapshot(resumed) = without.client.recv().await else {
            panic!("expected resume snapshot");
        };
        assert!(!resumed
            .sections
            .iter()
            .any(|section| section.kind == SnapshotSectionKind::TerminalSessions));

        let with = open_harness_with_capabilities(
            "snapshot-terminal-granted",
            None,
            vec![GuiCapability::Snapshots, GuiCapability::TerminalStreaming],
        )
        .await;
        with.client
            .send(&handshake_frame_with_capabilities(vec![
                GuiCapability::Snapshots,
                GuiCapability::TerminalStreaming,
            ]))
            .await;
        let ServerFrame::Handshake(_) = with.client.recv().await else {
            panic!("expected handshake accepted");
        };
        let ServerFrame::Snapshot(initial) = with.client.recv().await else {
            panic!("expected initial snapshot");
        };
        assert!(initial
            .sections
            .iter()
            .any(|section| section.kind == SnapshotSectionKind::TerminalSessions));
    }

    #[tokio::test]
    async fn terminal_events_require_terminal_streaming_capability() {
        let harness = open_harness("terminal-event-gate").await;
        let (handshake, _) = handshake_and_snapshot(&harness.client).await;
        let HandshakeResponse::Accepted {
            capabilities: granted,
            ..
        } = handshake
        else {
            panic!("expected handshake accepted");
        };
        assert!(granted.contains(&GuiCapability::Events));
        assert!(!granted.contains(&GuiCapability::TerminalStreaming));

        let mut terminal = event(1);
        terminal.stream = EventStream::Terminal("terminal-1".into());
        terminal.payload = AppEvent::TerminalOutput {
            terminal_session_id: "terminal-1".into(),
            delta: "secret terminal output".into(),
        };
        harness.host.publish(terminal);
        harness.host.publish(event(2));

        let ServerFrame::Event(visible) = harness.client.recv().await else {
            panic!("expected non-terminal event");
        };
        assert_eq!(visible.global_sequence, GlobalSequence(2));
        assert!(!matches!(visible.stream, EventStream::Terminal(_)));
    }
}
