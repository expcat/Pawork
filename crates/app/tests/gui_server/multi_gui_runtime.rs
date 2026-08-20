//! 多 GUI 运行时：MockHost 覆盖 V1 语义（不绑 AppCore / AppService）。
//!
//! - 三客户端同收事件
//! - 断线 Resume Replay
//! - 落后窗口 SnapshotRequired
//! - 慢客户端队列满不阻塞他人
//! - 断线不取消（host.command 不被乱发 RunCancel）

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use pawork_domain::{CommandId, CoreInstanceId, EventId, GuiClientId, RunId, SessionId, Timestamp};
use pawork_app::gui_server::{
    ConnectionManager, ConnectionManagerConfig, GuiHost, GuiHostError, GuiServer, GuiServerConfig,
};
use pawork_protocol::{
    decode_server_frame, encode_client_frame, ActorIdentity, AppCommand, AppCommandEnvelope,
    AppEvent, AppEventEnvelope, AppQueryEnvelope, AppResponse, ClientFrame, CommandSource,
    EventSource, EventStream, GlobalSequence, GuiCapability, HandshakeRequest, HandshakeResponse,
    HandshakeService, ProtocolErrorCode, ResumeDisposition, ResumeRequest, ServerFrame, Snapshot,
    SnapshotSection, SnapshotSectionKind, SubscribeRequest, TimelineItem, TimelineItemKind,
    TimelinePage, API_VERSION, SUPPORTED_API_VERSIONS,
};
use pawork_transport::{
    ConnectOptions, ConnectionInfo, GuiConnection, GuiListener, GuiTransportClient,
    GuiTransportServer, MemoryTransport, TransportEndpoint, TransportError, TransportFrame,
};
use tokio::sync::broadcast;

const WAIT: Duration = Duration::from_secs(5);

#[derive(Clone)]
struct RecordedCommand {
    command: AppCommand,
}

struct MockHost {
    instance_id: CoreInstanceId,
    events: broadcast::Sender<AppEventEnvelope>,
    ring: Mutex<VecDeque<AppEventEnvelope>>,
    ring_capacity: usize,
    commands: Mutex<Vec<RecordedCommand>>,
    snapshot_seq: AtomicU64,
}

impl MockHost {
    fn new() -> Arc<Self> {
        Self::with_ring_capacity(1024)
    }

    fn with_ring_capacity(ring_capacity: usize) -> Arc<Self> {
        let (events, _) = broadcast::channel(4096);
        Arc::new(Self {
            instance_id: CoreInstanceId::from("multi-gui-instance"),
            events,
            ring: Mutex::new(VecDeque::new()),
            ring_capacity: ring_capacity.max(1),
            commands: Mutex::new(Vec::new()),
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
            snapshot_sequence: self.current_sequence(),
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
        _session_id: &SessionId,
        after: Option<u64>,
        _limit: Option<u32>,
    ) -> Result<TimelinePage, GuiHostError> {
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

    async fn query(&self, _envelope: &AppQueryEnvelope) -> Result<AppResponse, GuiHostError> {
        Ok(AppResponse::Data(serde_json::json!({"ok": true})))
    }

    async fn command(&self, envelope: &AppCommandEnvelope) -> Result<AppResponse, GuiHostError> {
        self.commands.lock().expect("commands").push(RecordedCommand {
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

struct Runtime {
    host: Arc<MockHost>,
    connections: Arc<ConnectionManager>,
    listener: Arc<dyn GuiListener>,
    client_transport: Arc<dyn GuiTransportClient>,
    channel: String,
}

impl Runtime {
    async fn new(label: &str) -> Self {
        Self::new_with(label, MockHost::new(), None, None).await
    }

    async fn new_with(
        label: &str,
        host: Arc<MockHost>,
        connections: Option<Arc<ConnectionManager>>,
        transports: Option<(Arc<dyn GuiTransportServer>, Arc<dyn GuiTransportClient>)>,
    ) -> Self {
        let handshake = HandshakeService::new(
            host.instance_id(),
            SUPPORTED_API_VERSIONS.to_vec(),
            vec![GuiCapability::Events, GuiCapability::Snapshots],
        );
        let memory = Arc::new(MemoryTransport::new());
        let connections =
            connections.unwrap_or_else(|| Arc::new(ConnectionManager::default()));
        let (server_transport, client_transport) = match transports {
            Some(pair) => pair,
            None => (
                memory.clone() as Arc<dyn GuiTransportServer>,
                memory as Arc<dyn GuiTransportClient>,
            ),
        };
        let server = GuiServer::new(GuiServerConfig {
            host: host.clone(),
            handshake,
            transport: server_transport,
            connections: Some(Arc::clone(&connections)),
        });
        let channel = format!("multi-gui-{label}");
        let listener = server
            .bind(TransportEndpoint::Memory {
                channel: channel.clone(),
            })
            .await
            .expect("bind");
        Runtime {
            host,
            connections,
            listener: Arc::from(listener),
            client_transport,
            channel,
        }
    }

    async fn connect_gui(&self) -> TestClient {
        let listener = Arc::clone(&self.listener);
        let accept = tokio::spawn(async move { listener.accept().await });
        let conn = self
            .client_transport
            .connect(
                TransportEndpoint::Memory {
                    channel: self.channel.clone(),
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
        let client = TestClient {
            conn,
            _session: session,
        };
        let _ = client.handshake().await.expect("handshake accepted");
        client
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

    async fn handshake(&self) -> Result<(HandshakeResponse, Snapshot), String> {
        self.send(&ClientFrame::Handshake(HandshakeRequest {
            request_id: "hs".into(),
            client_name: "multi-gui-test".into(),
            client_version: "0.0.1".into(),
            supported_api_versions: vec![API_VERSION],
            capabilities: vec![GuiCapability::Events, GuiCapability::Snapshots],
            authentication: None,
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

    async fn subscribe_all(&self) {
        self.send(&ClientFrame::Subscribe(SubscribeRequest {
            request_id: "sub".into(),
            subscription_id: "all".into(),
            streams: vec![],
        }))
        .await;
    }

    async fn subscribe_all_ready(&self) {
        self.subscribe_all().await;
        self.send(&ClientFrame::Heartbeat { nonce: 7 }).await;
        loop {
            match self.recv().await {
                ServerFrame::Pong { nonce } => {
                    assert_eq!(nonce, 7);
                    break;
                }
                ServerFrame::Event(_) => continue,
                other => panic!("unexpected frame while awaiting subscribe: {other:?}"),
            }
        }
    }

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
                Ok(ServerFrame::Error(envelope))
                    if envelope.error.code == ProtocolErrorCode::ReplayUnavailable =>
                {
                    break;
                }
                Ok(other) => panic!("unexpected frame while awaiting events: {other:?}"),
                Err(_) => break,
            }
        }
        (received, false)
    }
}

fn event(seq: u64) -> AppEventEnvelope {
    AppEventEnvelope {
        api_version: API_VERSION,
        instance_id: CoreInstanceId::from("multi-gui-instance"),
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

struct SlowTransport {
    inner: MemoryTransport,
    block_first: Arc<AtomicUsize>,
    block_after: Arc<AtomicUsize>,
    released: Arc<AtomicBool>,
    unblock: Arc<tokio::sync::Notify>,
}

impl SlowTransport {
    fn new(inner: MemoryTransport, block_first: usize, block_after: usize) -> Self {
        Self {
            inner,
            block_first: Arc::new(AtomicUsize::new(block_first)),
            block_after: Arc::new(AtomicUsize::new(block_after)),
            released: Arc::new(AtomicBool::new(false)),
            unblock: Arc::new(tokio::sync::Notify::new()),
        }
    }

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

    fn info(&self) -> ConnectionInfo {
        self.inner.info()
    }
}

#[tokio::test]
async fn three_guis_receive_the_same_events() {
    let runtime = Runtime::new("three").await;
    let gui_a = runtime.connect_gui().await;
    let gui_b = runtime.connect_gui().await;
    let gui_c = runtime.connect_gui().await;
    assert_eq!(runtime.connections.count(), 3);
    gui_a.subscribe_all_ready().await;
    gui_b.subscribe_all_ready().await;
    gui_c.subscribe_all_ready().await;

    for seq in 1..=3u64 {
        runtime.host.publish(event(seq));
    }

    let (_a, a_done) = gui_a
        .recv_until(|e| e.global_sequence == GlobalSequence(3), WAIT)
        .await;
    let (_b, b_done) = gui_b
        .recv_until(|e| e.global_sequence == GlobalSequence(3), WAIT)
        .await;
    let (events_c, c_done) = gui_c
        .recv_until(|e| e.global_sequence == GlobalSequence(3), WAIT)
        .await;
    assert!(a_done, "GUI A 应收到 seq=3");
    assert!(b_done, "GUI B 应收到 seq=3");
    assert!(c_done, "GUI C 应收到 seq=3");
    assert!(
        events_c
            .iter()
            .any(|e| e.stream == EventStream::Run(RunId::from("run-1"))),
        "GUI C 的事件应属于该 Run 流"
    );
}

#[tokio::test]
async fn reconnect_replays_missing_events() {
    let runtime = Runtime::new("replay").await;
    let old = runtime.connect_gui().await;
    old.subscribe_all_ready().await;
    for seq in 1..=3u64 {
        runtime.host.publish(event(seq));
    }
    let (events, saw) = old
        .recv_until(|e| e.global_sequence == GlobalSequence(3), WAIT)
        .await;
    assert!(saw, "第一段连接应收到 seq=3");
    let last_sequence = events
        .iter()
        .map(|e| e.global_sequence.0)
        .max()
        .expect("至少一个事件");
    old.send(&ClientFrame::Ack {
        global_sequence: GlobalSequence(last_sequence),
    })
    .await;
    old.conn.close().await.expect("close");

    for seq in 4..=6u64 {
        runtime.host.publish(event(seq));
    }

    let fresh = runtime.connect_gui().await;
    let current_before = runtime.host.current_sequence().0;
    fresh
        .send(&ClientFrame::Resume(ResumeRequest {
            request_id: "resume-1".into(),
            last_global_sequence: GlobalSequence(last_sequence),
        }))
        .await;

    let mut replayed = Vec::new();
    let deadline = Instant::now() + WAIT;
    let mut saw_replay = false;
    while Instant::now() < deadline {
        match tokio::time::timeout(deadline.saturating_duration_since(Instant::now()), fresh.recv())
            .await
        {
            Ok(ServerFrame::Resume(response)) => {
                assert_eq!(response.request_id, "resume-1");
                match response.disposition {
                    ResumeDisposition::Replay {
                        from_sequence,
                        through_sequence,
                    } => {
                        assert_eq!(from_sequence.0, last_sequence + 1);
                        assert!(through_sequence.0 >= current_before);
                        saw_replay = true;
                    }
                    other => panic!("Ring 窗口内应 Replay，got {other:?}"),
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
        if saw_replay && replayed.len() >= 3 {
            break;
        }
    }
    assert!(saw_replay, "应收到 Replay disposition");
    assert!(!replayed.is_empty(), "断线期间应有缺失事件被 Replay 补发");
    let sequences: Vec<u64> = replayed.iter().map(|e| e.global_sequence.0).collect();
    for window in sequences.windows(2) {
        assert!(window[1] > window[0], "重放事件序列应严格递增");
    }
}

#[tokio::test]
async fn resume_falls_back_to_snapshot_when_replay_unavailable() {
    let host = MockHost::with_ring_capacity(2);
    let runtime = Runtime::new_with("fallback", host, None, None).await;
    for i in 1..=20u64 {
        runtime.host.publish(event(i));
    }
    let earliest = runtime.host.earliest_available().expect("earliest").0;
    assert!(earliest > 2, "ring 应已淘汰早期事件（earliest={earliest}）");

    let gui = runtime.connect_gui().await;
    gui.send(&ClientFrame::Resume(ResumeRequest {
        request_id: "resume-fallback".into(),
        last_global_sequence: GlobalSequence(1),
    }))
    .await;

    let mut saw_fallback = false;
    let mut saw_snapshot = false;
    let deadline = Instant::now() + WAIT;
    while Instant::now() < deadline {
        match tokio::time::timeout(deadline.saturating_duration_since(Instant::now()), gui.recv())
            .await
        {
            Ok(ServerFrame::Resume(response)) => {
                assert_eq!(response.request_id, "resume-fallback");
                assert!(
                    matches!(
                        response.disposition,
                        ResumeDisposition::SnapshotRequired { .. }
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
        if saw_fallback && saw_snapshot {
            break;
        }
    }
    assert!(saw_fallback, "应收到 SnapshotRequired 降级响应");
    assert!(saw_snapshot, "降级后应补发 Snapshot");
}

#[tokio::test]
async fn slow_client_does_not_block_other_guis() {
    let memory = MemoryTransport::new();
    let slow_transport = Arc::new(SlowTransport::new(memory.clone(), 1, 2));
    let connections = Arc::new(ConnectionManager::with_config(ConnectionManagerConfig {
        heartbeat_timeout: Duration::from_secs(30),
        queue_capacity: 8,
    }));
    let runtime = Runtime::new_with(
        "slow",
        MockHost::new(),
        Some(connections),
        Some((
            slow_transport.clone() as Arc<dyn GuiTransportServer>,
            Arc::new(memory) as Arc<dyn GuiTransportClient>,
        )),
    )
    .await;

    let slow = runtime.connect_gui().await;
    slow.subscribe_all().await;
    tokio::time::sleep(Duration::from_millis(20)).await;
    let fast = runtime.connect_gui().await;
    fast.subscribe_all_ready().await;

    let fast_drain = tokio::spawn(async move {
        fast.recv_until(|e| e.global_sequence == GlobalSequence(5), WAIT)
            .await
    });
    tokio::time::sleep(Duration::from_millis(20)).await;
    let started = Instant::now();
    for seq in 1..=40u64 {
        runtime.host.publish(event(seq));
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    assert!(
        started.elapsed() < Duration::from_millis(200),
        "publish 不得被慢客户端阻塞"
    );

    let (_events, done) = fast_drain.await.expect("fast drain task");
    assert!(done, "快客户端应收到事件，不被慢客户端阻塞");

    let deadline = Instant::now() + WAIT;
    while Instant::now() < deadline {
        if runtime
            .connections
            .session(&GuiClientId::from("client-0"))
            .is_some_and(|session| session.lagged)
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let session = runtime
        .connections
        .session(&GuiClientId::from("client-0"))
        .expect("slow session 仍在登记");
    assert!(session.lagged, "慢客户端应被标记 Lagged");

    slow.conn.close().await.expect("close slow");
    slow_transport.release();
    let deadline = Instant::now() + WAIT;
    while Instant::now() < deadline {
        if runtime.connections.count() == 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(runtime.connections.count(), 1, "慢客户端断线后应注销");
}

#[tokio::test]
async fn disconnect_does_not_issue_run_cancel() {
    let runtime = Runtime::new("disc").await;
    let gui = runtime.connect_gui().await;
    gui.send(&ClientFrame::Command(AppCommandEnvelope {
        api_version: API_VERSION,
        command_id: CommandId::from("cmd-keep"),
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
    let ServerFrame::Response(_) = gui.recv().await else {
        panic!("expected command response");
    };
    gui.conn.close().await.expect("client close");
    tokio::time::sleep(Duration::from_millis(80)).await;
    let commands = runtime.host.recorded_commands();
    assert!(
        commands
            .iter()
            .all(|item| !matches!(item.command, AppCommand::RunCancel { .. })),
        "disconnect must not issue RunCancel"
    );
}

#[tokio::test]
async fn heartbeat_timeout_disconnects_without_run_cancel() {
    let connections = Arc::new(ConnectionManager::with_config(ConnectionManagerConfig {
        heartbeat_timeout: Duration::from_millis(80),
        queue_capacity: 16,
    }));
    let runtime = Runtime::new_with("hb-timeout", MockHost::new(), Some(connections), None).await;
    let _gui = runtime.connect_gui().await;
    assert_eq!(runtime.connections.count(), 1);

    let deadline = Instant::now() + WAIT;
    while Instant::now() < deadline {
        if runtime.connections.count() == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(
        runtime.connections.count(),
        0,
        "心跳超时只应 unregister/断线"
    );
    assert!(
        runtime
            .host
            .recorded_commands()
            .iter()
            .all(|item| !matches!(item.command, AppCommand::RunCancel { .. })),
        "心跳超时不得乱发 RunCancel"
    );
}
