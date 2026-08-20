//! 每连接的握手与帧循环（S10 多客户端路径）。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use async_trait::async_trait;
use pawork_domain::{ActorId, ConnectionId, GuiClientId};
use pawork_protocol::{
    compute_resume_disposition, decode_client_frame_checked, encode_server_frame,
    validate_server_frame_api_version, ActorIdentity, ApiVersion, AppCommandEnvelope,
    AppQuery, AppQueryEnvelope, AppResponseEnvelope, ClientFrame,
    CommandSource, GlobalSequence, GuiCapability, HandshakeRequest, HandshakeResponse,
    HandshakeSession, ProtocolError, ProtocolErrorCode, ProtocolErrorEnvelope, ResumeContext,
    ResumeDisposition, ResumeRequest, ResumeResponse, ServerFrame,
};
use pawork_protocol::app::registry::{command_entry, query_entry, RegistryEntry};
use pawork_protocol::codec::decode_client_frame;
use pawork_transport::{
    ConnectionInfo, GuiConnection, TransportError, TransportErrorKind, TransportFrame,
};
use tokio::sync::{broadcast, mpsc, oneshot, watch};
use tokio::time::{interval, MissedTickBehavior};

use crate::gui_server::connection::{ClientRegistration, ManagerError};
use crate::gui_server::{GuiHostError, Inner};

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
        host_tx: host_tx.clone(),
        close_tx: StdMutex::new(Some(close_tx)),
        info,
        closed: AtomicBool::new(false),
        done_rx: StdMutex::new(done_rx),
    };
    let task = async move {
        run(
            inner,
            connection,
            client_id,
            connection_id,
            host_tx,
            host_rx,
            close_rx,
        )
        .await;
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
    request: HandshakeRequest,
    response: HandshakeResponse,
}

async fn run(
    inner: Arc<Inner>,
    connection: Box<dyn GuiConnection>,
    client_id: GuiClientId,
    connection_id: ConnectionId,
    host_tx: mpsc::UnboundedSender<TransportFrame>,
    mut host_rx: mpsc::UnboundedReceiver<TransportFrame>,
    mut close_rx: oneshot::Receiver<()>,
) {
    let Some(outcome) =
        handshake_phase(&inner, connection.as_ref(), &client_id, &connection_id).await
    else {
        return;
    };
    let negotiated = negotiated_version(&outcome.response);
    let locality = connection.info().locality;

    let registration = ClientRegistration {
        client_id: client_id.clone(),
        connection_id: connection_id.clone(),
        name: outcome.request.client_name,
        version: outcome.request.client_version,
        locality,
        identity: None,
        capabilities: granted_capabilities(&outcome.response),
        connected_at: now_timestamp(),
    };
    let mut event_rx = match inner.connections.register(registration) {
        Ok(receiver) => receiver,
        Err(error) => {
            tracing::warn!(%client_id, %error, "gui client registration failed");
            let _ = connection.close().await;
            return;
        }
    };

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
                inner.connections.unregister(&client_id);
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

    let (stop_tx, stop_rx) = oneshot::channel();
    let _forwarder = spawn_forwarder(
        Arc::clone(&inner),
        client_id.clone(),
        stop_rx,
        host_tx,
    );

    let mut watchdog = interval(watchdog_interval(
        inner.connections.config().heartbeat_timeout,
    ));
    watchdog.set_missed_tick_behavior(MissedTickBehavior::Delay);
    watchdog.tick().await;

    loop {
        tokio::select! {
            biased;
            _ = &mut close_rx => break,
            Some(frame) = host_rx.recv() => {
                if connection.send(frame).await.is_err() {
                    break;
                }
            }
            event = event_rx.recv() => {
                match event {
                    Some(envelope) => {
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
                    None => break,
                }
            }
            _ = watchdog.tick() => {
                if inner.connections.is_timed_out(&client_id, now_timestamp()) {
                    tracing::debug!(
                        %client_id,
                        "gui connection timed out; disconnecting (runs are not cancelled)"
                    );
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
                let _ = inner.connections.heartbeat(&client_id, now_timestamp());
                match handle_frame(&inner, frame, &client_id).await {
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
    inner.connections.unregister(&client_id);
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
    let current = inner.host.current_sequence();
    let earliest_available = inner
        .host
        .earliest_available()
        .unwrap_or(GlobalSequence(0));
    let session = HandshakeSession::new(client_id.clone(), connection_id.clone())
        .with_resume_context(ResumeContext {
            earliest_available,
            current,
        })
        .with_last_global_sequence(
            inner
                .connections
                .last_ack(client_id)
                .unwrap_or(GlobalSequence(0)),
        );
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
    Some(HandshakeOutcome { request, response })
}

enum FrameOutcome {
    None,
    Reply(Vec<ServerFrame>),
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
) -> FrameOutcome {
    match frame {
        ClientFrame::Command(envelope) => {
            let granted = granted_capabilities_for(inner, client_id);
            if let Some(error) =
                gui_channel_gate(command_entry(&envelope.command), &granted, "command")
            {
                return FrameOutcome::Reply(vec![ServerFrame::Error(ProtocolErrorEnvelope {
                    request_id: Some(envelope.command_id.as_str().to_string()),
                    error,
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
            let granted = granted_capabilities_for(inner, client_id);
            if let Some(error) = gui_channel_gate(query_entry(&envelope.query), &granted, "query") {
                return FrameOutcome::Reply(vec![ServerFrame::Error(ProtocolErrorEnvelope {
                    request_id: Some(envelope.request_id.as_str().to_string()),
                    error,
                })]);
            }
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
            match inner.connections.subscribe(
                client_id,
                &request.subscription_id,
                request.streams,
            ) {
                Ok(()) => FrameOutcome::None,
                Err(error) => {
                    FrameOutcome::Reply(vec![manager_error_frame(Some(request.request_id), &error)])
                }
            }
        }
        ClientFrame::Unsubscribe {
            request_id,
            subscription_id,
        } => match inner.connections.unsubscribe(client_id, &subscription_id) {
            Ok(()) => FrameOutcome::None,
            Err(error) => FrameOutcome::Reply(vec![manager_error_frame(Some(request_id), &error)]),
        },
        ClientFrame::Resume(request) => handle_resume(inner, client_id, request).await,
        ClientFrame::SnapshotRequest { request_id } => match inner.host.snapshot().await {
            Ok(snapshot) => FrameOutcome::Reply(vec![ServerFrame::Snapshot(snapshot)]),
            Err(error) => FrameOutcome::Reply(vec![ServerFrame::Error(ProtocolErrorEnvelope {
                request_id: Some(request_id),
                error: host_error_to_protocol(&error),
            })]),
        },
        ClientFrame::Ack { global_sequence } => {
            if let Err(error) = inner.connections.ack(client_id, global_sequence) {
                tracing::debug!(%client_id, %error, "gui ack failed");
            }
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

async fn handle_resume(
    inner: &Inner,
    client_id: &GuiClientId,
    request: ResumeRequest,
) -> FrameOutcome {
    let current = inner.host.current_sequence();
    let earliest = inner
        .host
        .earliest_available()
        .unwrap_or(GlobalSequence(0));
    // Resume 用请求中的 last_global_sequence；为 0 时回落到 Ack 记录，
    // 使 Ack 能影响后续 Resume disposition。
    let last = if request.last_global_sequence.0 == 0 {
        inner
            .connections
            .last_ack(client_id)
            .unwrap_or(GlobalSequence(0))
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
        } => match inner.host.replay(from_sequence, Some(through_sequence)) {
            Ok(events) => replies.extend(events.into_iter().map(ServerFrame::Event)),
            Err(error) => {
                tracing::warn!(%error, "resume replay unavailable; falling back to snapshot");
                replies.clear();
                replies.push(ServerFrame::Resume(ResumeResponse {
                    request_id: request.request_id,
                    disposition: ResumeDisposition::SnapshotRequired {
                        earliest_available_sequence: inner
                            .host
                            .earliest_available()
                            .unwrap_or(GlobalSequence(0)),
                    },
                }));
                if let Ok(snapshot) = inner.host.snapshot().await {
                    replies.push(ServerFrame::Snapshot(snapshot));
                }
            }
        },
        ResumeDisposition::SnapshotRequired { .. } => {
            if let Ok(snapshot) = inner.host.snapshot().await {
                replies.push(ServerFrame::Snapshot(snapshot));
            }
        }
        ResumeDisposition::UpToDate { .. } => {}
    }
    FrameOutcome::Reply(replies)
}

fn spawn_forwarder(
    inner: Arc<Inner>,
    client_id: GuiClientId,
    stop: oneshot::Receiver<()>,
    host_tx: mpsc::UnboundedSender<TransportFrame>,
) -> tokio::task::JoinHandle<()> {
    let mut subscription = inner.host.subscribe_events();
    tokio::spawn(async move {
        let mut stop = stop;
        loop {
            tokio::select! {
                _ = &mut stop => return,
                received = subscription.recv() => {
                    match received {
                        Ok(event) => {
                            if !inner.connections.should_forward(&client_id, &event.stream) {
                                continue;
                            }
                            if let Err(error) = inner.connections.enqueue(&client_id, event) {
                                match error {
                                    ManagerError::Lagged { .. } => {
                                        send_lagged_error(&host_tx, &error);
                                        return;
                                    }
                                    ManagerError::UnknownClient(_) | ManagerError::ChannelClosed(_) => {
                                        return
                                    }
                                    ManagerError::AlreadyRegistered(_) => {
                                        unreachable!("registration happens once at session start")
                                    }
                                }
                            }
                        }
                        Err(broadcast::error::RecvError::Closed) => return,
                        Err(broadcast::error::RecvError::Lagged(_)) => {
                            let error = ManagerError::Lagged {
                                client_id: client_id.clone(),
                            };
                            let _ = inner.connections.mark_lagged(&client_id);
                            send_lagged_error(&host_tx, &error);
                            return;
                        }
                    }
                }
            }
        }
    })
}

fn send_lagged_error(host_tx: &mpsc::UnboundedSender<TransportFrame>, error: &ManagerError) {
    let frame = manager_error_frame(None, error);
    if let Ok(bytes) = encode_server_frame(&frame) {
        let _ = host_tx.send(TransportFrame::new(bytes));
    }
}

fn watchdog_interval(timeout: Duration) -> Duration {
    Duration::from_millis((timeout.as_millis() / 2).max(1) as u64)
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

fn granted_capabilities(response: &HandshakeResponse) -> Vec<GuiCapability> {
    match response {
        HandshakeResponse::Accepted { capabilities, .. } => capabilities.clone(),
        HandshakeResponse::Rejected { .. } => Vec::new(),
    }
}

/// 当前连接被授予的能力集；会话记录缺失时按空集处理（fail-closed）。
fn granted_capabilities_for(inner: &Inner, client_id: &GuiClientId) -> Vec<GuiCapability> {
    inner
        .connections
        .session(client_id)
        .map(|session| session.capabilities)
        .unwrap_or_default()
}

/// Registry 授权门：GUI 不可用或所需能力未授予即在进入 host 前拒绝。
/// 错误形状沿用通道现有 PermissionDenied 帧，不新增协议帧。
fn gui_channel_gate(
    entry: &RegistryEntry,
    granted: &[GuiCapability],
    kind: &str,
) -> Option<ProtocolError> {
    if !entry.gui.available {
        return Some(ProtocolError {
            code: ProtocolErrorCode::PermissionDenied,
            message: format!(
                "{kind} {} is not available on the gui channel",
                entry.wire_name
            ),
            retryable: false,
        });
    }
    if let Some(capability) = entry.gui.required_capability.as_ref() {
        if !granted.contains(capability) {
            return Some(ProtocolError {
                code: ProtocolErrorCode::PermissionDenied,
                message: format!(
                    "capability {capability:?} is not granted to this client for {kind} {}",
                    entry.wire_name
                ),
                retryable: false,
            });
        }
    }
    None
}

fn manager_error_frame(request_id: Option<String>, error: &ManagerError) -> ServerFrame {
    let (code, retryable) = match error {
        ManagerError::UnknownClient(_) => (ProtocolErrorCode::RequestNotFound, false),
        ManagerError::AlreadyRegistered(_) => (ProtocolErrorCode::Internal, false),
        ManagerError::Lagged { .. } => (ProtocolErrorCode::ReplayUnavailable, true),
        ManagerError::ChannelClosed(_) => (ProtocolErrorCode::Internal, false),
    };
    ServerFrame::Error(ProtocolErrorEnvelope {
        request_id,
        error: ProtocolError {
            code,
            message: error.to_string(),
            retryable,
        },
    })
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
