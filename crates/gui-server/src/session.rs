//! 每连接的握手与帧循环（P13-4 / P13-5）。
//!
//! [`spawn`] 派发一个连接任务：先处理握手（版本协商 + 认证），成功后登记到
//! [`ConnectionManager`] 并发首帧 Snapshot，然后进入帧循环。P13-5 接线：
//!
//! - Subscribe / Unsubscribe 登记连接订阅；事件经每连接有界队列由帧循环
//!   （每连接 writer 任务）发送，慢客户端满队列即标记 Lagged，不阻塞他人；
//! - Resume 用 [`compute_resume_disposition`] 判定，Replay 走
//!   [`EventHub::replay`]，窗口不可用（ring 已淘汰）时降级 SnapshotRequired；
//! - SnapshotRequest 生成完整 Snapshot；Ack 记录 `last_ack`；Heartbeat /
//!   任意入站帧刷新活跃，心跳超时断线清理但绝不取消 Run（[ADR-026]）。
//!
//! 入站帧用 `gui-protocol` 解码并校验信封版本，出站帧编码前同样校验信封
//! 版本（[ADR-036]）；宿主侧通过 [`SessionHandle`] 推送帧或关闭连接。
//!
//! [ADR-026]: ../../docs/adr/ADR-026-gui-disconnect-safe.md

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use agent_domain::{ConnectionId, GuiClientId, Timestamp};
use app_service::AppServiceError;
use async_trait::async_trait;
use connection_manager::{ClientRegistration, ManagerError};
use core_api::{ApiVersion, GlobalSequence};
use gui_protocol::{
    codec::decode_client_frame, compute_resume_disposition, decode_client_frame_checked,
    encode_server_frame, validate_server_frame_api_version, ArtifactChunk, ArtifactReadRequest,
    ClientFrame, GuiCapability, HandshakeRequest, HandshakeResponse, HandshakeSession,
    ProtocolError, ProtocolErrorCode, ProtocolErrorEnvelope, ResumeContext, ResumeDisposition,
    ResumeRequest, ResumeResponse, ServerFrame, MAX_ARTIFACT_CHUNK_BYTES,
};
use subscription_hub::HubError;
use tokio::sync::{mpsc, oneshot};
use tokio::time::{interval, MissedTickBehavior};
use transport_api::{
    ConnectionInfo, GuiConnection, TransportError, TransportErrorKind, TransportFrame,
};

use crate::Inner;

/// 连接任务派发：返回宿主侧句柄与待 spawn 的任务。
pub(super) fn spawn(
    inner: Arc<Inner>,
    connection: Box<dyn GuiConnection>,
    client_id: GuiClientId,
    connection_id: ConnectionId,
) -> (SessionHandle, impl std::future::Future<Output = ()> + Send) {
    let (host_tx, host_rx) = mpsc::unbounded_channel::<TransportFrame>();
    let (close_tx, close_rx) = oneshot::channel::<()>();
    let info = connection.info();
    let handle = SessionHandle {
        host_tx,
        close_tx: StdMutex::new(Some(close_tx)),
        info,
        closed: AtomicBool::new(false),
    };
    let task = run(
        inner,
        connection,
        client_id,
        connection_id,
        host_rx,
        close_rx,
    );
    (handle, task)
}

/// 宿主侧连接句柄：`send` 经任务写入传输层；入站帧由任务消费，
/// `receive` 恒返回 ConnectionClosed（宿主不应接收）。
pub(super) struct SessionHandle {
    host_tx: mpsc::UnboundedSender<TransportFrame>,
    close_tx: StdMutex<Option<oneshot::Sender<()>>>,
    info: ConnectionInfo,
    closed: AtomicBool,
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
        Err(connection_closed(
            "inbound frames are consumed by the gui-server connection task",
        ))
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

/// 握手结果：接受的请求与响应（仅 Accepted 路径返回）。
struct HandshakeOutcome {
    request: HandshakeRequest,
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

    // 登记连接（有界事件队列由管理器持有发送端，接收端归本任务）。
    let registration = ClientRegistration {
        client_id: client_id.clone(),
        connection_id: connection_id.clone(),
        name: outcome.request.client_name,
        version: outcome.request.client_version,
        locality: connection.info().locality,
        identity: None,
        capabilities: granted_capabilities(&outcome.response),
        connected_at: now_timestamp(),
    };
    let event_rx = match inner.connections.register(registration) {
        Ok(receiver) => receiver,
        Err(error) => {
            tracing::warn!(%client_id, %error, "gui client registration failed");
            let _ = connection.close().await;
            return;
        }
    };

    // 首连握手后发 Snapshot（snapshot_sequence = hub.current()）；
    // Handshake 响应已由 handshake_phase 发出。
    let mut initial = Vec::new();
    match inner.snapshots.build() {
        Ok(snapshot) => initial.push(ServerFrame::Snapshot(snapshot)),
        Err(error) => tracing::warn!(%client_id, %error, "initial snapshot build failed"),
    }
    for frame in initial {
        if send_frame(connection.as_ref(), &frame, Some(negotiated))
            .await
            .is_err()
        {
            inner.connections.unregister(&client_id);
            let _ = connection.close().await;
            return;
        }
    }

    // 事件转发任务：Hub → 本连接有界队列（未订阅不投递；满则标记 Lagged）。
    let (stop_tx, stop_rx) = oneshot::channel();
    let _forwarder = spawn_forwarder(Arc::clone(&inner), client_id.clone(), stop_rx);

    // 心跳看门狗：超时断线清理（绝不取消 Run）。
    let mut watchdog = interval(watchdog_interval(
        inner.connections.config().heartbeat_timeout,
    ));
    watchdog.set_missed_tick_behavior(MissedTickBehavior::Delay);
    watchdog.tick().await; // 跳过首个立即 tick

    let mut event_rx = event_rx;
    loop {
        tokio::select! {
            biased;
            _ = &mut close_rx => {
                break;
            }
            Some(frame) = host_rx.recv() => {
                if let Err(error) = connection.send(frame).await {
                    tracing::debug!(%client_id, "gui server host send failed: {error}");
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
                    None => break, // 已注销：管理器释放了队列发送端
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
                // 任意入站帧都是活跃证据。
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

/// 握手：首帧必须是 `Handshake`；Rejected 或编解码失败时发送响应后关闭。
/// 接受路径返回请求与响应（含按 Hub 历史计算的 resume disposition）。
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
    let current = inner.hub.current();
    let earliest_available = inner.hub.earliest_available().unwrap_or(current);
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

async fn handle_frame(inner: &Inner, frame: ClientFrame, client_id: &GuiClientId) -> FrameOutcome {
    match frame {
        ClientFrame::Command(envelope) => {
            // 同步进程内派发；P13-5 异步接受时再引入 CommandAccepted。
            let response = inner.app_service.dispatch_envelope(envelope);
            FrameOutcome::Reply(vec![ServerFrame::Response(response)])
        }
        ClientFrame::Query(envelope) => {
            let response = inner.app_service.dispatch_query(envelope);
            FrameOutcome::Reply(vec![ServerFrame::Response(response)])
        }
        ClientFrame::ArtifactRead(request) => match artifact_chunks(inner, &request).await {
            Ok(chunks) => FrameOutcome::Reply(chunks),
            Err(error) => FrameOutcome::Reply(vec![ServerFrame::Error(ProtocolErrorEnvelope {
                request_id: Some(request.request_id),
                error,
            })]),
        },
        ClientFrame::Heartbeat { nonce } => FrameOutcome::Reply(vec![ServerFrame::Pong { nonce }]),
        ClientFrame::Pong { nonce } => {
            tracing::debug!(%client_id, nonce, "gui client pong");
            FrameOutcome::None
        }
        ClientFrame::Subscribe(request) => {
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
        } => match inner.connections.unsubscribe(client_id, &subscription_id) {
            Ok(()) => FrameOutcome::None,
            Err(error) => FrameOutcome::Reply(vec![manager_error_frame(Some(request_id), &error)]),
        },
        ClientFrame::Resume(request) => handle_resume(inner, request),
        ClientFrame::SnapshotRequest { request_id } => match inner.snapshots.build() {
            Ok(snapshot) => FrameOutcome::Reply(vec![ServerFrame::Snapshot(snapshot)]),
            Err(error) => FrameOutcome::Reply(vec![ServerFrame::Error(ProtocolErrorEnvelope {
                request_id: Some(request_id),
                error: internal_error(error.to_string()),
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

/// Resume：按 [`compute_resume_disposition`] 判定后补发 Replay 事件或降级 Snapshot。
fn handle_resume(inner: &Inner, request: ResumeRequest) -> FrameOutcome {
    let current = inner.hub.current();
    let earliest = inner.hub.earliest_available().unwrap_or(current);
    let disposition = compute_resume_disposition(earliest, current, request.last_global_sequence);
    let mut replies = vec![ServerFrame::Resume(ResumeResponse {
        request_id: request.request_id.clone(),
        disposition: disposition.clone(),
    })];
    match disposition {
        ResumeDisposition::Replay {
            from_sequence,
            through_sequence,
        } => match inner.hub.replay(from_sequence, Some(through_sequence)) {
            Ok(events) => replies.extend(events.into_iter().map(ServerFrame::Event)),
            Err(error) => {
                // 理论不可达（disposition 已保证窗口在 ring 内），防御性降级。
                tracing::warn!(%error, "resume replay unavailable; falling back to snapshot");
                replies.clear();
                replies.push(ServerFrame::Resume(ResumeResponse {
                    request_id: request.request_id,
                    disposition: ResumeDisposition::SnapshotRequired {
                        earliest_available_sequence: inner
                            .hub
                            .earliest_available()
                            .unwrap_or(current),
                    },
                }));
                if let Ok(snapshot) = inner.snapshots.build() {
                    replies.push(ServerFrame::Snapshot(snapshot));
                }
            }
        },
        ResumeDisposition::SnapshotRequired { .. } => {
            if let Ok(snapshot) = inner.snapshots.build() {
                replies.push(ServerFrame::Snapshot(snapshot));
            }
        }
        ResumeDisposition::UpToDate { .. } => {}
    }
    FrameOutcome::Reply(replies)
}

/// 事件转发任务：Hub 广播 → 本连接的有界队列（`enqueue` 非阻塞）。
///
/// 未订阅不投递；队列满时管理器标记 `Lagged` 并丢弃事件，不阻塞 Hub
/// 发布者与其他连接。连接注销（发送端释放）后本任务退出。
fn spawn_forwarder(
    inner: Arc<Inner>,
    client_id: GuiClientId,
    mut stop: oneshot::Receiver<()>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut subscription = inner.hub.subscribe();
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
                                    ManagerError::Lagged { .. } => continue,
                                    ManagerError::UnknownClient(_) | ManagerError::ChannelClosed(_) => return,
                                    ManagerError::AlreadyRegistered(_) => {
                                        unreachable!("registration happens once at session start")
                                    }
                                }
                            }
                        }
                        Err(HubError::Closed) => return,
                        Err(_) => continue,
                    }
                }
            }
        }
    })
}

/// 心跳看门狗间隔：超时的一半（至少 1ms）。
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

fn internal_error(message: String) -> ProtocolError {
    ProtocolError {
        code: ProtocolErrorCode::Internal,
        message,
        retryable: false,
    }
}

/// ArtifactRead → 经 app-service 读取真实 payload（P13-8），按 ≤64 KiB
/// 分片回 `ArtifactChunk`：offset 连续，末片 `eof = true`。
///
/// `limit == 0` 表示读到文件尾；`offset` 超尾时 app-service 返回空 data +
/// `eof = true`，本层因此只回单个空片。错误映射：NotFound → `RequestNotFound`，
/// Unavailable 及其余 → `Internal`。
async fn artifact_chunks(
    inner: &Inner,
    request: &ArtifactReadRequest,
) -> Result<Vec<ServerFrame>, ProtocolError> {
    let mut frames = Vec::new();
    let mut offset = request.offset;
    loop {
        let limit = if request.limit == 0 {
            MAX_ARTIFACT_CHUNK_BYTES as u64
        } else {
            request
                .limit
                .saturating_sub(offset.saturating_sub(request.offset))
                .min(MAX_ARTIFACT_CHUNK_BYTES as u64)
        };
        let result = inner
            .app_service
            .artifact_read(&request.artifact_id, offset, limit)
            .await
            .map_err(artifact_error_to_protocol)?;
        let chunk_len = result.data.len() as u64;
        let consumed = offset.saturating_sub(request.offset) + chunk_len;
        // app-service 的 eof 表示已到文件尾；客户端显式 limit 耗尽同样视为末片。
        let eof = result.eof || (request.limit != 0 && consumed >= request.limit);
        frames.push(ServerFrame::ArtifactChunk(ArtifactChunk {
            request_id: request.request_id.clone(),
            artifact_id: request.artifact_id.clone(),
            offset,
            data: result.data,
            eof,
        }));
        offset += chunk_len;
        if eof {
            break;
        }
    }
    Ok(frames)
}

fn artifact_error_to_protocol(error: AppServiceError) -> ProtocolError {
    let code = match error {
        AppServiceError::NotFound(_) => ProtocolErrorCode::RequestNotFound,
        // 无 store / 未就绪（Unavailable）与其余内部错误统一为 Internal。
        _ => ProtocolErrorCode::Internal,
    };
    ProtocolError {
        code,
        message: error.to_string(),
        retryable: false,
    }
}

/// 编码并发送一帧；`negotiated` 为握手协商版本（握手完成前为 `None`，此时
/// 仅编码不校验，非 Response/Event 信封帧本身也不参与版本校验）。
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

fn now_timestamp() -> Timestamp {
    Timestamp::from_unix_millis(now_unix_millis())
}

fn now_unix_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_domain::{CommandId, QueryId};
    use core_api::{AppResponse, AppResponseEnvelope, API_VERSION};
    use std::sync::Mutex;

    /// 只捕获出站字节的测试连接，receive 恒返回 ConnectionClosed（本组测试
    /// 只覆盖 send_frame 的出站校验路径）。
    struct CapturingConnection {
        sent: Mutex<Vec<TransportFrame>>,
    }

    #[async_trait]
    impl GuiConnection for CapturingConnection {
        async fn send(&self, frame: TransportFrame) -> Result<(), TransportError> {
            self.sent.lock().unwrap().push(frame);
            Ok(())
        }

        async fn receive(&self) -> Result<TransportFrame, TransportError> {
            Err(connection_closed("receive is not used in this test"))
        }

        async fn close(&self) -> Result<(), TransportError> {
            Ok(())
        }

        fn info(&self) -> ConnectionInfo {
            ConnectionInfo {
                connection_id: "test".into(),
                locality: transport_api::ConnectionLocality::InProcess,
                peer_label: None,
                encrypted: false,
                max_frame_bytes: 1024 * 1024,
            }
        }
    }

    fn response_frame(api_version: ApiVersion) -> ServerFrame {
        ServerFrame::Response(AppResponseEnvelope {
            api_version,
            request_id: QueryId::from("q-1"),
            responded_at: Timestamp::from_unix_millis(1),
            response: AppResponse::Accepted {
                command_id: CommandId::from("cmd-1"),
            },
        })
    }

    #[tokio::test]
    async fn send_frame_validates_api_version() {
        let connection = CapturingConnection {
            sent: Mutex::new(Vec::new()),
        };

        // 信封版本 == 协商版本：正常发送。
        send_frame(&connection, &response_frame(API_VERSION), Some(API_VERSION))
            .await
            .expect("matching version is sent");
        assert_eq!(connection.sent.lock().unwrap().len(), 1);

        // minor 过高（1.1 > 协商的 1.0）：拒绝发送，映射为 Internal。
        let error = send_frame(
            &connection,
            &response_frame(ApiVersion::new(1, 1)),
            Some(API_VERSION),
        )
        .await
        .expect_err("too-high minor must be rejected");
        assert_eq!(error.kind, TransportErrorKind::Internal);
        assert_eq!(connection.sent.lock().unwrap().len(), 1, "不发送违规帧");
    }

    #[tokio::test]
    async fn send_frame_skips_validation_before_negotiation() {
        // 握手完成前 negotiated 未知（None）：即使信封版本不匹配也仅编码发送。
        let connection = CapturingConnection {
            sent: Mutex::new(Vec::new()),
        };
        send_frame(&connection, &response_frame(ApiVersion::new(1, 1)), None)
            .await
            .expect("pre-negotiation sends are not version-checked");
        assert_eq!(connection.sent.lock().unwrap().len(), 1);
    }
}
