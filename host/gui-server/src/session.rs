//! 每连接的握手与帧循环（S7 单客户端路径）。

use std::collections::{HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex};

use async_trait::async_trait;
use pawork_domain::{ActorId, ConnectionId, GuiClientId};
use pawork_protocol::{
    compute_resume_disposition, decode_client_frame_checked, encode_server_frame,
    validate_server_frame_api_version, ActorIdentity, ApiVersion, AppCommand, AppCommandEnvelope,
    AppEventEnvelope, AppQuery, AppQueryEnvelope, AppResponseEnvelope, ClientFrame,
    CommandSource, GlobalSequence, HandshakeResponse, HandshakeSession,
    ProtocolError, ProtocolErrorCode, ProtocolErrorEnvelope, ResumeContext, ResumeDisposition,
    ResumeRequest, ResumeResponse, ServerFrame,
};
use pawork_protocol::codec::decode_client_frame;
use pawork_transport::{
    ConnectionInfo, GuiConnection, TransportError, TransportErrorKind,
    TransportFrame,
};
use tokio::sync::{broadcast, mpsc, oneshot, watch};

use crate::{GuiHostError, Inner};

const EVENT_QUEUE_CAPACITY: usize = 64;
const RESUME_LOG_CAPACITY: usize = 1024;

pub(super) fn spawn(
    inner: Arc<Inner>,
    connection: Box<dyn GuiConnection>,
    client_id: GuiClientId,
    connection_id: ConnectionId,
) -> (SessionHandle, impl std::future::Future<Output = ()> + Send) {
    let (host_tx, host_rx) = mpsc::unbounded_channel::<TransportFrame>();
    let (close_tx, close_rx) = oneshot::channel::<()>();
    let info = connection.info();
    let (done_tx, done_rx) = watch::channel(false);
    let handle = SessionHandle {
        host_tx,
        close_tx: StdMutex::new(Some(close_tx)),
        info,
        closed: AtomicBool::new(false),
        done_rx: StdMutex::new(done_rx),
    };
    let task = async move {
        run(inner, connection, client_id, connection_id, host_rx, close_rx).await;
        let _ = done_tx.send(true);
    };
    (handle, task)
}

pub(super) struct SessionHandle {
    host_tx: mpsc::UnboundedSender<TransportFrame>,
    close_tx: StdMutex<Option<oneshot::Sender<()>>>,
    info: ConnectionInfo,
    closed: AtomicBool,
    done_rx: StdMutex<watch::Receiver<bool>>,
}

#[async_trait]
impl GuiConnection for SessionHandle {
    async fn send(&self, frame: TransportFrame) -> Result<(), TransportError> {
        if self.closed.load(Ordering::Acquire) {
            return Err(connection_closed("connection is closed"));
        }
        self.host_tx.send(frame).map_err(|_| {
            self.closed.store(true, Ordering::Release);
            connection_closed("connection task has ended")
        })
    }

    async fn receive(&self) -> Result<TransportFrame, TransportError> {
        let mut done = self.done_rx.lock().expect("done rx lock").clone();
        if !*done.borrow() {
            let _ = done.changed().await;
        }
        Err(connection_closed("connection task has ended"))
    }

    async fn close(&self) -> Result<(), TransportError> {
        if self.closed.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        if let Some(tx) = self.close_tx.lock().expect("close tx lock").take() {
            let _ = tx.send(());
        }
        Ok(())
    }

    fn info(&self) -> ConnectionInfo {
        self.info.clone()
    }
}

struct HandshakeOutcome {
    response: HandshakeResponse,
}

async fn run(
    inner: Arc<Inner>,
    connection: Box<dyn GuiConnection>,
    client_id: GuiClientId,
    connection_id: ConnectionId,
    mut host_rx: mpsc::UnboundedReceiver<TransportFrame>,
    mut close_rx: oneshot::Receiver<()>,
) {
    let Some(outcome) =
        handshake_phase(&inner, connection.as_ref(), &client_id, &connection_id).await
    else {
        return;
    };
    let negotiated = negotiated_version(&outcome.response);

    let mut session = SessionState::new();
    match inner.host.snapshot().await {
        Ok(snapshot) => {
            if send_frame(
                connection.as_ref(),
                &ServerFrame::Snapshot(snapshot),
                Some(negotiated),
            )
            .await
            .is_err()
            {
                let _ = connection.close().await;
                return;
            }
        }
        Err(error) => {
            tracing::warn!(%client_id, %error, "initial snapshot failed");
            let _ = send_frame(
                connection.as_ref(),
                &ServerFrame::Error(ProtocolErrorEnvelope {
                    request_id: None,
                    error: host_error_to_protocol(&error),
                }),
                Some(negotiated),
            )
            .await;
        }
    }

    let event_queue = BoundedEventQueue::new(EVENT_QUEUE_CAPACITY);
    let (stop_tx, stop_rx) = oneshot::channel();
    let _forwarder = spawn_forwarder(
        inner.host.subscribe_events(),
        event_queue.clone(),
        session.resume_log.clone(),
        stop_rx,
    );

    loop {
        tokio::select! {
            biased;
            _ = &mut close_rx => break,
            Some(frame) = host_rx.recv() => {
                if connection.send(frame).await.is_err() {
                    break;
                }
            }
            event = event_queue.recv() => {
                let Some(envelope) = event else { break };
                if send_frame(
                    connection.as_ref(),
                    &ServerFrame::Event(envelope),
                    Some(negotiated),
                )
                .await
                .is_err()
                {
                    break;
                }
            }
            received = connection.receive() => {
                let bytes = match received {
                    Ok(bytes) => bytes,
                    Err(error) => {
                        tracing::debug!(%client_id, "gui connection receive ended: {error}");
                        break;
                    }
                };
                let frame = match decode_client_frame_checked(bytes.as_bytes(), negotiated) {
                    Ok(frame) => frame,
                    Err(protocol_error) => {
                        let _ = send_frame(
                            connection.as_ref(),
                            &ServerFrame::Error(ProtocolErrorEnvelope {
                                request_id: None,
                                error: protocol_error,
                            }),
                            Some(negotiated),
                        )
                        .await;
                        break;
                    }
                };
                match handle_frame(
                    &inner,
                    frame,
                    &client_id,
                    &mut session,
                )
                .await
                {
                    FrameOutcome::None => {}
                    FrameOutcome::Reply(replies) => {
                        let mut sent = true;
                        for reply in replies {
                            if send_frame(connection.as_ref(), &reply, Some(negotiated))
                                .await
                                .is_err()
                            {
                                sent = false;
                                break;
                            }
                        }
                        if !sent {
                            break;
                        }
                    }
                }
            }
        }
    }
    let _ = stop_tx.send(());
    let _ = connection.close().await;
}

async fn handshake_phase(
    inner: &Inner,
    connection: &dyn GuiConnection,
    client_id: &GuiClientId,
    connection_id: &ConnectionId,
) -> Option<HandshakeOutcome> {
    let bytes = match connection.receive().await {
        Ok(bytes) => bytes,
        Err(error) => {
            tracing::debug!(%client_id, "gui connection ended before handshake: {error}");
            return None;
        }
    };
    let frame = match decode_client_frame(bytes.as_bytes()) {
        Ok(frame) => frame,
        Err(error) => {
            let protocol_error: ProtocolError = error.into();
            let _ = send_frame(
                connection,
                &ServerFrame::Error(ProtocolErrorEnvelope {
                    request_id: None,
                    error: protocol_error,
                }),
                None,
            )
            .await;
            let _ = connection.close().await;
            return None;
        }
    };
    let ClientFrame::Handshake(request) = frame else {
        let _ = send_frame(
            connection,
            &ServerFrame::Error(ProtocolErrorEnvelope {
                request_id: None,
                error: ProtocolError::invalid_frame("first frame must be ClientFrame::Handshake"),
            }),
            None,
        )
        .await;
        let _ = connection.close().await;
        return None;
    };
    let session = HandshakeSession::new(client_id.clone(), connection_id.clone())
        .with_resume_context(ResumeContext {
            earliest_available: GlobalSequence(0),
            current: GlobalSequence(0),
        })
        .with_last_global_sequence(GlobalSequence(0));
    let response = inner.handshake.accept(&request, session);
    let negotiated = match &response {
        HandshakeResponse::Accepted {
            selected_api_version,
            ..
        } => Some(*selected_api_version),
        HandshakeResponse::Rejected { .. } => None,
    };
    if send_frame(
        connection,
        &ServerFrame::Handshake(response.clone()),
        negotiated,
    )
    .await
    .is_err()
    {
        return None;
    }
    if negotiated.is_none() {
        tracing::debug!(%client_id, "gui handshake rejected");
        let _ = connection.close().await;
        return None;
    }
    Some(HandshakeOutcome { response })
}

enum FrameOutcome {
    None,
    Reply(Vec<ServerFrame>),
}

#[derive(Clone)]
struct ResumeLog {
    events: Arc<StdMutex<VecDeque<AppEventEnvelope>>>,
    last_forwarded: Arc<StdMutex<Option<GlobalSequence>>>,
    dropped: Arc<StdMutex<u64>>,
}

impl ResumeLog {
    fn new() -> Self {
        Self {
            events: Arc::new(StdMutex::new(VecDeque::new())),
            last_forwarded: Arc::new(StdMutex::new(None)),
            dropped: Arc::new(StdMutex::new(0)),
        }
    }

    fn record(&self, envelope: AppEventEnvelope) {
        let mut last = self.last_forwarded.lock().expect("last forwarded");
        if let Some(previous) = *last {
            if !envelope.global_sequence.is_immediately_after(previous) {
                tracing::warn!(
                    previous = previous.0,
                    current = envelope.global_sequence.0,
                    "gui-server saw non-contiguous global_sequence; not reordering"
                );
            }
        }
        *last = Some(envelope.global_sequence);
        drop(last);
        let mut log = self.events.lock().expect("resume log");
        if log.len() == RESUME_LOG_CAPACITY {
            log.pop_front();
        }
        log.push_back(envelope);
    }

    fn earliest_available(&self) -> GlobalSequence {
        self.events
            .lock()
            .expect("resume log")
            .front()
            .map(|event| event.global_sequence)
            .unwrap_or(GlobalSequence(0))
    }

    fn current(&self) -> GlobalSequence {
        self.events
            .lock()
            .expect("resume log")
            .back()
            .map(|event| event.global_sequence)
            .unwrap_or(GlobalSequence(0))
    }

    fn replay(&self, from: GlobalSequence, through: GlobalSequence) -> Vec<AppEventEnvelope> {
        self.events
            .lock()
            .expect("resume log")
            .iter()
            .filter(|event| event.global_sequence.0 >= from.0 && event.global_sequence.0 <= through.0)
            .cloned()
            .collect()
    }
}

struct SessionState {
    subscriptions: HashSet<String>,
    last_ack: GlobalSequence,
    resume_log: ResumeLog,
}

impl SessionState {
    fn new() -> Self {
        Self {
            subscriptions: HashSet::new(),
            last_ack: GlobalSequence(0),
            resume_log: ResumeLog::new(),
        }
    }

    fn earliest_available(&self) -> GlobalSequence {
        self.resume_log.earliest_available()
    }

    fn current(&self) -> GlobalSequence {
        self.resume_log.current()
    }

    fn replay(&self, from: GlobalSequence, through: GlobalSequence) -> Vec<AppEventEnvelope> {
        self.resume_log.replay(from, through)
    }
}

fn host_stamp_command(
    mut envelope: AppCommandEnvelope,
    client_id: &GuiClientId,
) -> AppCommandEnvelope {
    envelope.source = CommandSource::LocalGui {
        client_id: client_id.clone(),
    };
    envelope.identity = ActorIdentity::LocalUser {
        actor_id: ActorId::from(client_id.as_str()),
        display_name: None,
    };
    envelope
}

fn host_stamp_query(
    mut envelope: AppQueryEnvelope,
    client_id: &GuiClientId,
) -> AppQueryEnvelope {
    envelope.source = CommandSource::LocalGui {
        client_id: client_id.clone(),
    };
    envelope.identity = ActorIdentity::LocalUser {
        actor_id: ActorId::from(client_id.as_str()),
        display_name: None,
    };
    envelope
}

async fn handle_frame(
    inner: &Inner,
    frame: ClientFrame,
    client_id: &GuiClientId,
    session: &mut SessionState,
) -> FrameOutcome {
    match frame {
        ClientFrame::Command(envelope) => {
            if matches!(
                envelope.command,
                AppCommand::SessionClientContextReplace { .. }
            ) {
                return FrameOutcome::Reply(vec![ServerFrame::Error(ProtocolErrorEnvelope {
                    request_id: Some(envelope.command_id.as_str().to_string()),
                    error: ProtocolError {
                        code: ProtocolErrorCode::PermissionDenied,
                        message: "session_client_context_replace is not supported in S7"
                            .into(),
                        retryable: false,
                    },
                })]);
            }
            let stamped = host_stamp_command(envelope, client_id);
            match inner.host.command(&stamped).await {
                Ok(response) => FrameOutcome::Reply(vec![ServerFrame::Response(
                    AppResponseEnvelope {
                        api_version: stamped.api_version,
                        request_id: pawork_domain::QueryId::from(stamped.command_id.as_str()),
                        responded_at: now_timestamp(),
                        response,
                    },
                )]),
                Err(error) => FrameOutcome::Reply(vec![ServerFrame::Error(ProtocolErrorEnvelope {
                    request_id: Some(stamped.command_id.as_str().to_string()),
                    error: host_error_to_protocol(&error),
                })]),
            }
        }
        ClientFrame::Query(envelope) => {
            let stamped = host_stamp_query(envelope, client_id);
            match &stamped.query {
                AppQuery::SessionGet {
                    session_id,
                    timeline_after_sequence,
                    timeline_limit,
                } => {
                    let _ = inner
                        .host
                        .timeline(session_id, *timeline_after_sequence, *timeline_limit)
                        .await;
                }
                _ => {}
            }
            match inner.host.query(&stamped).await {
                Ok(response) => FrameOutcome::Reply(vec![ServerFrame::Response(
                    AppResponseEnvelope {
                        api_version: stamped.api_version,
                        request_id: stamped.request_id.clone(),
                        responded_at: now_timestamp(),
                        response,
                    },
                )]),
                Err(error) => FrameOutcome::Reply(vec![ServerFrame::Error(ProtocolErrorEnvelope {
                    request_id: Some(stamped.request_id.as_str().to_string()),
                    error: host_error_to_protocol(&error),
                })]),
            }
        }
        ClientFrame::ArtifactRead(request) => FrameOutcome::Reply(vec![ServerFrame::Error(
            ProtocolErrorEnvelope {
                request_id: Some(request.request_id),
                error: ProtocolError {
                    code: ProtocolErrorCode::RequestNotFound,
                    message: "artifact streaming is unsupported until S10".into(),
                    retryable: false,
                },
            },
        )]),
        ClientFrame::Heartbeat { nonce } => FrameOutcome::Reply(vec![ServerFrame::Pong { nonce }]),
        ClientFrame::Pong { .. } => FrameOutcome::None,
        ClientFrame::Subscribe(request) => {
            session.subscriptions.insert(request.subscription_id);
            FrameOutcome::None
        }
        ClientFrame::Unsubscribe {
            subscription_id, ..
        } => {
            session.subscriptions.remove(&subscription_id);
            FrameOutcome::None
        }
        ClientFrame::Resume(request) => handle_resume(session, request),
        ClientFrame::SnapshotRequest { request_id } => match inner.host.snapshot().await {
            Ok(snapshot) => FrameOutcome::Reply(vec![ServerFrame::Snapshot(snapshot)]),
            Err(error) => FrameOutcome::Reply(vec![ServerFrame::Error(ProtocolErrorEnvelope {
                request_id: Some(request_id),
                error: host_error_to_protocol(&error),
            })]),
        },
        ClientFrame::Ack { global_sequence } => {
            session.last_ack = global_sequence;
            FrameOutcome::None
        }
        ClientFrame::Handshake(_) => {
            FrameOutcome::Reply(vec![ServerFrame::Error(ProtocolErrorEnvelope {
                request_id: None,
                error: ProtocolError::invalid_frame("handshake already completed"),
            })])
        }
    }
}

fn handle_resume(session: &SessionState, request: ResumeRequest) -> FrameOutcome {
    let current = session.current();
    let earliest = session.earliest_available();
    // Resume 用请求中的 last_global_sequence；为 0 时回落到 Ack 记录，
    // 使 Ack 能影响后续 Resume disposition。
    let last = if request.last_global_sequence.0 == 0 && session.last_ack.0 != 0 {
        session.last_ack
    } else {
        request.last_global_sequence
    };
    let disposition = compute_resume_disposition(earliest, current, last);
    let mut replies = vec![ServerFrame::Resume(ResumeResponse {
        request_id: request.request_id.clone(),
        disposition: disposition.clone(),
    })];
    match disposition {
        ResumeDisposition::Replay {
            from_sequence,
            through_sequence,
        } => {
            replies.extend(
                session
                    .replay(from_sequence, through_sequence)
                    .into_iter()
                    .map(ServerFrame::Event),
            );
        }
        ResumeDisposition::SnapshotRequired { .. } | ResumeDisposition::UpToDate { .. } => {}
    }
    FrameOutcome::Reply(replies)
}

#[derive(Clone)]
struct BoundedEventQueue {
    items: Arc<StdMutex<Option<VecDeque<AppEventEnvelope>>>>,
    notify: Arc<tokio::sync::Notify>,
    capacity: usize,
    dropped: Arc<StdMutex<u64>>,
}

impl BoundedEventQueue {
    fn new(capacity: usize) -> Self {
        Self {
            items: Arc::new(StdMutex::new(Some(VecDeque::new()))),
            notify: Arc::new(tokio::sync::Notify::new()),
            capacity,
            dropped: Arc::new(StdMutex::new(0)),
        }
    }

    fn push(&self, event: AppEventEnvelope) -> bool {
        let mut guard = self.items.lock().expect("event queue");
        let Some(items) = guard.as_mut() else {
            return false;
        };
        if items.len() == self.capacity {
            items.pop_front();
            if let Ok(mut count) = self.dropped.lock() {
                *count += 1;
            }
        }
        items.push_back(event);
        self.notify.notify_one();
        true
    }

    fn close(&self) {
        *self.items.lock().expect("event queue") = None;
        self.notify.notify_waiters();
    }

    async fn recv(&self) -> Option<AppEventEnvelope> {
        loop {
            {
                let mut guard = self.items.lock().expect("event queue");
                match guard.as_mut() {
                    Some(items) => {
                        if let Some(event) = items.pop_front() {
                            return Some(event);
                        }
                    }
                    None => return None,
                }
            }
            self.notify.notified().await;
        }
    }
}

fn spawn_forwarder(
    mut subscription: broadcast::Receiver<AppEventEnvelope>,
    event_queue: BoundedEventQueue,
    resume_log: ResumeLog,
    stop: oneshot::Receiver<()>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut stop = stop;
        loop {
            tokio::select! {
                _ = &mut stop => {
                    event_queue.close();
                    return;
                }
                received = subscription.recv() => {
                    match received {
                        Ok(event) => {
                            // 双写：环形日志始终记录；有界队列满则丢最旧，不阻塞 host。
                            resume_log.record(event.clone());
                            if !event_queue.push(event) {
                                return;
                            }
                        }
                        Err(broadcast::error::RecvError::Closed) => {
                            event_queue.close();
                            return;
                        }
                        Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    }
                }
            }
        }
    })
}

fn negotiated_version(response: &HandshakeResponse) -> ApiVersion {
    match response {
        HandshakeResponse::Accepted {
            selected_api_version,
            ..
        } => *selected_api_version,
        HandshakeResponse::Rejected { .. } => {
            unreachable!("handshake_phase only returns accepted outcomes")
        }
    }
}

fn host_error_to_protocol(error: &GuiHostError) -> ProtocolError {
    ProtocolError {
        code: ProtocolErrorCode::Internal,
        message: error.message.clone(),
        retryable: error.retryable,
    }
}

async fn send_frame(
    connection: &dyn GuiConnection,
    frame: &ServerFrame,
    negotiated: Option<ApiVersion>,
) -> Result<(), TransportError> {
    if let Some(negotiated) = negotiated {
        validate_server_frame_api_version(frame, negotiated).map_err(|error| TransportError {
            kind: TransportErrorKind::Internal,
            message: error.message,
            retryable: false,
        })?;
    }
    let bytes = encode_server_frame(frame).map_err(|error| TransportError {
        kind: TransportErrorKind::Internal,
        message: error.to_string(),
        retryable: false,
    })?;
    connection.send(TransportFrame::new(bytes)).await
}

fn connection_closed(message: &str) -> TransportError {
    TransportError {
        kind: TransportErrorKind::ConnectionClosed,
        message: message.into(),
        retryable: false,
    }
}

fn now_timestamp() -> pawork_domain::Timestamp {
    pawork_domain::Timestamp::from_unix_millis(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_millis() as u64)
            .unwrap_or(0),
    )
}
