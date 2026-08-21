//! 每连接的握手与帧循环（S10 多客户端路径）。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use async_trait::async_trait;
use pawork_domain::{ActorId, ConnectionId, GuiClientId};
use pawork_protocol::{
    compute_resume_disposition, decode_client_frame_checked, encode_server_frame,
    validate_server_frame_api_version, ActorIdentity, ApiVersion, AppCommandEnvelope,
    AppQueryEnvelope, AppResponseEnvelope, ClientFrame, CommandSource, EventStream,
    GlobalSequence, GuiCapability, HandshakeRequest, HandshakeResponse, HandshakeSession,
    ProtocolError, ProtocolErrorCode, ProtocolErrorEnvelope, ResumeContext, ResumeDisposition,
    ResumeRequest, ResumeResponse, ServerFrame, SnapshotSectionKind,
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
        if let Err(error) = done_tx.send(true) {
            tracing::debug!(error = ?error, "gui session done signal dropped");
        }
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
            if let Err(error) = done.changed().await {
                tracing::debug!(%error, "gui session done watch closed");
            }
        }
        Err(connection_closed("connection task has ended"))
    }

    async fn close(&self) -> Result<(), TransportError> {
        if self.closed.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        if let Some(tx) = self.close_tx.lock().expect("close tx lock").take() {
            if let Err(error) = tx.send(()) {
                tracing::debug!(error = ?error, "gui session close signal dropped");
            }
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

    let granted = granted_capabilities(&outcome.response);
    let registration = ClientRegistration {
        client_id: client_id.clone(),
        connection_id: connection_id.clone(),
        name: outcome.request.client_name,
        version: outcome.request.client_version,
        locality,
        identity: None,
        capabilities: granted.clone(),
        connected_at: now_timestamp(),
    };
    let mut event_rx = match inner.connections.register(registration) {
        Ok(receiver) => receiver,
        Err(error) => {
            tracing::warn!(%client_id, %error, "gui client registration failed");
            if let Err(error) = connection.close().await {
                tracing::debug!(%client_id, %error, "gui connection close failed after registration error");
            }
            return;
        }
    };

    if granted.contains(&GuiCapability::Snapshots) {
        match snapshot_for_client(inner.as_ref(), &client_id).await {
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
                if let Err(error) = connection.close().await {
                    tracing::debug!(%client_id, %error, "gui connection close failed after snapshot send error");
                }
                return;
            }
        }
        Err(error) => {
            tracing::warn!(%client_id, %error, "initial snapshot failed");
            if let Err(error) = send_frame(
                connection.as_ref(),
                &ServerFrame::Error(ProtocolErrorEnvelope {
                    request_id: None,
                    error: host_error_to_protocol(&error),
                }),
                Some(negotiated),
            )
            .await
            {
                tracing::debug!(%client_id, %error, "gui snapshot error frame dropped");
            }
        }
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
                        if let Err(error) = send_frame(
                            connection.as_ref(),
                            &ServerFrame::Error(ProtocolErrorEnvelope {
                                request_id: None,
                                error: protocol_error,
                            }),
                            Some(negotiated),
                        )
                        .await
                        {
                            tracing::debug!(%client_id, %error, "gui decode error frame dropped");
                        }
                        break;
                    }
                };
                if let Err(error) = inner.connections.heartbeat(&client_id, now_timestamp()) {
                    tracing::debug!(%client_id, %error, "gui heartbeat update failed");
                }
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
    if let Err(error) = stop_tx.send(()) {
        tracing::debug!(%client_id, error = ?error, "gui forwarder stop signal dropped");
    }
    inner.connections.unregister(&client_id);
    if let Err(error) = connection.close().await {
        tracing::debug!(%client_id, %error, "gui connection close failed after session loop");
    }
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
            if let Err(error) = send_frame(
                connection,
                &ServerFrame::Error(ProtocolErrorEnvelope {
                    request_id: None,
                    error: protocol_error,
                }),
                None,
            )
            .await
            {
                tracing::debug!(%client_id, %error, "gui handshake decode error frame dropped");
            }
            if let Err(error) = connection.close().await {
                tracing::debug!(%client_id, %error, "gui connection close failed after handshake decode error");
            }
            return None;
        }
    };
    let ClientFrame::Handshake(request) = frame else {
        if let Err(error) = send_frame(
            connection,
            &ServerFrame::Error(ProtocolErrorEnvelope {
                request_id: None,
                error: ProtocolError::invalid_frame("first frame must be ClientFrame::Handshake"),
            }),
            None,
        )
        .await
        {
            tracing::debug!(%client_id, %error, "gui handshake-required error frame dropped");
        }
        if let Err(error) = connection.close().await {
            tracing::debug!(%client_id, %error, "gui connection close failed after non-handshake first frame");
        }
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
        if let Err(error) = connection.close().await {
            tracing::debug!(%client_id, %error, "gui connection close failed after handshake reject");
        }
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
            // SessionGet 的 timeline 分页由 host.query() 内部执行（S7 wave A
            // 曾在此预调用一次并丢弃结果，导致带分页参数的查询执行两遍）。
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
        ClientFrame::ArtifactRead(request) => FrameOutcome::Reply(vec![capability_error_frame(
            Some(request.request_id),
            GuiCapability::ArtifactStreaming,
            "artifact read",
        )]),
        ClientFrame::Heartbeat { nonce } => FrameOutcome::Reply(vec![ServerFrame::Pong { nonce }]),
        ClientFrame::Pong { .. } => FrameOutcome::None,
        ClientFrame::Subscribe(request) => {
            if !client_has_capability(inner, client_id, &GuiCapability::Events) {
                return FrameOutcome::Reply(vec![capability_error_frame(
                    Some(request.request_id),
                    GuiCapability::Events,
                    "event subscription",
                )]);
            }
            match inner
                .connections
                .subscribe(client_id, &request.subscription_id, request.streams)
            {
                Ok(()) => FrameOutcome::None,
                Err(error) => {
                    FrameOutcome::Reply(vec![manager_error_frame(Some(request.request_id), &error)])
                }
            }
        }
        ClientFrame::Unsubscribe {
            request_id,
            subscription_id,
        } => {
            if !client_has_capability(inner, client_id, &GuiCapability::Events) {
                return FrameOutcome::Reply(vec![capability_error_frame(
                    Some(request_id),
                    GuiCapability::Events,
                    "event unsubscription",
                )]);
            }
            match inner.connections.unsubscribe(client_id, &subscription_id) {
                Ok(()) => FrameOutcome::None,
                Err(error) => {
                    FrameOutcome::Reply(vec![manager_error_frame(Some(request_id), &error)])
                }
            }
        }
        ClientFrame::Resume(request) => handle_resume(inner, client_id, request).await,
        ClientFrame::SnapshotRequest { request_id } => {
            if !client_has_capability(inner, client_id, &GuiCapability::Snapshots) {
                return FrameOutcome::Reply(vec![capability_error_frame(
                    Some(request_id),
                    GuiCapability::Snapshots,
                    "snapshot request",
                )]);
            }
            match snapshot_for_client(inner, client_id).await {
                Ok(snapshot) => FrameOutcome::Reply(vec![ServerFrame::Snapshot(snapshot)]),
                Err(error) => {
                    FrameOutcome::Reply(vec![ServerFrame::Error(ProtocolErrorEnvelope {
                        request_id: Some(request_id),
                        error: host_error_to_protocol(&error),
                    })])
                }
            }
        }
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
    match &disposition {
        ResumeDisposition::Replay { .. }
            if !client_has_capability(inner, client_id, &GuiCapability::Events) =>
        {
            return FrameOutcome::Reply(vec![capability_error_frame(
                Some(request.request_id),
                GuiCapability::Events,
                "resume replay",
            )]);
        }
        ResumeDisposition::SnapshotRequired { .. }
            if !client_has_capability(inner, client_id, &GuiCapability::Snapshots) =>
        {
            return FrameOutcome::Reply(vec![capability_error_frame(
                Some(request.request_id),
                GuiCapability::Snapshots,
                "resume snapshot",
            )]);
        }
        _ => {}
    }
    let mut replies = vec![ServerFrame::Resume(ResumeResponse {
        request_id: request.request_id.clone(),
        disposition: disposition.clone(),
    })];
    match disposition {
        ResumeDisposition::Replay {
            from_sequence,
            through_sequence,
        } => match inner.host.replay(from_sequence, Some(through_sequence)) {
            Ok(events) => {
                let events: Vec<_> = events
                    .into_iter()
                    .filter(|event| event_is_granted(inner, client_id, event))
                    .collect();
                if let (Some(first), Some(last)) = (events.first(), events.last()) {
                    replies.clear();
                    replies.push(ServerFrame::Resume(ResumeResponse {
                        request_id: request.request_id.clone(),
                        disposition: ResumeDisposition::Replay {
                            from_sequence: first.global_sequence,
                            through_sequence: last.global_sequence,
                        },
                    }));
                    replies.extend(events.into_iter().map(ServerFrame::Event));
                } else {
                    // Replay 窗口里可能只有当前客户端未获授权的流（例如 terminal）。
                    // 以该窗口末端报告 UpToDate，避免客户端等待永远不会发送的事件。
                    replies.clear();
                    replies.push(ServerFrame::Resume(ResumeResponse {
                        request_id: request.request_id.clone(),
                        disposition: ResumeDisposition::UpToDate {
                            current_sequence: through_sequence,
                        },
                    }));
                }
            }
            Err(error) => {
                tracing::warn!(%error, "resume replay unavailable; falling back to snapshot");
                replies.clear();
                if !client_has_capability(inner, client_id, &GuiCapability::Snapshots) {
                    replies.push(capability_error_frame(
                        Some(request.request_id),
                        GuiCapability::Snapshots,
                        "resume fallback snapshot",
                    ));
                    return FrameOutcome::Reply(replies);
                }
                replies.push(ServerFrame::Resume(ResumeResponse {
                    request_id: request.request_id,
                    disposition: ResumeDisposition::SnapshotRequired {
                        earliest_available_sequence: inner
                            .host
                            .earliest_available()
                            .unwrap_or(GlobalSequence(0)),
                    },
                }));
                if let Ok(snapshot) = snapshot_for_client(inner, client_id).await {
                    replies.push(ServerFrame::Snapshot(snapshot));
                }
            }
        },
        ResumeDisposition::SnapshotRequired { .. } => {
            if let Ok(snapshot) = snapshot_for_client(inner, client_id).await {
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
                            if !event_is_granted(inner.as_ref(), &client_id, &event) {
                                continue;
                            }
                            if !inner.connections.should_forward(&client_id, &event.stream) {
                                continue;
                            }
                            if let Err(error) = inner.connections.enqueue(&client_id, event) {
                                match error {
                                    ManagerError::Lagged { .. } => {
                                        send_lagged_degrade(&inner, &client_id, None, &host_tx, &error);
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
                        Err(broadcast::error::RecvError::Lagged(missed)) => {
                            let error = ManagerError::Lagged {
                                client_id: client_id.clone(),
                            };
                            if let Err(error) = inner.connections.mark_lagged(&client_id) {
                                tracing::debug!(%client_id, %error, "gui mark_lagged failed");
                            }
                            send_lagged_degrade(&inner, &client_id, Some(missed), &host_tx, &error);
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
        if let Err(error) = host_tx.send(TransportFrame::new(bytes)) {
            tracing::debug!(error = ?error, "gui lagged error frame dropped");
        }
    }
}

fn send_lagged_degrade(
    inner: &Inner,
    client_id: &GuiClientId,
    missed: Option<u64>,
    host_tx: &mpsc::UnboundedSender<TransportFrame>,
    error: &ManagerError,
) {
    if let Some(envelope) = inner
        .host
        .publish_event_stream_lagged(missed, Some(client_id.as_str()))
    {
        if let Ok(bytes) = encode_server_frame(&ServerFrame::Event(envelope)) {
            if host_tx.send(TransportFrame::new(bytes)).is_err() {
                tracing::warn!(
                    code = "degrade.event_stream_lagged",
                    %client_id,
                    missed,
                    "event stream lagged frame send failed"
                );
            }
        }
    } else {
        tracing::warn!(
            code = "degrade.event_stream_lagged",
            %client_id,
            missed,
            "event stream lagged publish returned no envelope"
        );
    }
    send_lagged_error(host_tx, error);
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

fn client_has_capability(
    inner: &Inner,
    client_id: &GuiClientId,
    capability: &GuiCapability,
) -> bool {
    inner
        .connections
        .session(client_id)
        .is_some_and(|session| session.capabilities.contains(capability))
}

/// Snapshot 本身由 host 共源构造；连接层再按协商能力裁掉未授权 section。
/// TerminalSessions 即使不含输出正文，也会暴露 terminal id / owner / state，
/// 因而与 live terminal event 一样受 TerminalStreaming 保护。
async fn snapshot_for_client(
    inner: &Inner,
    client_id: &GuiClientId,
) -> Result<pawork_protocol::Snapshot, GuiHostError> {
    let mut snapshot = inner.host.snapshot().await?;
    if !client_has_capability(inner, client_id, &GuiCapability::TerminalStreaming) {
        snapshot
            .sections
            .retain(|section| section.kind != SnapshotSectionKind::TerminalSessions);
    }
    Ok(snapshot)
}

fn event_is_granted(
    inner: &Inner,
    client_id: &GuiClientId,
    event: &pawork_protocol::AppEventEnvelope,
) -> bool {
    if !client_has_capability(inner, client_id, &GuiCapability::Events) {
        return false;
    }
    !matches!(event.stream, EventStream::Terminal(_))
        || client_has_capability(inner, client_id, &GuiCapability::TerminalStreaming)
}

fn capability_error_frame(
    request_id: Option<String>,
    capability: GuiCapability,
    operation: &str,
) -> ServerFrame {
    ServerFrame::Error(ProtocolErrorEnvelope {
        request_id,
        error: ProtocolError {
            code: ProtocolErrorCode::PermissionDenied,
            message: format!(
                "capability {capability:?} is not granted to this client for {operation}"
            ),
            retryable: false,
        },
    })
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
