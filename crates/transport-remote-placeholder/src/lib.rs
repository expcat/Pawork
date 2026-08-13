//! 远程 GUI Transport 的可替换 Adapter 占位（P13-6）。
//!
//! 远程连接（内网穿透 / NAT / 中继 / 加密）由可替换 Adapter 提供：
//! - CLI 侧经 [`RemoteGuiTransportProvider`] 发布 / 撤销远程端点；
//! - GUI 侧经 [`RemoteGuiConnector`] 连接已发布端点；
//! - [`MockRemoteTransport`] 是内存 / loopback 的 Mock 实现：实现
//!   `transport-api` 的 [`GuiTransportServer`] / [`GuiTransportClient`] /
//!   [`GuiConnection`]，端点为 `TransportEndpoint::Remote`，locality 为
//!   [`ConnectionLocality::Remote`]。
//!
//! Transport 只搬运有界字节帧，不含任何 Agent 业务逻辑；本地与远程 GUI 复用
//! 同一 GUI Connection Protocol（[ADR-027] / [ADR-028]），替换真实实现
//! （[P17-11]）不修改 Agent Core 与 GUI Protocol。
//!
//! [ADR-027]: ../../docs/adr/ADR-027-local-remote-same-protocol.md
//! [ADR-028]: ../../docs/adr/ADR-028-replaceable-remote-transport.md
//! [P17-11]: ../../plan/P17-11-real-remote-transport.md

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
pub use transport_api::{
    ConnectOptions, ConnectionInfo, ConnectionLocality, GuiConnection, GuiListener,
    GuiTransportClient, GuiTransportServer, TransportEndpoint, TransportError, TransportErrorKind,
    TransportFrame,
};

/// Mock 默认单帧上限（字节），与 `gui-protocol::MAX_PROTOCOL_FRAME_BYTES` 一致。
pub const DEFAULT_MAX_FRAME_BYTES: u64 = 1024 * 1024;

/// Mock Adapter 名。
pub const MOCK_ADAPTER: &str = "mock";

// ---------- 可替换 Adapter 占位接口 ----------

/// Provider 的描述信息（CLI 输出与日志用）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteTransportDescription {
    /// Adapter 名（如 `mock`；替换实现时更换）。
    pub adapter: String,
    /// 人类可读名称。
    pub display_name: String,
}

/// `publish` 的输入：端点描述。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemotePublishRequest {
    /// 端点名称（用户可读；Mock 用它生成地址）。
    pub name: String,
}

/// 已发布远程端点的句柄：id 供 `unpublish` 使用，endpoint 供 GUI Server
/// 绑定与 GUI Connector 连接。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemotePublishHandle {
    pub id: String,
    pub endpoint: TransportEndpoint,
}

/// CLI 侧的远程端点生命周期 Adapter（可替换，[ADR-028]）。
///
/// 实现只负责端点发布/撤销与描述，不含 Agent 业务逻辑；发布后的实际监听由
/// CLI 把 [`RemotePublishHandle::endpoint`] 交给 GUI Server 绑定。
#[async_trait]
pub trait RemoteGuiTransportProvider: Send + Sync {
    /// Adapter 描述信息。
    fn describe(&self) -> RemoteTransportDescription;

    /// 发布远程端点，返回句柄（含端点描述）。
    async fn publish(
        &self,
        request: RemotePublishRequest,
    ) -> Result<RemotePublishHandle, TransportError>;

    /// 撤销已发布端点（按 `publish` 返回的 handle id）。
    async fn unpublish(&self, handle_id: &str) -> Result<(), TransportError>;

    /// 撤销已发布端点（按 `publish` 返回的 handle id）：关闭已绑定的监听器、
    /// 销毁端点凭证并使凭证立即失效；实现按各自策略断开已建立连接。撤销后
    /// 对该端点的 `connect` 必须失败。
    async fn revoke(&self, handle_id: &str) -> Result<(), TransportError>;
}

/// GUI 侧的远程连接 Adapter（可替换，[ADR-028]）。
///
/// `connect` 返回的 [`GuiConnection`] 与本地 Transport 返回的是同一抽象，
/// GUI 侧协议流程与本地完全一致（[ADR-027]）。
#[async_trait]
pub trait RemoteGuiConnector: Send + Sync {
    async fn connect(
        &self,
        endpoint: &TransportEndpoint,
        options: ConnectOptions,
    ) -> Result<Box<dyn GuiConnection>, TransportError>;
}

// ---------- Mock 实现 ----------

/// 地址槽位：Provider `publish` 预占（listener 为空），GUI Server `bind`
/// 挂上 listener 后 `connect` 才可建立连接。
#[derive(Debug)]
struct Slot {
    listener: Option<mpsc::UnboundedSender<Box<dyn GuiConnection>>>,
}

/// channel 地址 → 槽位。
type Registry = Mutex<HashMap<String, Slot>>;

/// Mock 远程 Transport：内存 / loopback 实现 [`GuiTransportServer`] /
/// [`GuiTransportClient`]，只接受 [`TransportEndpoint::Remote`]，连接 locality
/// 为 [`ConnectionLocality::Remote`]。帧大小仍按 `max_frame_bytes` 校验，
/// 与真实 Transport 共享同一帧语义。
///
/// 同一实例既可作为 Server（`bind`）也可作为 Client（`connect`）；
/// Provider / Connector 通过共享同一 `Arc` 协作（见
/// [`MockRemoteTransportProvider`] / [`MockRemoteConnector`]）。
#[derive(Debug)]
pub struct MockRemoteTransport {
    registry: Arc<Registry>,
    max_frame_bytes: u64,
    next_id: AtomicU64,
}

impl MockRemoteTransport {
    /// 指定服务端单帧上限（字节）；客户端上限来自 [`ConnectOptions::max_frame_bytes`]。
    pub fn new(max_frame_bytes: u64) -> Self {
        Self {
            registry: Arc::new(Mutex::new(HashMap::new())),
            max_frame_bytes,
            next_id: AtomicU64::new(0),
        }
    }

    fn next_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }
}

impl Default for MockRemoteTransport {
    fn default() -> Self {
        Self::new(DEFAULT_MAX_FRAME_BYTES)
    }
}

impl Clone for MockRemoteTransport {
    fn clone(&self) -> Self {
        Self {
            registry: Arc::clone(&self.registry),
            max_frame_bytes: self.max_frame_bytes,
            next_id: AtomicU64::new(0),
        }
    }
}

#[async_trait]
impl GuiTransportServer for MockRemoteTransport {
    async fn bind(
        &self,
        endpoint: TransportEndpoint,
    ) -> Result<Box<dyn GuiListener>, TransportError> {
        let TransportEndpoint::Remote { address, .. } = endpoint else {
            return Err(transport_error(
                TransportErrorKind::InvalidEndpoint,
                "MockRemoteTransport requires TransportEndpoint::Remote",
            ));
        };
        let mut registry = lock(&self.registry);
        let slot = registry
            .entry(address.clone())
            .or_insert(Slot { listener: None });
        if slot.listener.is_some() {
            return Err(transport_error(
                TransportErrorKind::BindFailed,
                format!("remote endpoint {address:?} is already bound"),
            ));
        }
        let (tx, rx) = mpsc::unbounded_channel::<Box<dyn GuiConnection>>();
        slot.listener = Some(tx);
        drop(registry);
        Ok(Box::new(MockRemoteListener {
            registry: Arc::clone(&self.registry),
            address,
            rx: tokio::sync::Mutex::new(rx),
            closed: AtomicBool::new(false),
        }))
    }
}

#[async_trait]
impl GuiTransportClient for MockRemoteTransport {
    async fn connect(
        &self,
        endpoint: TransportEndpoint,
        options: ConnectOptions,
    ) -> Result<Box<dyn GuiConnection>, TransportError> {
        let TransportEndpoint::Remote { address, .. } = endpoint else {
            return Err(transport_error(
                TransportErrorKind::InvalidEndpoint,
                "MockRemoteTransport requires TransportEndpoint::Remote",
            ));
        };
        let max_frame_bytes = options.max_frame_bytes;
        let tx = {
            let registry = lock(&self.registry);
            registry
                .get(&address)
                .and_then(|slot| slot.listener.clone())
                .ok_or_else(|| {
                    transport_error(
                        TransportErrorKind::ConnectionFailed,
                        format!("no remote listener is bound to address {address:?}"),
                    )
                })?
        };
        let id = self.next_id();
        let (client_tx, server_rx) = mpsc::unbounded_channel::<TransportFrame>();
        let (server_tx, client_rx) = mpsc::unbounded_channel::<TransportFrame>();
        let client_conn = Box::new(MockRemoteConnection::new(
            Some(client_tx),
            client_rx,
            ConnectionInfo {
                connection_id: format!("remote-client-{id}"),
                locality: ConnectionLocality::Remote,
                peer_label: options.client_label,
                encrypted: false,
                max_frame_bytes,
            },
        ));
        let server_conn = Box::new(MockRemoteConnection::new(
            Some(server_tx),
            server_rx,
            ConnectionInfo {
                connection_id: format!("remote-server-{id}"),
                locality: ConnectionLocality::Remote,
                peer_label: None,
                encrypted: false,
                max_frame_bytes,
            },
        ));
        tx.send(server_conn).map_err(|_| {
            transport_error(
                TransportErrorKind::ConnectionFailed,
                format!("remote listener for address {address:?} is closed"),
            )
        })?;
        Ok(client_conn)
    }
}

/// 已绑定的 mock 监听器：`accept` 弹出 `connect` 推入的连接。
pub struct MockRemoteListener {
    registry: Arc<Registry>,
    address: String,
    rx: tokio::sync::Mutex<mpsc::UnboundedReceiver<Box<dyn GuiConnection>>>,
    closed: AtomicBool,
}

#[async_trait]
impl GuiListener for MockRemoteListener {
    async fn accept(&self) -> Result<Box<dyn GuiConnection>, TransportError> {
        if self.closed.load(Ordering::Acquire) {
            return Err(connection_closed("listener is closed"));
        }
        let mut rx = self.rx.lock().await;
        match rx.recv().await {
            Some(connection) => Ok(connection),
            None => Err(connection_closed("remote channel is closed")),
        }
    }

    async fn close(&self) -> Result<(), TransportError> {
        self.closed.store(true, Ordering::Release);
        if let Some(slot) = lock(&self.registry).get_mut(&self.address) {
            slot.listener = None;
        }
        Ok(())
    }
}

/// 单向 channel 对的一端：`send` 写向对端，`receive` 读自对端。
struct MockRemoteConnection {
    tx: Mutex<Option<mpsc::UnboundedSender<TransportFrame>>>,
    rx: tokio::sync::Mutex<mpsc::UnboundedReceiver<TransportFrame>>,
    info: ConnectionInfo,
    closed: AtomicBool,
}

impl MockRemoteConnection {
    fn new(
        tx: Option<mpsc::UnboundedSender<TransportFrame>>,
        rx: mpsc::UnboundedReceiver<TransportFrame>,
        info: ConnectionInfo,
    ) -> Self {
        Self {
            tx: Mutex::new(tx),
            rx: tokio::sync::Mutex::new(rx),
            info,
            closed: AtomicBool::new(false),
        }
    }
}

#[async_trait]
impl GuiConnection for MockRemoteConnection {
    async fn send(&self, frame: TransportFrame) -> Result<(), TransportError> {
        if self.closed.load(Ordering::Acquire) {
            return Err(connection_closed("connection is closed"));
        }
        if frame.as_bytes().len() > self.info.max_frame_bytes as usize {
            return Err(transport_error(
                TransportErrorKind::FrameTooLarge,
                format!(
                    "frame is {} bytes, limit {}",
                    frame.as_bytes().len(),
                    self.info.max_frame_bytes
                ),
            ));
        }
        let tx = self.tx.lock().expect("mock tx lock").as_ref().cloned();
        match tx {
            Some(tx) => tx.send(frame).map_err(|_| {
                self.closed.store(true, Ordering::Release);
                connection_closed("peer closed the connection")
            }),
            None => Err(connection_closed("connection is closed")),
        }
    }

    async fn receive(&self) -> Result<TransportFrame, TransportError> {
        if self.closed.load(Ordering::Acquire) {
            return Err(connection_closed("connection is closed"));
        }
        let mut rx = self.rx.lock().await;
        match rx.recv().await {
            Some(frame) => Ok(frame),
            None => {
                self.closed.store(true, Ordering::Release);
                Err(connection_closed("peer closed the connection"))
            }
        }
    }

    async fn close(&self) -> Result<(), TransportError> {
        if self.closed.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        self.tx.lock().expect("mock tx lock").take();
        Ok(())
    }

    fn info(&self) -> ConnectionInfo {
        self.info.clone()
    }
}

/// Mock Provider：基于共享的 [`MockRemoteTransport`] 发布 / 撤销端点。
///
/// `publish` 生成唯一地址 `mock://<name>-<seq>` 并预占注册表槽位；
/// CLI 随后把句柄的 endpoint 交给 GUI Server `bind`。`unpublish` 移除槽位，
/// 之后对该地址的 `connect` 失败。
#[derive(Debug)]
pub struct MockRemoteTransportProvider {
    transport: Arc<MockRemoteTransport>,
    next: AtomicU64,
}

impl MockRemoteTransportProvider {
    /// 与给定的 transport 共享注册表（CLI / 测试用同一实例发布与连接）。
    pub fn new(transport: Arc<MockRemoteTransport>) -> Self {
        Self {
            transport,
            next: AtomicU64::new(0),
        }
    }

    /// 共享的底层 transport（GUI Server 绑定 / Connector 连接用）。
    pub fn transport(&self) -> &Arc<MockRemoteTransport> {
        &self.transport
    }
}

impl Default for MockRemoteTransportProvider {
    fn default() -> Self {
        Self::new(Arc::new(MockRemoteTransport::default()))
    }
}

#[async_trait]
impl RemoteGuiTransportProvider for MockRemoteTransportProvider {
    fn describe(&self) -> RemoteTransportDescription {
        RemoteTransportDescription {
            adapter: MOCK_ADAPTER.into(),
            display_name: "Mock Remote Transport (loopback)".into(),
        }
    }

    async fn publish(
        &self,
        request: RemotePublishRequest,
    ) -> Result<RemotePublishHandle, TransportError> {
        let seq = self.next.fetch_add(1, Ordering::Relaxed);
        let id = format!("{}-{seq}", sanitize(&request.name));
        let address = format!("mock://{id}");
        let mut registry = lock(&self.transport.registry);
        if registry.contains_key(&address) {
            return Err(transport_error(
                TransportErrorKind::BindFailed,
                format!("remote endpoint {address:?} is already published"),
            ));
        }
        // 预占槽位：后续 GuiServer::bind 在该地址上挂 listener。
        registry.insert(address.clone(), Slot { listener: None });
        drop(registry);
        Ok(RemotePublishHandle {
            id,
            endpoint: TransportEndpoint::Remote {
                address,
                adapter: MOCK_ADAPTER.into(),
            },
        })
    }

    async fn unpublish(&self, handle_id: &str) -> Result<(), TransportError> {
        // Mock 地址由 id 确定性推导；移除槽位后该端点不可再连接。
        let address = format!("mock://{handle_id}");
        let mut registry = lock(&self.transport.registry);
        if registry.remove(&address).is_none() {
            return Err(transport_error(
                TransportErrorKind::Internal,
                format!("unknown remote publish handle {handle_id:?}"),
            ));
        }
        Ok(())
    }

    async fn revoke(&self, handle_id: &str) -> Result<(), TransportError> {
        // Mock 的 revoke 与 unpublish 同语义：移除槽位并关闭已绑定监听器
        // （listener 通道断开后 accept 循环退出），端点不可再连接。
        // 已建立的 Mock 连接按连接自身生命周期结束；真实实现
        // （transport-remote 的 revoke）会额外即时断开已建立连接。
        self.unpublish(handle_id).await
    }
}

/// Mock Connector：把连接请求转发到共享的 [`MockRemoteTransport`]。
#[derive(Debug)]
pub struct MockRemoteConnector {
    transport: Arc<MockRemoteTransport>,
}

impl MockRemoteConnector {
    pub fn new(transport: Arc<MockRemoteTransport>) -> Self {
        Self { transport }
    }
}

#[async_trait]
impl RemoteGuiConnector for MockRemoteConnector {
    async fn connect(
        &self,
        endpoint: &TransportEndpoint,
        options: ConnectOptions,
    ) -> Result<Box<dyn GuiConnection>, TransportError> {
        let TransportEndpoint::Remote { adapter, .. } = endpoint else {
            return Err(transport_error(
                TransportErrorKind::InvalidEndpoint,
                "MockRemoteConnector requires TransportEndpoint::Remote",
            ));
        };
        if adapter != MOCK_ADAPTER {
            return Err(transport_error(
                TransportErrorKind::InvalidEndpoint,
                format!(
                    "MockRemoteConnector only handles adapter {MOCK_ADAPTER:?}, got {adapter:?}"
                ),
            ));
        }
        self.transport.connect(endpoint.clone(), options).await
    }
}

fn transport_error(kind: TransportErrorKind, message: impl Into<String>) -> TransportError {
    TransportError {
        kind,
        message: message.into(),
        retryable: false,
    }
}

fn connection_closed(message: &str) -> TransportError {
    transport_error(TransportErrorKind::ConnectionClosed, message)
}

fn sanitize(name: &str) -> String {
    let mut sanitized = String::with_capacity(name.len());
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
            sanitized.push(ch);
        } else {
            sanitized.push('-');
        }
    }
    if sanitized.is_empty() {
        sanitized.push_str("endpoint");
    }
    sanitized
}

fn lock(inner: &Registry) -> MutexGuard<'_, HashMap<String, Slot>> {
    inner
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options(max_frame_bytes: u64) -> ConnectOptions {
        ConnectOptions {
            timeout_ms: 100,
            client_label: Some("mock-remote-test".into()),
            max_frame_bytes,
        }
    }

    fn remote_endpoint(address: &str) -> TransportEndpoint {
        TransportEndpoint::Remote {
            address: address.into(),
            adapter: MOCK_ADAPTER.into(),
        }
    }

    #[tokio::test]
    async fn round_trip_both_directions_with_remote_locality() {
        let transport = MockRemoteTransport::default();
        let listener = transport
            .bind(remote_endpoint("round-trip"))
            .await
            .expect("bind");
        let accept = tokio::spawn(async move { listener.accept().await });
        let client = transport
            .connect(remote_endpoint("round-trip"), options(1024))
            .await
            .expect("connect");
        let server = accept.await.expect("accept task").expect("accept");

        assert_eq!(server.info().locality, ConnectionLocality::Remote);
        assert_eq!(client.info().locality, ConnectionLocality::Remote);
        assert_eq!(
            client.info().peer_label.as_deref(),
            Some("mock-remote-test")
        );
        assert!(!client.info().encrypted);

        client
            .send(TransportFrame::new(vec![1, 2, 3]))
            .await
            .expect("client send");
        assert_eq!(
            server.receive().await.expect("server receive").as_bytes(),
            &[1, 2, 3]
        );
        server
            .send(TransportFrame::new(vec![9, 8, 7]))
            .await
            .expect("server send");
        assert_eq!(
            client.receive().await.expect("client receive").as_bytes(),
            &[9, 8, 7]
        );

        client.close().await.expect("client close");
        server.close().await.expect("server close");
    }

    #[tokio::test]
    async fn connect_to_unbound_address_fails() {
        let transport = MockRemoteTransport::default();
        let error = match transport
            .connect(remote_endpoint("missing"), options(1024))
            .await
        {
            Err(error) => error,
            Ok(_) => panic!("connect to unbound address must fail"),
        };
        assert_eq!(error.kind, TransportErrorKind::ConnectionFailed);
    }

    #[tokio::test]
    async fn duplicate_bind_fails() {
        let transport = MockRemoteTransport::default();
        let listener = transport
            .bind(remote_endpoint("dup"))
            .await
            .expect("first bind");
        let error = match transport.bind(remote_endpoint("dup")).await {
            Err(error) => error,
            Ok(_) => panic!("duplicate bind must fail"),
        };
        assert_eq!(error.kind, TransportErrorKind::BindFailed);
        listener.close().await.expect("listener close");
    }

    #[tokio::test]
    async fn peer_close_and_listener_close_semantics() {
        let transport = MockRemoteTransport::default();
        let listener = transport
            .bind(remote_endpoint("close"))
            .await
            .expect("bind");
        let listener: Arc<dyn GuiListener> = Arc::from(listener);
        let accept = tokio::spawn({
            let listener = Arc::clone(&listener);
            async move { listener.accept().await }
        });
        let client = transport
            .connect(remote_endpoint("close"), options(1024))
            .await
            .expect("connect");
        let server = accept.await.expect("accept task").expect("accept");

        client.close().await.expect("client close");
        assert_eq!(
            server.receive().await.expect_err("peer closed").kind,
            TransportErrorKind::ConnectionClosed
        );

        listener.close().await.expect("listener close");
        let error = match transport
            .connect(remote_endpoint("close"), options(1024))
            .await
        {
            Err(error) => error,
            Ok(_) => panic!("connect to closed listener must fail"),
        };
        assert_eq!(error.kind, TransportErrorKind::ConnectionFailed);
        server.close().await.expect("server close");
    }

    #[tokio::test]
    async fn frame_size_bound_is_enforced() {
        let transport = MockRemoteTransport::default();
        let listener = transport
            .bind(remote_endpoint("bound"))
            .await
            .expect("bind");
        let accept = tokio::spawn(async move { listener.accept().await });
        let client = transport
            .connect(remote_endpoint("bound"), options(16))
            .await
            .expect("connect");
        let _server = accept.await.expect("accept task").expect("accept");

        let error = client
            .send(TransportFrame::new(vec![0u8; 17]))
            .await
            .expect_err("must reject");
        assert_eq!(error.kind, TransportErrorKind::FrameTooLarge);

        client.close().await.expect("client close");
    }

    #[tokio::test]
    async fn invalid_endpoint_kind_is_rejected() {
        let transport = MockRemoteTransport::default();
        for endpoint in [
            TransportEndpoint::Local {
                address: "nope".into(),
            },
            TransportEndpoint::Memory {
                channel: "nope".into(),
            },
        ] {
            let error = match transport.bind(endpoint.clone()).await {
                Err(error) => error,
                Ok(_) => panic!("must reject non-remote endpoint"),
            };
            assert_eq!(error.kind, TransportErrorKind::InvalidEndpoint);
            let error = match transport.connect(endpoint, options(1024)).await {
                Err(error) => error,
                Ok(_) => panic!("must reject non-remote endpoint"),
            };
            assert_eq!(error.kind, TransportErrorKind::InvalidEndpoint);
        }
    }

    #[tokio::test]
    async fn provider_publish_then_gui_bind_then_connector_connect() {
        let transport = Arc::new(MockRemoteTransport::default());
        let provider = MockRemoteTransportProvider::new(Arc::clone(&transport));

        let description = provider.describe();
        assert_eq!(description.adapter, MOCK_ADAPTER);
        assert!(description.display_name.contains("Mock"));

        let handle = provider
            .publish(RemotePublishRequest {
                name: "my endpoint".into(),
            })
            .await
            .expect("publish");
        assert_eq!(handle.id, "my-endpoint-0");
        let TransportEndpoint::Remote { address, adapter } = &handle.endpoint else {
            panic!("expected remote endpoint");
        };
        assert_eq!(address, "mock://my-endpoint-0");
        assert_eq!(adapter, MOCK_ADAPTER);

        let listener = transport.bind(handle.endpoint.clone()).await.expect("bind");
        let accept = tokio::spawn(async move { listener.accept().await });
        let connector = MockRemoteConnector::new(Arc::clone(&transport));
        let conn = connector
            .connect(&handle.endpoint, options(1024))
            .await
            .expect("connector connect");
        assert_eq!(conn.info().locality, ConnectionLocality::Remote);
        let server = accept.await.expect("accept task").expect("accept");

        conn.send(TransportFrame::new(vec![7])).await.expect("send");
        assert_eq!(server.receive().await.expect("receive").as_bytes(), &[7]);
        conn.close().await.expect("client close");
        server.close().await.expect("server close");
    }

    #[tokio::test]
    async fn provider_unpublish_makes_endpoint_unreachable() {
        let transport = Arc::new(MockRemoteTransport::default());
        let provider = MockRemoteTransportProvider::new(Arc::clone(&transport));
        let handle = provider
            .publish(RemotePublishRequest {
                name: "ephemeral".into(),
            })
            .await
            .expect("publish");
        let listener = transport.bind(handle.endpoint.clone()).await.expect("bind");

        provider.unpublish(&handle.id).await.expect("unpublish");
        let error = match transport
            .connect(handle.endpoint.clone(), options(1024))
            .await
        {
            Err(error) => error,
            Ok(_) => panic!("connect after unpublish must fail"),
        };
        assert_eq!(error.kind, TransportErrorKind::ConnectionFailed);

        // 撤销未知 handle 返回结构化错误。
        let error = match provider.unpublish("never-published").await {
            Err(error) => error,
            Ok(_) => panic!("unpublish of unknown handle must fail"),
        };
        assert_eq!(error.kind, TransportErrorKind::Internal);
        listener.close().await.expect("listener close");
    }

    #[tokio::test]
    async fn connector_rejects_non_mock_adapter_and_non_remote_endpoint() {
        let transport = Arc::new(MockRemoteTransport::default());
        let connector = MockRemoteConnector::new(transport);
        let error = match connector
            .connect(
                &TransportEndpoint::Remote {
                    address: "tcp://somewhere".into(),
                    adapter: "real-tunnel".into(),
                },
                options(1024),
            )
            .await
        {
            Err(error) => error,
            Ok(_) => panic!("must reject foreign adapter"),
        };
        assert_eq!(error.kind, TransportErrorKind::InvalidEndpoint);

        let error = match connector
            .connect(
                &TransportEndpoint::Local {
                    address: "nope".into(),
                },
                options(1024),
            )
            .await
        {
            Err(error) => error,
            Ok(_) => panic!("must reject non-remote endpoint"),
        };
        assert_eq!(error.kind, TransportErrorKind::InvalidEndpoint);
    }
}
