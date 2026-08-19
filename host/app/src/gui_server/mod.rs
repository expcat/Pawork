//! GUI 协议服务器（S10 多客户端切片）。
//!
//! [`GuiServer`] 在 CLI 进程内接受 GUI 连接：`bind` 经由
//! [`pawork_transport::GuiTransportServer`] 绑定端点；每次 `accept` 派生一个
//! 连接任务，完成握手后登记到 [`ConnectionManager`]、先发 Snapshot，再进入
//! 帧循环。事件经每连接有界队列转发，满则丢**新**事件并标 lagged。Resume
//! 走 host 共源 `current` / `earliest` / `replay`。断线只清理连接，绝不取消 Run。

mod connection;
mod session;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use pawork_domain::{ConnectionId, GuiClientId};
use pawork_protocol::HandshakeService;
use pawork_transport::{GuiConnection, GuiListener, GuiTransportServer, TransportEndpoint, TransportError};

pub use connection::{
    ClientRegistration, ConnectionManager, ConnectionManagerConfig, GuiClientSession,
    GuiSubscription, ManagerError, DEFAULT_HEARTBEAT_TIMEOUT, DEFAULT_QUEUE_CAPACITY,
};

/// Host 端口错误（签名冻结，由 `pawork-app` 实现）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GuiHostError {
    pub code: String,
    pub message: String,
    pub retryable: bool,
}

impl std::fmt::Display for GuiHostError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for GuiHostError {}

/// Host 端口（既有方法签名保持；新增方法带 default，避免并行期间 pawork-app 编译失败）。
#[async_trait::async_trait]
pub trait GuiHost: Send + Sync {
    fn instance_id(&self) -> pawork_domain::CoreInstanceId;
    async fn snapshot(&self) -> Result<pawork_protocol::Snapshot, GuiHostError>;
    async fn timeline(
        &self,
        session_id: &pawork_domain::SessionId,
        after: Option<u64>,
        limit: Option<u32>,
    ) -> Result<pawork_protocol::TimelinePage, GuiHostError>;
    async fn query(
        &self,
        envelope: &pawork_protocol::AppQueryEnvelope,
    ) -> Result<pawork_protocol::AppResponse, GuiHostError>;
    async fn command(
        &self,
        envelope: &pawork_protocol::AppCommandEnvelope,
    ) -> Result<pawork_protocol::AppResponse, GuiHostError>;
    fn subscribe_events(&self) -> tokio::sync::broadcast::Receiver<pawork_protocol::AppEventEnvelope>;

    fn current_sequence(&self) -> pawork_protocol::GlobalSequence {
        pawork_protocol::GlobalSequence(0)
    }
    fn earliest_available(&self) -> Option<pawork_protocol::GlobalSequence> {
        None
    }
    fn replay(
        &self,
        from: pawork_protocol::GlobalSequence,
        through: Option<pawork_protocol::GlobalSequence>,
    ) -> Result<Vec<pawork_protocol::AppEventEnvelope>, GuiHostError> {
        let _ = (from, through);
        Ok(vec![])
    }
}

/// GUI 服务器的共享配置。
pub struct GuiServerConfig {
    pub host: std::sync::Arc<dyn GuiHost>,
    pub handshake: pawork_protocol::HandshakeService,
    pub transport: std::sync::Arc<dyn pawork_transport::GuiTransportServer>,
    /// 缺省为默认心跳/队列；测试可注入短超时或小队列。
    pub connections: Option<Arc<ConnectionManager>>,
}

pub(crate) struct Inner {
    pub host: Arc<dyn GuiHost>,
    pub handshake: HandshakeService,
    pub connections: Arc<ConnectionManager>,
}

/// CLI 进程内的 GUI 协议服务器。
#[derive(Clone)]
pub struct GuiServer {
    inner: Arc<Inner>,
    transport: Arc<dyn GuiTransportServer>,
}

impl GuiServer {
    pub fn new(config: GuiServerConfig) -> Self {
        let connections = config
            .connections
            .unwrap_or_else(|| Arc::new(ConnectionManager::default()));
        Self {
            inner: Arc::new(Inner {
                host: config.host,
                handshake: config.handshake,
                connections,
            }),
            transport: config.transport,
        }
    }

    pub fn host(&self) -> &Arc<dyn GuiHost> {
        &self.inner.host
    }

    pub fn handshake(&self) -> &HandshakeService {
        &self.inner.handshake
    }

    pub fn connections(&self) -> &Arc<ConnectionManager> {
        &self.inner.connections
    }

    /// 绑定端点并返回 GUI 监听器；每次 `accept` 启动一个连接任务。
    pub async fn bind(
        &self,
        endpoint: TransportEndpoint,
    ) -> Result<Box<dyn GuiListener>, TransportError> {
        let transport_listener = self.transport.bind(endpoint).await?;
        Ok(Box::new(GuiServerListener {
            inner: Arc::clone(&self.inner),
            transport_listener,
            next_connection: AtomicU64::new(0),
        }))
    }
}

struct GuiServerListener {
    inner: Arc<Inner>,
    transport_listener: Box<dyn GuiListener>,
    next_connection: AtomicU64,
}

#[async_trait]
impl GuiListener for GuiServerListener {
    async fn accept(&self) -> Result<Box<dyn GuiConnection>, TransportError> {
        let connection = self.transport_listener.accept().await?;
        let n = self.next_connection.fetch_add(1, Ordering::Relaxed);
        let client_id = GuiClientId::from(format!("client-{n}"));
        let connection_id = ConnectionId::from(format!("connection-{n}"));
        let (handle, task) = session::spawn(
            Arc::clone(&self.inner),
            connection,
            client_id,
            connection_id,
        );
        tokio::spawn(task);
        Ok(Box::new(handle))
    }

    async fn close(&self) -> Result<(), TransportError> {
        self.transport_listener.close().await
    }
}
