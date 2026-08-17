use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use pawork_domain::{CommandId, CoreInstanceId, EventId, QueryId, RunId, SessionId, Timestamp};
use pawork_gui_server::{GuiHost, GuiHostError, GuiServer, GuiServerConfig};
use pawork_protocol::{
    decode_server_frame, encode_client_frame, ActorIdentity, ApiVersion, AppCommand,
    AppCommandEnvelope, AppEvent, AppEventEnvelope, AppQuery, AppQueryEnvelope, AppResponse,
    ClientContextSnapshot, ClientFrame, CommandSource, EventSource, EventStream, GlobalSequence,
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
            sections: vec![SnapshotSection {
                kind: SnapshotSectionKind::ActiveRuns,
                revision: 1,
                data: Some(serde_json::json!({"run_ids": []})),
                artifact_id: None,
            }],
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
        Ok(AppResponse::Data(serde_json::json!({"ok": true})))
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
    let host = MockHost::new();
    let handshake = HandshakeService::new(
        host.instance_id(),
        SUPPORTED_API_VERSIONS.to_vec(),
        vec![],
    );
    let transport = Arc::new(LocalTransport::default());
    let server = GuiServer::new(GuiServerConfig {
        host: host.clone(),
        handshake,
        transport: transport.clone(),
        connections: None,
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
    ClientFrame::Handshake(HandshakeRequest {
        request_id: "hs-1".into(),
        client_name: "test-gui".into(),
        client_version: "0.1.0".into(),
        supported_api_versions: vec![API_VERSION],
        capabilities: vec![],
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
                command: AppCommand::CoreInitialize,
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
                command: AppCommand::CoreInitialize,
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
}
