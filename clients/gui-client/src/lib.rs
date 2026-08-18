//! Pawork GUI Connection Protocol typed 连接 SDK。
//!
//! [`GuiClient`] 包装 [`pawork-transport`] 的 [`GuiTransportClient`] /
//! [`GuiConnection`] 与 [`pawork-protocol`] 的帧编解码，为 Desktop GUI 与协议
//! 测试客户端提供有类型的连接面：
//!
//! - `connect`：传输连接 + 握手（认证 + 版本协商）+ 消费首帧 Snapshot；
//! - Command / Query 请求-响应往返（按 request_id 关联；同 command_id 重放
//!   由服务端幂等存储返回首次响应）；
//! - Subscribe / Unsubscribe 与 `next_event` 事件读取；
//! - Snapshot 请求与 Resume（Replay 补发 / SnapshotRequired 降级重建）；
//! - Ack / Heartbeat（自动回 Pong，心跳往返校验）；
//! - `close` 断开与 `connect_with_resume` 重连辅助（按 last_global_sequence
//!   重建缺失事件）。
//!
//! 错误一律是结构化 [`ClientError`]；SDK 不向调用方泄漏内部帧字节，意外的
//! 帧只以 [`ClientError::UnexpectedFrame`] 的类别标签呈现，不携带原始内容。
//!
//! 本 crate 不依赖任何 GUI 框架，也不链接 pawork-app / pawork-gui-server（契约
//! 测试使用的服务端装配仅位于 dev-dependencies）。

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use pawork_domain::{CommandId, ConnectionId, GuiClientId, QueryId, Timestamp};
use pawork_protocol::{
    decode_server_frame, decode_server_frame_checked, encode_client_frame, ApiHandle,
    ClientFrame, HandshakeRequest, HandshakeResponse, ProtocolCodecError, ProtocolError,
    ProtocolErrorCode, ResumeRequest, ResumeResponse, ServerFrame, SubscribeRequest,
    SUPPORTED_API_VERSIONS,
};
use thiserror::Error;
use tokio::sync::Mutex as AsyncMutex;
use pawork_transport::{ConnectionInfo, GuiConnection, TransportError, TransportFrame};

/// 重连 disposition（服务端判定结果，[`SessionInfo`] / [`ResumeOutcome`] 使用）。
pub use pawork_protocol::ResumeDisposition;

/// Desktop 等上层 crate 的唯一业务依赖面：协议类型与本机传输。
pub use pawork_protocol::client_auth::TOKEN_SCHEME;
pub use pawork_protocol::{
    ActorIdentity, ApiVersion, AppCommand, AppCommandEnvelope, AppEvent, AppEventEnvelope,
    AppQuery, AppQueryEnvelope, AppResponse, AppResponseEnvelope, ClientAuthentication,
    CommandSource, EventStream, GlobalSequence, GuiCapability, Snapshot, TimelinePage,
};
pub use pawork_transport::{ConnectOptions, GuiTransportClient, LocalTransport, TransportEndpoint};

/// 客户端连接配置。
#[derive(Clone, Debug)]
pub struct ClientConfig {
    /// 单次协议操作的等待上限（握手、往返、事件读取等）。
    pub timeout: Duration,
    /// 握手时向服务端声明的客户端名称。
    pub client_name: String,
    /// 握手时向服务端声明的客户端版本。
    pub client_version: String,
    /// 请求的能力；服务端按自身能力筛选后授予（见 [`GuiClient::capabilities`]）。
    pub capabilities: Vec<GuiCapability>,
    /// 支持的 API 版本候选表；服务端取 major 相同的最高共同 minor。
    pub supported_api_versions: Vec<ApiVersion>,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(10),
            client_name: "gui-client".into(),
            client_version: env!("CARGO_PKG_VERSION").into(),
            capabilities: vec![
                GuiCapability::Events,
                GuiCapability::Snapshots,
                GuiCapability::Approvals,
            ],
            supported_api_versions: SUPPORTED_API_VERSIONS.to_vec(),
        }
    }
}

/// 握手成功后固定的会话信息。
#[derive(Clone, Debug)]
pub struct SessionInfo {
    pub handle: ApiHandle,
    pub client_id: GuiClientId,
    pub connection_id: ConnectionId,
    pub capabilities: Vec<GuiCapability>,
    /// 服务端按重连历史计算的初始 resume disposition（首连通常为
    /// `SnapshotRequired`）。
    pub resume: ResumeDisposition,
}

/// Resume 结果：disposition 与随 disposition 补发的内容。
#[derive(Clone, Debug)]
pub struct ResumeOutcome {
    pub disposition: ResumeDisposition,
    /// `Replay` 时补发的缺失事件（global_sequence 严格递增）。
    pub replayed: Vec<AppEventEnvelope>,
    /// `SnapshotRequired` 时随后的重建快照。
    pub snapshot: Option<Snapshot>,
}

/// 结构化客户端错误类别（供匹配，不携带内部帧）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClientErrorKind {
    Transport,
    Codec,
    HandshakeRejected,
    Protocol,
    Version,
    Timeout,
    Disconnected,
    ProtocolViolation,
    Internal,
}

/// gui-client 的结构化错误。
#[derive(Debug, Error)]
pub enum ClientError {
    #[error("transport error: {0}")]
    Transport(#[from] TransportError),
    #[error("protocol codec error: {0}")]
    Codec(#[from] ProtocolCodecError),
    #[error("handshake was rejected: {0:?}")]
    HandshakeRejected(ProtocolError),
    #[error("server reported a protocol error: {0:?}")]
    Protocol(ProtocolError),
    #[error("server frame api_version is incompatible with the negotiated version: {0:?}")]
    Version(ProtocolError),
    #[error("operation {operation} timed out after {timeout:?}")]
    Timeout {
        operation: &'static str,
        timeout: Duration,
    },
    #[error("connection is closed")]
    Disconnected,
    #[error("unexpected protocol frame while waiting for {context}: {found}")]
    UnexpectedFrame {
        context: &'static str,
        found: &'static str,
    },
    #[error("client internal error: {0}")]
    Internal(String),
}

impl ClientError {
    pub fn kind(&self) -> ClientErrorKind {
        match self {
            ClientError::Transport(_) => ClientErrorKind::Transport,
            ClientError::Codec(_) => ClientErrorKind::Codec,
            ClientError::HandshakeRejected(_) => ClientErrorKind::HandshakeRejected,
            ClientError::Protocol(_) => ClientErrorKind::Protocol,
            ClientError::Version(_) => ClientErrorKind::Version,
            ClientError::Timeout { .. } => ClientErrorKind::Timeout,
            ClientError::Disconnected => ClientErrorKind::Disconnected,
            ClientError::UnexpectedFrame { .. } => ClientErrorKind::ProtocolViolation,
            ClientError::Internal(_) => ClientErrorKind::Internal,
        }
    }

    pub fn is_retryable(&self) -> bool {
        match self {
            ClientError::Transport(error) => error.retryable,
            ClientError::Protocol(error) => error.retryable,
            ClientError::Timeout { .. } | ClientError::Disconnected => true,
            ClientError::Codec(_)
            | ClientError::HandshakeRejected(_)
            | ClientError::Version(_)
            | ClientError::UnexpectedFrame { .. }
            | ClientError::Internal(_) => false,
        }
    }

    pub fn is_auth_failure(&self) -> bool {
        matches!(
            self,
            ClientError::HandshakeRejected(error)
                if error.code == ProtocolErrorCode::AuthenticationFailed
        )
    }

    /// 错误是否表示版本不兼容：握手被拒，或后续 ServerFrame 信封版本与协商
    /// 版本不匹配（[ADR-036]）。
    pub fn is_incompatible_version(&self) -> bool {
        match self {
            ClientError::HandshakeRejected(error) => {
                error.code == ProtocolErrorCode::IncompatibleVersion
            }
            ClientError::Version(_) => true,
            _ => false,
        }
    }

    pub fn is_request_not_found(&self) -> bool {
        matches!(
            self,
            ClientError::Protocol(error)
                if error.code == ProtocolErrorCode::RequestNotFound
        )
    }

    pub fn is_replay_unavailable(&self) -> bool {
        matches!(
            self,
            ClientError::Protocol(error)
                if error.code == ProtocolErrorCode::ReplayUnavailable
        )
    }
}

/// GUI Connection Protocol typed 客户端。
///
/// 连接建立后即可往返 Command / Query、订阅事件、请求 Snapshot / Resume、
/// 分片读取 Artifact、发送 Ack / Heartbeat。所有等待操作受
/// [`ClientConfig::timeout`] 约束。
#[derive(Clone)]
pub struct GuiClient {
    conn: Arc<dyn GuiConnection>,
    config: ClientConfig,
    info: Arc<SessionInfo>,
    initial_snapshot: Arc<Mutex<Option<Snapshot>>>,
    /// 等待后续操作消费的帧（当前请求不匹配的帧先缓存，事件帧由此投递）。
    inbox: Arc<AsyncMutex<VecDeque<ServerFrame>>>,
    /// 单连接只允许一个任务读传输层。事件泵与 command/snapshot 并发
    /// `receive` 会把对端响应拆丢；锁内先按调用方类型查 inbox。
    io: Arc<AsyncMutex<()>>,
    next_request: Arc<AtomicU64>,
    next_nonce: Arc<AtomicU64>,
    last_acked: Arc<AtomicU64>,
    closed: Arc<AtomicBool>,
}

impl GuiClient {
    /// 连接 + 握手（认证与版本协商）+ 消费首帧 Snapshot。
    pub async fn connect(
        transport: Arc<dyn GuiTransportClient>,
        endpoint: TransportEndpoint,
        options: ConnectOptions,
        authentication: Option<ClientAuthentication>,
    ) -> Result<Self, ClientError> {
        Self::connect_with_config(transport, endpoint, options, authentication, ClientConfig::default())
            .await
    }

    /// 连接 + 握手（自定义配置）。
    pub async fn connect_with_config(
        transport: Arc<dyn GuiTransportClient>,
        endpoint: TransportEndpoint,
        options: ConnectOptions,
        authentication: Option<ClientAuthentication>,
        config: ClientConfig,
    ) -> Result<Self, ClientError> {
        let conn = transport
            .connect(endpoint, options)
            .await
            .map_err(ClientError::Transport)?;
        let conn: Arc<dyn GuiConnection> = Arc::from(conn);
        Self::handshake(conn, &config, authentication).await
    }

    /// 重连辅助：连接 + 握手后按 `last_global_sequence` Resume，返回新客户端
    /// 与补发结果。`None` 表示不执行 Resume（新客户端直接可用）。
    pub async fn connect_with_resume(
        transport: Arc<dyn GuiTransportClient>,
        endpoint: TransportEndpoint,
        options: ConnectOptions,
        authentication: Option<ClientAuthentication>,
        last_global_sequence: Option<GlobalSequence>,
    ) -> Result<(Self, Option<ResumeOutcome>), ClientError> {
        Self::connect_with_resume_config(
            transport,
            endpoint,
            options,
            authentication,
            last_global_sequence,
            ClientConfig::default(),
        )
        .await
    }

    /// 重连辅助（自定义配置版本）。
    pub async fn connect_with_resume_config(
        transport: Arc<dyn GuiTransportClient>,
        endpoint: TransportEndpoint,
        options: ConnectOptions,
        authentication: Option<ClientAuthentication>,
        last_global_sequence: Option<GlobalSequence>,
        config: ClientConfig,
    ) -> Result<(Self, Option<ResumeOutcome>), ClientError> {
        let client =
            Self::connect_with_config(Arc::clone(&transport), endpoint, options, authentication, config)
                .await?;
        let outcome = match last_global_sequence {
            Some(last) => Some(client.resume(last).await?),
            None => None,
        };
        Ok((client, outcome))
    }

    async fn handshake(
        conn: Arc<dyn GuiConnection>,
        config: &ClientConfig,
        authentication: Option<ClientAuthentication>,
    ) -> Result<Self, ClientError> {
        send_frame(
            conn.as_ref(),
            &ClientFrame::Handshake(HandshakeRequest {
                request_id: "handshake".into(),
                client_name: config.client_name.clone(),
                client_version: config.client_version.clone(),
                supported_api_versions: config.supported_api_versions.clone(),
                capabilities: config.capabilities.clone(),
                authentication,
            }),
        )
        .await?;
        let response = match recv_frame(conn.as_ref(), config.timeout, None).await? {
            ServerFrame::Handshake(response) => response,
            other => {
                return Err(unexpected_frame("handshake response", &other));
            }
        };
        let (handle, client_id, connection_id, resume, capabilities) = match response {
            HandshakeResponse::Accepted {
                handle,
                client_id,
                connection_id,
                resume,
                capabilities,
                ..
            } => (handle, client_id, connection_id, resume, capabilities),
            HandshakeResponse::Rejected { error, .. } => {
                return Err(ClientError::HandshakeRejected(error));
            }
        };
        // P13-5：Accepted 后服务端先发首帧 Snapshot。
        let snapshot =
            match recv_frame(conn.as_ref(), config.timeout, Some(handle.api_version)).await? {
                ServerFrame::Snapshot(snapshot) => snapshot,
                other => return Err(unexpected_frame("initial snapshot", &other)),
            };
        let info = Arc::new(SessionInfo {
            handle: handle.clone(),
            client_id,
            connection_id,
            capabilities,
            resume,
        });
        Ok(Self {
            conn,
            config: config.clone(),
            info,
            initial_snapshot: Arc::new(Mutex::new(Some(snapshot))),
            inbox: Arc::new(AsyncMutex::new(VecDeque::new())),
            io: Arc::new(AsyncMutex::new(())),
            next_request: Arc::new(AtomicU64::new(0)),
            next_nonce: Arc::new(AtomicU64::new(0)),
            last_acked: Arc::new(AtomicU64::new(0)),
            closed: Arc::new(AtomicBool::new(false)),
        })
    }

    // -----------------------------------------------------------------------
    // 会话信息访问
    // -----------------------------------------------------------------------

    pub fn info(&self) -> &SessionInfo {
        &self.info
    }

    pub fn handle(&self) -> &ApiHandle {
        &self.info.handle
    }

    pub fn client_id(&self) -> &GuiClientId {
        &self.info.client_id
    }

    pub fn connection_id(&self) -> &ConnectionId {
        &self.info.connection_id
    }

    pub fn api_version(&self) -> ApiVersion {
        self.info.handle.api_version
    }

    pub fn capabilities(&self) -> &[GuiCapability] {
        &self.info.capabilities
    }

    pub fn initial_snapshot(&self) -> Option<Snapshot> {
        self.initial_snapshot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    pub fn connection_info(&self) -> ConnectionInfo {
        self.conn.info()
    }

    pub fn is_connected(&self) -> bool {
        !self.closed.load(Ordering::Acquire)
    }

    pub fn last_acked_sequence(&self) -> GlobalSequence {
        GlobalSequence(self.last_acked.load(Ordering::Acquire))
    }

    // -----------------------------------------------------------------------
    // Command / Query
    // -----------------------------------------------------------------------

    /// 便捷 Command：SDK 生成 command_id 与时间戳后往返。
    pub async fn command(
        &self,
        command: AppCommand,
        source: CommandSource,
        identity: ActorIdentity,
    ) -> Result<AppResponseEnvelope, ClientError> {
        let id = self.next_request.fetch_add(1, Ordering::Relaxed);
        self.command_envelope(AppCommandEnvelope {
            api_version: self.api_version(),
            command_id: CommandId::from(format!(
                "gui-cmd-{}-{id}",
                self.info.client_id.as_str()
            )),
            source,
            identity,
            expected_revision: None,
            idempotency_key: None,
            issued_at: now_timestamp(),
            command,
        })
        .await
    }

    /// 直接往返一个完整信封（同 command_id 重放由服务端幂等存储返回首次响应）。
    pub async fn command_envelope(
        &self,
        envelope: AppCommandEnvelope,
    ) -> Result<AppResponseEnvelope, ClientError> {
        let command_id = envelope.command_id.as_str().to_string();
        self.send_frame(&ClientFrame::Command(envelope)).await?;
        self.await_response(&command_id).await
    }

    /// 便捷 Query：SDK 生成 request_id 与时间戳后往返。
    pub async fn query(
        &self,
        query: AppQuery,
        source: CommandSource,
        identity: ActorIdentity,
    ) -> Result<AppResponseEnvelope, ClientError> {
        let id = self.next_request.fetch_add(1, Ordering::Relaxed);
        self.query_envelope(AppQueryEnvelope {
            api_version: self.api_version(),
            request_id: QueryId::from(format!(
                "gui-query-{}-{id}",
                self.info.client_id.as_str()
            )),
            source,
            identity,
            issued_at: now_timestamp(),
            query,
        })
        .await
    }

    /// 直接往返一个完整查询信封。
    pub async fn query_envelope(
        &self,
        envelope: AppQueryEnvelope,
    ) -> Result<AppResponseEnvelope, ClientError> {
        let request_id = envelope.request_id.as_str().to_string();
        self.send_frame(&ClientFrame::Query(envelope)).await?;
        self.await_response(&request_id).await
    }

    async fn await_response(&self, request_id: &str) -> Result<AppResponseEnvelope, ClientError> {
        loop {
            match self.recv_matching(self.config.timeout, FrameWant::Response).await? {
                ServerFrame::Response(envelope) if envelope.request_id.as_str() == request_id => {
                    return Ok(envelope);
                }
                ServerFrame::Error(envelope)
                    if envelope.request_id.as_deref() == Some(request_id) =>
                {
                    return Err(ClientError::Protocol(envelope.error));
                }
                other => self.stash(other).await,
            }
        }
    }

    // -----------------------------------------------------------------------
    // 订阅 / 事件
    // -----------------------------------------------------------------------

    /// 订阅事件流；`streams` 为空表示全量订阅。
    pub async fn subscribe(
        &self,
        subscription_id: impl Into<String>,
        streams: Vec<EventStream>,
    ) -> Result<(), ClientError> {
        let id = self.next_request.fetch_add(1, Ordering::Relaxed);
        self.send_frame(&ClientFrame::Subscribe(SubscribeRequest {
            request_id: format!("subscribe-{id}"),
            subscription_id: subscription_id.into(),
            streams,
        }))
        .await
    }

    /// 全量订阅（等价于空 streams 的 [`GuiClient::subscribe`]）。
    pub async fn subscribe_all(&self) -> Result<(), ClientError> {
        self.subscribe("all", Vec::new()).await
    }

    pub async fn unsubscribe(&self, subscription_id: &str) -> Result<(), ClientError> {
        let id = self.next_request.fetch_add(1, Ordering::Relaxed);
        self.send_frame(&ClientFrame::Unsubscribe {
            request_id: format!("unsubscribe-{id}"),
            subscription_id: subscription_id.into(),
        })
        .await
    }

    /// 读取下一条事件（受 [`ClientConfig::timeout`] 约束）。
    pub async fn next_event(&self) -> Result<AppEventEnvelope, ClientError> {
        self.next_event_timeout(self.config.timeout).await
    }

    /// 在指定时限内读取下一条事件。
    pub async fn next_event_timeout(
        &self,
        timeout: Duration,
    ) -> Result<AppEventEnvelope, ClientError> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            match self.recv_matching(remaining, FrameWant::Event).await? {
                ServerFrame::Event(event) => return Ok(event),
                ServerFrame::Error(envelope) => return Err(ClientError::Protocol(envelope.error)),
                other => self.stash(other).await,
            }
        }
    }

    // -----------------------------------------------------------------------
    // Snapshot / Resume
    // -----------------------------------------------------------------------

    /// 请求完整 Snapshot。
    pub async fn snapshot(&self) -> Result<Snapshot, ClientError> {
        let id = self.next_request.fetch_add(1, Ordering::Relaxed);
        self.send_frame(&ClientFrame::SnapshotRequest {
            request_id: format!("snapshot-{id}"),
        })
        .await?;
        loop {
            match self.recv_matching(self.config.timeout, FrameWant::Snapshot).await? {
                ServerFrame::Snapshot(snapshot) => return Ok(snapshot),
                ServerFrame::Error(envelope) => return Err(ClientError::Protocol(envelope.error)),
                other => self.stash(other).await,
            }
        }
    }

    /// Resume：按 `last_global_sequence` 请求补发；服务端返回 Replay（补发缺失
    /// 事件）或降级 SnapshotRequired（附带重建 Snapshot）。
    pub async fn resume(
        &self,
        last_global_sequence: GlobalSequence,
    ) -> Result<ResumeOutcome, ClientError> {
        let id = self.next_request.fetch_add(1, Ordering::Relaxed);
        let request_id = format!("resume-{id}");
        self.send_frame(&ClientFrame::Resume(ResumeRequest {
            request_id: request_id.clone(),
            last_global_sequence,
        }))
        .await?;

        let mut disposition = None;
        let mut through: Option<u64> = None;
        let mut replayed = Vec::new();
        let mut snapshot = None;
        loop {
            match self.recv_matching(self.config.timeout, FrameWant::Resume).await? {
                ServerFrame::Resume(ResumeResponse {
                    request_id: rid,
                    disposition: found,
                }) if rid == request_id => match found {
                    ResumeDisposition::Replay {
                        from_sequence,
                        through_sequence,
                    } => {
                        through = Some(through_sequence.0);
                        disposition = Some(ResumeDisposition::Replay {
                            from_sequence,
                            through_sequence,
                        });
                    }
                    ResumeDisposition::SnapshotRequired {
                        earliest_available_sequence,
                    } => {
                        // 服务端附带第二帧 Snapshot（gui-design §4.1）；收齐后再返回。
                        disposition = Some(ResumeDisposition::SnapshotRequired {
                            earliest_available_sequence,
                        });
                        if snapshot.is_some() {
                            break;
                        }
                    }
                    ResumeDisposition::UpToDate { current_sequence } => {
                        disposition = Some(ResumeDisposition::UpToDate { current_sequence });
                        break;
                    }
                },
                ServerFrame::Event(event) => match through {
                    Some(through_sequence) => {
                        if event.global_sequence.0 <= through_sequence {
                            let done = event.global_sequence.0 == through_sequence;
                            replayed.push(event);
                            if done {
                                break;
                            }
                        } else {
                            // 已进入实时事件流：留给 next_event。
                            self.stash(ServerFrame::Event(event)).await;
                            break;
                        }
                    }
                    None => self.stash(ServerFrame::Event(event)).await,
                },
                ServerFrame::Snapshot(found) => {
                    if matches!(
                        disposition,
                        Some(ResumeDisposition::SnapshotRequired { .. }) | None
                    ) {
                        snapshot = Some(found);
                        if matches!(
                            disposition,
                            Some(ResumeDisposition::SnapshotRequired { .. })
                        ) {
                            break;
                        }
                    } else {
                        self.stash(ServerFrame::Snapshot(found)).await;
                    }
                }
                ServerFrame::Error(envelope) => {
                    return Err(ClientError::Protocol(envelope.error));
                }
                other => self.stash(other).await,
            }
        }
        let disposition = disposition.ok_or_else(|| {
            ClientError::Internal("resume response was not received before stream ended".into())
        })?;
        Ok(ResumeOutcome {
            disposition,
            replayed,
            snapshot,
        })
    }

    // -----------------------------------------------------------------------
    // Ack / Heartbeat
    // -----------------------------------------------------------------------

    /// Ack：确认已消费到 `global_sequence`（服务端据此计算重连 Replay 范围）。
    pub async fn ack(&self, global_sequence: GlobalSequence) -> Result<(), ClientError> {
        self.send_frame(&ClientFrame::Ack { global_sequence })
            .await?;
        self.last_acked.store(global_sequence.0, Ordering::Release);
        Ok(())
    }

    /// Heartbeat 往返：发送随机 nonce，等待服务端 Pong 返回。
    pub async fn heartbeat(&self) -> Result<u64, ClientError> {
        let nonce = self.next_nonce.fetch_add(1, Ordering::Relaxed);
        self.heartbeat_with_nonce(nonce).await
    }

    /// 指定 nonce 的 Heartbeat 往返。
    pub async fn heartbeat_with_nonce(&self, nonce: u64) -> Result<u64, ClientError> {
        self.send_frame(&ClientFrame::Heartbeat { nonce }).await?;
        loop {
            match self.recv_matching(self.config.timeout, FrameWant::Any).await? {
                ServerFrame::Pong { nonce: pong } if pong == nonce => return Ok(pong),
                other => self.stash(other).await,
            }
        }
    }

    // -----------------------------------------------------------------------
    // 断开 / 重连
    // -----------------------------------------------------------------------

    /// 主动断开连接。断开后所有操作返回 [`ClientError::Disconnected`]；
    /// 服务端会话注销但不会取消任何 Run。
    pub async fn close(&self) -> Result<(), ClientError> {
        self.disconnect().await
    }

    /// [`GuiClient::close`] 的别名。
    pub async fn disconnect(&self) -> Result<(), ClientError> {
        if self.closed.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        self.conn.close().await.map_err(ClientError::Transport)
    }

    // -----------------------------------------------------------------------
    // 内部收发
    // -----------------------------------------------------------------------

    async fn send_frame(&self, frame: &ClientFrame) -> Result<(), ClientError> {
        if self.closed.load(Ordering::Acquire) {
            return Err(ClientError::Disconnected);
        }
        send_frame(self.conn.as_ref(), frame).await
    }

    /// 取下一帧：先查 inbox，再读传输层；服务端主动 Heartbeat 自动回 Pong。
    /// 单次等待不超过 `timeout`，超时返回 [`ClientError::Timeout`]。
    async fn recv_frame(&self, timeout: Duration) -> Result<ServerFrame, ClientError> {
        self.recv_matching(timeout, FrameWant::Any).await
    }

    async fn recv_matching(
        &self,
        timeout: Duration,
        want: FrameWant,
    ) -> Result<ServerFrame, ClientError> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            if let Some(frame) = self.pop_inbox(want).await {
                return Ok(frame);
            }
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return Err(ClientError::Timeout {
                    operation: "receive frame",
                    timeout,
                });
            }
            let frame = {
                let _guard = self.io.lock().await;
                if let Some(frame) = self.pop_inbox(want).await {
                    return Ok(frame);
                }
                let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
                if remaining.is_zero() {
                    return Err(ClientError::Timeout {
                        operation: "receive frame",
                        timeout,
                    });
                }
                let received = tokio::time::timeout(remaining, self.conn.receive()).await;
                let bytes = match received {
                    Ok(Ok(bytes)) => bytes,
                    Ok(Err(error)) => return Err(ClientError::Transport(error)),
                    Err(_) => {
                        return Err(ClientError::Timeout {
                            operation: "receive frame",
                            timeout,
                        });
                    }
                };
                decode_server_frame_checked(bytes.as_bytes(), self.api_version())
                    .map_err(decode_error)?
            };
            match frame {
                ServerFrame::Heartbeat { nonce } => {
                    send_frame(self.conn.as_ref(), &ClientFrame::Pong { nonce }).await?;
                }
                other if want.matches(&other) => return Ok(other),
                other => {
                    self.stash(other).await;
                    tokio::task::yield_now().await;
                }
            }
        }
    }

    /// 把当前请求不匹配的帧放回 inbox 供后续操作消费。
    async fn stash(&self, frame: ServerFrame) {
        self.inbox.lock().await.push_back(frame);
    }

    async fn pop_inbox(&self, want: FrameWant) -> Option<ServerFrame> {
        let mut inbox = self.inbox.lock().await;
        if let Some(index) = inbox.iter().position(|frame| want.matches(frame)) {
            return Some(inbox.remove(index).expect("inbox index exists"));
        }
        None
    }
}

#[derive(Clone, Copy)]
enum FrameWant {
    Any,
    Event,
    Response,
    Snapshot,
    Resume,
}

impl FrameWant {
    fn matches(self, frame: &ServerFrame) -> bool {
        match self {
            Self::Any => true,
            Self::Event => matches!(frame, ServerFrame::Event(_) | ServerFrame::Error(_)),
            Self::Response => matches!(frame, ServerFrame::Response(_) | ServerFrame::Error(_)),
            Self::Snapshot => matches!(frame, ServerFrame::Snapshot(_) | ServerFrame::Error(_)),
            Self::Resume => matches!(
                frame,
                ServerFrame::Resume(_)
                    | ServerFrame::Snapshot(_)
                    | ServerFrame::Event(_)
                    | ServerFrame::Error(_)
            ),
        }
    }
}

async fn send_frame(conn: &dyn GuiConnection, frame: &ClientFrame) -> Result<(), ClientError> {
    let bytes = encode_client_frame(frame).map_err(ClientError::Codec)?;
    conn.send(TransportFrame::new(bytes))
        .await
        .map_err(ClientError::Transport)
}

/// 读取并解码一帧（自动回 Pong 服务端 Heartbeat），等待受 `timeout` 约束。
/// `negotiated` 为握手协商版本：`Some` 时校验信封 api_version（[ADR-036]），
/// 握手完成前传 `None` 仅解码。
async fn recv_frame(
    conn: &dyn GuiConnection,
    timeout: Duration,
    negotiated: Option<ApiVersion>,
) -> Result<ServerFrame, ClientError> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        let received = tokio::time::timeout(remaining, conn.receive()).await;
        let bytes = match received {
            Ok(Ok(bytes)) => bytes,
            Ok(Err(error)) => return Err(ClientError::Transport(error)),
            Err(_) => {
                return Err(ClientError::Timeout {
                    operation: "receive frame",
                    timeout,
                });
            }
        };
        let frame = match negotiated {
            Some(version) => decode_server_frame_checked(bytes.as_bytes(), version),
            None => decode_server_frame(bytes.as_bytes()).map_err(ProtocolError::from),
        }
        .map_err(decode_error)?;
        match frame {
            ServerFrame::Heartbeat { nonce } => {
                send_frame(conn, &ClientFrame::Pong { nonce }).await?;
            }
            other => return Ok(other),
        }
    }
}

/// 受检解码失败 → [`ClientError`]：版本不兼容单列 [`ClientError::Version`]，
/// 其余（编解码失败经 checked 路径折叠为线上错误）按协议错误呈现。
fn decode_error(error: ProtocolError) -> ClientError {
    if error.code == ProtocolErrorCode::IncompatibleVersion {
        ClientError::Version(error)
    } else {
        ClientError::Protocol(error)
    }
}

fn unexpected_frame(context: &'static str, frame: &ServerFrame) -> ClientError {
    ClientError::UnexpectedFrame {
        context,
        found: frame_label(frame),
    }
}

fn frame_label(frame: &ServerFrame) -> &'static str {
    match frame {
        ServerFrame::Handshake(_) => "handshake",
        ServerFrame::CommandAccepted { .. } => "command accepted",
        ServerFrame::Response(_) => "response",
        ServerFrame::Event(_) => "event",
        ServerFrame::Snapshot(_) => "snapshot",
        ServerFrame::Resume(_) => "resume",
        ServerFrame::ArtifactChunk(_) => "artifact chunk",
        ServerFrame::Error(_) => "error",
        ServerFrame::Heartbeat { .. } => "heartbeat",
        ServerFrame::Pong { .. } => "pong",
    }
}

fn now_timestamp() -> Timestamp {
    Timestamp::from_unix_millis(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_millis() as u64)
            .unwrap_or(0),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use pawork_domain::{CommandId, QueryId};
    use pawork_protocol::GuiCapability;
    use pawork_protocol::{encode_server_frame, AppResponse, AppResponseEnvelope, API_VERSION};
    use std::future::Future;
    use std::pin::Pin;
    use pawork_transport::{ConnectionLocality, TransportErrorKind};

    /// 返回固定帧字节队列的测试连接。gui-client 无 async-trait 依赖，按
    /// `#[async_trait]` 的脱糖签名手工实现 `GuiConnection`。
    struct MockConnection {
        frames: Mutex<VecDeque<TransportFrame>>,
    }

    impl GuiConnection for MockConnection {
        fn send<'life0, 'async_trait>(
            &'life0 self,
            _frame: TransportFrame,
        ) -> Pin<Box<dyn Future<Output = Result<(), TransportError>> + Send + 'async_trait>>
        where
            'life0: 'async_trait,
            Self: 'async_trait,
        {
            Box::pin(async { Ok(()) })
        }

        fn receive<'life0, 'async_trait>(
            &'life0 self,
        ) -> Pin<
            Box<dyn Future<Output = Result<TransportFrame, TransportError>> + Send + 'async_trait>,
        >
        where
            'life0: 'async_trait,
            Self: 'async_trait,
        {
            Box::pin(async move {
                self.frames
                    .lock()
                    .unwrap()
                    .pop_front()
                    .ok_or_else(|| TransportError {
                        kind: TransportErrorKind::ConnectionClosed,
                        message: "no more frames".into(),
                        retryable: false,
                    })
            })
        }

        fn close<'life0, 'async_trait>(
            &'life0 self,
        ) -> Pin<Box<dyn Future<Output = Result<(), TransportError>> + Send + 'async_trait>>
        where
            'life0: 'async_trait,
            Self: 'async_trait,
        {
            Box::pin(async { Ok(()) })
        }

        fn info(&self) -> ConnectionInfo {
            ConnectionInfo {
                connection_id: "mock".into(),
                locality: ConnectionLocality::InProcess,
                peer_label: None,
                encrypted: false,
                max_frame_bytes: 1024 * 1024,
            }
        }
    }

    fn response_bytes(api_version: ApiVersion) -> Vec<u8> {
        encode_server_frame(&ServerFrame::Response(AppResponseEnvelope {
            api_version,
            request_id: QueryId::from("q-1"),
            responded_at: Timestamp::from_unix_millis(1),
            response: AppResponse::Accepted {
                command_id: CommandId::from("cmd-1"),
                run_id: None,
            },
        }))
        .expect("encode response")
    }

    fn mock(frames: Vec<TransportFrame>) -> MockConnection {
        MockConnection {
            frames: Mutex::new(VecDeque::from(frames)),
        }
    }

    #[test]
    fn client_config_can_request_terminal_streaming() {
        // Desktop 握手自行声明 TerminalStreaming；默认配置不强制，避免影响既有契约装配。
        let mut config = ClientConfig::default();
        config.capabilities.push(GuiCapability::TerminalStreaming);
        assert!(config.capabilities.contains(&GuiCapability::TerminalStreaming));
    }

    #[tokio::test]
    async fn recv_frame_rejects_mismatched_version() {
        // 协商为 1.0，服务端帧信封带当前 API_VERSION（minor 过高）：拒绝并归类 Version。
        let conn = mock(vec![TransportFrame::new(response_bytes(API_VERSION))]);
        let error = recv_frame(&conn, Duration::from_millis(100), Some(ApiVersion::new(1, 0)))
            .await
            .expect_err("too-high minor must be rejected");
        assert!(matches!(error, ClientError::Version(_)));
        assert_eq!(error.kind(), ClientErrorKind::Version);
        assert!(error.is_incompatible_version());
    }

    #[tokio::test]
    async fn recv_frame_accepts_matching_version() {
        let conn = mock(vec![TransportFrame::new(response_bytes(API_VERSION))]);
        let frame = recv_frame(&conn, Duration::from_millis(100), Some(API_VERSION))
            .await
            .expect("matching version decodes");
        assert!(matches!(frame, ServerFrame::Response(_)));
    }

    #[tokio::test]
    async fn recv_frame_skips_validation_before_negotiation() {
        // 握手完成前 negotiated=None：只解码，不做版本校验。
        let conn = mock(vec![TransportFrame::new(response_bytes(ApiVersion::new(
            1, 1,
        )))]);
        let frame = recv_frame(&conn, Duration::from_millis(100), None)
            .await
            .expect("pre-negotiation recv decodes without version check");
        assert!(matches!(frame, ServerFrame::Response(_)));
    }

    #[tokio::test]
    async fn recv_matching_stashes_mismatched_frames_for_other_waiters() {
        use pawork_domain::{CoreInstanceId, EventId};
        use pawork_protocol::{EventSource, EventStream};

        let event = ServerFrame::Event(AppEventEnvelope {
            api_version: API_VERSION,
            instance_id: CoreInstanceId::from("instance-1"),
            event_id: EventId::from("event-1"),
            global_sequence: GlobalSequence(1),
            stream: EventStream::Global,
            stream_sequence: 1,
            timestamp: Timestamp::from_unix_millis(1),
            source: EventSource::Core,
            payload: AppEvent::RunChanged {
                run_id: pawork_domain::RunId::from("run-1"),
                state: pawork_protocol::RunState::StreamingResponse,
            },
        });
        let client = GuiClient {
            conn: Arc::new(mock(vec![
                TransportFrame::new(encode_server_frame(&event).expect("encode event")),
                TransportFrame::new(response_bytes(API_VERSION)),
            ])),
            config: ClientConfig::default(),
            info: Arc::new(SessionInfo {
                handle: ApiHandle {
                    instance_id: CoreInstanceId::from("instance-1"),
                    api_version: API_VERSION,
                },
                client_id: GuiClientId::from("client-1"),
                connection_id: ConnectionId::from("conn-1"),
                capabilities: Vec::new(),
                resume: ResumeDisposition::SnapshotRequired {
                    earliest_available_sequence: GlobalSequence(0),
                },
            }),
            initial_snapshot: Arc::new(Mutex::new(None)),
            inbox: Arc::new(AsyncMutex::new(VecDeque::new())),
            io: Arc::new(AsyncMutex::new(())),
            next_request: Arc::new(AtomicU64::new(0)),
            next_nonce: Arc::new(AtomicU64::new(0)),
            last_acked: Arc::new(AtomicU64::new(0)),
            closed: Arc::new(AtomicBool::new(false)),
        };

        let waiter = client.clone();
        let response = tokio::spawn(async move {
            waiter
                .recv_matching(Duration::from_secs(1), FrameWant::Response)
                .await
        });
        tokio::task::yield_now().await;
        let event_frame = client
            .recv_matching(Duration::from_secs(1), FrameWant::Event)
            .await
            .expect("event waiter receives stashed event");
        assert!(matches!(event_frame, ServerFrame::Event(_)));
        let response_frame = response
            .await
            .expect("response task")
            .expect("response waiter is not starved by the event pump");
        assert!(matches!(response_frame, ServerFrame::Response(_)));
    }

    #[tokio::test]
    async fn next_event_surfaces_replay_unavailable() {
        use pawork_protocol::{ProtocolError, ProtocolErrorEnvelope};

        let error = ServerFrame::Error(ProtocolErrorEnvelope {
            request_id: None,
            error: ProtocolError {
                code: ProtocolErrorCode::ReplayUnavailable,
                message: "lagged".into(),
                retryable: true,
            },
        });
        let client = GuiClient {
            conn: Arc::new(mock(vec![TransportFrame::new(
                encode_server_frame(&error).expect("encode error"),
            )])),
            config: ClientConfig::default(),
            info: Arc::new(SessionInfo {
                handle: ApiHandle {
                    instance_id: pawork_domain::CoreInstanceId::from("instance-1"),
                    api_version: API_VERSION,
                },
                client_id: GuiClientId::from("client-1"),
                connection_id: ConnectionId::from("conn-1"),
                capabilities: Vec::new(),
                resume: ResumeDisposition::SnapshotRequired {
                    earliest_available_sequence: GlobalSequence(0),
                },
            }),
            initial_snapshot: Arc::new(Mutex::new(None)),
            inbox: Arc::new(AsyncMutex::new(VecDeque::new())),
            io: Arc::new(AsyncMutex::new(())),
            next_request: Arc::new(AtomicU64::new(0)),
            next_nonce: Arc::new(AtomicU64::new(0)),
            last_acked: Arc::new(AtomicU64::new(0)),
            closed: Arc::new(AtomicBool::new(false)),
        };
        let error = client
            .next_event()
            .await
            .expect_err("ReplayUnavailable must be observable");
        assert!(error.is_replay_unavailable());
        assert_eq!(error.kind(), ClientErrorKind::Protocol);
    }
}
