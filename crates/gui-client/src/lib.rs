//! Pawork GUI Connection Protocol typed 连接 SDK（P13-9）。
//!
//! [`GuiClient`] 包装 [`transport-api`] 的 [`GuiTransportClient`] /
//! [`GuiConnection`] 与 [`gui-protocol`] 的帧编解码，为 Desktop GUI 与协议
//! 测试客户端提供有类型的连接面：
//!
//! - `connect`：传输连接 + 握手（认证 + 版本协商）+ 消费首帧 Snapshot；
//! - Command / Query 请求-响应往返（按 request_id 关联；同 command_id 重放
//!   由服务端幂等存储返回首次响应）；
//! - Subscribe / Unsubscribe 与 `next_event` 事件读取；
//! - Snapshot 请求与 Resume（Replay 补发 / SnapshotRequired 降级重建）；
//! - ArtifactRead 分片读取与重组（循环接收 `ArtifactChunk` 直到 `eof`）；
//! - Ack / Heartbeat（自动回 Pong，心跳往返校验）；
//! - `close` 断开与 `connect_with_resume` 重连辅助（按 last_global_sequence
//!   重建缺失事件）。
//!
//! 错误一律是结构化 [`ClientError`]；SDK 不向调用方泄漏内部帧字节，意外的
//! 帧只以 [`ClientError::UnexpectedFrame`] 的类别标签呈现，不携带原始内容。
//!
//! 本 crate 不依赖任何 GUI 框架，也不链接 core-runtime / app-service（契约
//! 测试使用的服务端装配仅位于 dev-dependencies，见 workspace-layout）。

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use agent_domain::{ArtifactId, CommandId, ConnectionId, GuiClientId, QueryId, Timestamp};
use client_auth::{Token, TOKEN_SCHEME};
use core_api::{
    ActorIdentity, ApiHandle, ApiVersion, AppCommand, AppCommandEnvelope, AppEventEnvelope,
    AppQuery, AppQueryEnvelope, AppResponseEnvelope, CommandSource, EventStream, GlobalSequence,
    SUPPORTED_API_VERSIONS,
};
use gui_protocol::{
    decode_server_frame, encode_client_frame, ArtifactChunk, ArtifactReadRequest,
    ClientAuthentication, ClientFrame, GuiCapability, HandshakeRequest, HandshakeResponse,
    ProtocolCodecError, ProtocolError, ResumeRequest, ResumeResponse, ServerFrame, Snapshot,
    SubscribeRequest,
};
use thiserror::Error;
use tokio::sync::Mutex as AsyncMutex;
use transport_api::{
    ConnectOptions, ConnectionInfo, GuiConnection, GuiTransportClient, TransportEndpoint,
    TransportError, TransportFrame,
};

/// 重连 disposition（服务端判定结果，[`SessionInfo`] / [`ResumeOutcome`] 使用）。
pub use gui_protocol::ResumeDisposition;

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
                GuiCapability::ArtifactStreaming,
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
            | ClientError::UnexpectedFrame { .. }
            | ClientError::Internal(_) => false,
        }
    }

    pub fn is_auth_failure(&self) -> bool {
        matches!(
            self,
            ClientError::HandshakeRejected(error)
                if error.code == gui_protocol::ProtocolErrorCode::AuthenticationFailed
        )
    }

    pub fn is_incompatible_version(&self) -> bool {
        matches!(
            self,
            ClientError::HandshakeRejected(error)
                if error.code == gui_protocol::ProtocolErrorCode::IncompatibleVersion
        )
    }

    pub fn is_request_not_found(&self) -> bool {
        matches!(
            self,
            ClientError::Protocol(error)
                if error.code == gui_protocol::ProtocolErrorCode::RequestNotFound
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
        token: &Token,
    ) -> Result<Self, ClientError> {
        Self::connect_with_config(transport, endpoint, options, token, ClientConfig::default())
            .await
    }

    /// 连接 + 握手（自定义配置）。
    pub async fn connect_with_config(
        transport: Arc<dyn GuiTransportClient>,
        endpoint: TransportEndpoint,
        options: ConnectOptions,
        token: &Token,
        config: ClientConfig,
    ) -> Result<Self, ClientError> {
        let conn = transport
            .connect(endpoint, options)
            .await
            .map_err(ClientError::Transport)?;
        let conn: Arc<dyn GuiConnection> = Arc::from(conn);
        Self::handshake(conn, &config, token).await
    }

    /// 重连辅助：连接 + 握手后按 `last_global_sequence` Resume，返回新客户端
    /// 与补发结果。`None` 表示不执行 Resume（新客户端直接可用）。
    pub async fn connect_with_resume(
        transport: Arc<dyn GuiTransportClient>,
        endpoint: TransportEndpoint,
        options: ConnectOptions,
        token: &Token,
        last_global_sequence: Option<GlobalSequence>,
    ) -> Result<(Self, Option<ResumeOutcome>), ClientError> {
        Self::connect_with_resume_config(
            transport,
            endpoint,
            options,
            token,
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
        token: &Token,
        last_global_sequence: Option<GlobalSequence>,
        config: ClientConfig,
    ) -> Result<(Self, Option<ResumeOutcome>), ClientError> {
        let client =
            Self::connect_with_config(Arc::clone(&transport), endpoint, options, token, config)
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
        token: &Token,
    ) -> Result<Self, ClientError> {
        send_frame(
            conn.as_ref(),
            &ClientFrame::Handshake(HandshakeRequest {
                request_id: "handshake".into(),
                client_name: config.client_name.clone(),
                client_version: config.client_version.clone(),
                supported_api_versions: config.supported_api_versions.clone(),
                capabilities: config.capabilities.clone(),
                authentication: Some(ClientAuthentication {
                    scheme: TOKEN_SCHEME.into(),
                    proof: token.as_str().into(),
                }),
            }),
        )
        .await?;
        let response = match recv_frame(conn.as_ref(), config.timeout).await? {
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
        let snapshot = match recv_frame(conn.as_ref(), config.timeout).await? {
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
            command_id: CommandId::from(format!("gui-cmd-{id}")),
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
            request_id: QueryId::from(format!("gui-query-{id}")),
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
            match self.recv_frame(self.config.timeout).await? {
                ServerFrame::Response(envelope) if envelope.request_id.as_str() == request_id => {
                    return Ok(envelope);
                }
                ServerFrame::Error(envelope)
                    if envelope.request_id.as_deref() == Some(request_id) =>
                {
                    return Err(ClientError::Protocol(envelope.error));
                }
                other => self.stash(other),
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
            match self.recv_frame(remaining).await? {
                ServerFrame::Event(event) => return Ok(event),
                other => self.stash(other),
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
            match self.recv_frame(self.config.timeout).await? {
                ServerFrame::Snapshot(snapshot) => return Ok(snapshot),
                ServerFrame::Error(envelope) => return Err(ClientError::Protocol(envelope.error)),
                other => self.stash(other),
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
            match self.recv_frame(self.config.timeout).await? {
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
                        disposition = Some(ResumeDisposition::SnapshotRequired {
                            earliest_available_sequence,
                        });
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
                            self.stash(ServerFrame::Event(event));
                            break;
                        }
                    }
                    None => self.stash(ServerFrame::Event(event)),
                },
                ServerFrame::Snapshot(found) => {
                    snapshot = Some(found.clone());
                    if matches!(
                        disposition,
                        Some(ResumeDisposition::SnapshotRequired { .. })
                    ) {
                        break;
                    }
                    self.stash(ServerFrame::Snapshot(found));
                }
                ServerFrame::Error(envelope) => {
                    return Err(ClientError::Protocol(envelope.error));
                }
                other => self.stash(other),
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
    // Artifact 分片读取
    // -----------------------------------------------------------------------

    /// 分片读取 Artifact 并重组：循环接收 `ArtifactChunk`（offset 连续）直到
    /// `eof`。`limit == 0` 表示读到文件尾。
    pub async fn read_artifact(
        &self,
        artifact_id: &ArtifactId,
        offset: u64,
        limit: u64,
    ) -> Result<Vec<u8>, ClientError> {
        let id = self.next_request.fetch_add(1, Ordering::Relaxed);
        let request_id = format!("artifact-{id}");
        self.send_frame(&ClientFrame::ArtifactRead(ArtifactReadRequest {
            request_id: request_id.clone(),
            artifact_id: artifact_id.clone(),
            offset,
            limit,
        }))
        .await?;
        let mut assembled = Vec::new();
        let mut expected = offset;
        loop {
            match self.recv_frame(self.config.timeout).await? {
                ServerFrame::ArtifactChunk(chunk) if chunk.request_id == request_id => {
                    if chunk.offset != expected {
                        return Err(ClientError::UnexpectedFrame {
                            context: "artifact chunk",
                            found: "non-contiguous chunk offset",
                        });
                    }
                    assembled.extend_from_slice(&chunk.data);
                    expected = chunk.offset + chunk.data.len() as u64;
                    if chunk.eof {
                        return Ok(assembled);
                    }
                }
                ServerFrame::Error(envelope)
                    if envelope.request_id.as_deref() == Some(&request_id) =>
                {
                    return Err(ClientError::Protocol(envelope.error));
                }
                other => self.stash(other),
            }
        }
    }

    /// 单次分片读取（不重组），返回原始分片；`eof` 在末片为 true。
    pub async fn read_artifact_chunks(
        &self,
        artifact_id: &ArtifactId,
        offset: u64,
        limit: u64,
    ) -> Result<Vec<ArtifactChunk>, ClientError> {
        let id = self.next_request.fetch_add(1, Ordering::Relaxed);
        let request_id = format!("artifact-chunks-{id}");
        self.send_frame(&ClientFrame::ArtifactRead(ArtifactReadRequest {
            request_id: request_id.clone(),
            artifact_id: artifact_id.clone(),
            offset,
            limit,
        }))
        .await?;
        let mut chunks = Vec::new();
        loop {
            match self.recv_frame(self.config.timeout).await? {
                ServerFrame::ArtifactChunk(chunk) if chunk.request_id == request_id => {
                    let eof = chunk.eof;
                    chunks.push(chunk);
                    if eof {
                        return Ok(chunks);
                    }
                }
                ServerFrame::Error(envelope)
                    if envelope.request_id.as_deref() == Some(&request_id) =>
                {
                    return Err(ClientError::Protocol(envelope.error));
                }
                other => self.stash(other),
            }
        }
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
            match self.recv_frame(self.config.timeout).await? {
                ServerFrame::Pong { nonce: pong } if pong == nonce => return Ok(pong),
                other => self.stash(other),
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
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            if let Some(frame) = self.inbox.lock().await.pop_front() {
                return Ok(frame);
            }
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
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
            let frame = decode_server_frame(bytes.as_bytes()).map_err(ClientError::Codec)?;
            match frame {
                ServerFrame::Heartbeat { nonce } => {
                    send_frame(self.conn.as_ref(), &ClientFrame::Pong { nonce }).await?;
                }
                other => return Ok(other),
            }
        }
    }

    /// 把当前请求不匹配的帧放回 inbox 供后续操作消费。
    fn stash(&self, frame: ServerFrame) {
        let mut inbox = self
            .inbox
            .try_lock()
            .expect("inbox lock is never held across await points");
        inbox.push_back(frame);
    }
}

async fn send_frame(conn: &dyn GuiConnection, frame: &ClientFrame) -> Result<(), ClientError> {
    let bytes = encode_client_frame(frame).map_err(ClientError::Codec)?;
    conn.send(TransportFrame::new(bytes))
        .await
        .map_err(ClientError::Transport)
}

/// 读取并解码一帧（自动回 Pong 服务端 Heartbeat），等待受 `timeout` 约束。
async fn recv_frame(
    conn: &dyn GuiConnection,
    timeout: Duration,
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
        let frame = decode_server_frame(bytes.as_bytes()).map_err(ClientError::Codec)?;
        match frame {
            ServerFrame::Heartbeat { nonce } => {
                send_frame(conn, &ClientFrame::Pong { nonce }).await?;
            }
            other => return Ok(other),
        }
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
