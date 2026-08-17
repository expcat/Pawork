//! 进程内 GUI Transport（P13-4，测试用）。
//!
//! 通过内存 channel 对实现 [`GuiTransportServer`] / [`GuiTransportClient`] /
//! [`GuiConnection`]，与真实 Transport 共享同一帧语义（`TransportFrame` 只搬运
//! 有界字节），但无需真实 socket。locality 为
//! [`ConnectionLocality::InProcess`]。帧大小仍按 `max_frame_bytes` 校验，
//! 保证测试与线上行为一致。

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use async_trait::async_trait;
use tokio::sync::mpsc;
use crate::{
    ConnectOptions, ConnectionInfo, ConnectionLocality, GuiConnection, GuiListener,
    GuiTransportClient, GuiTransportServer, TransportEndpoint, TransportError, TransportErrorKind,
    TransportFrame,
};

mod mock;
pub use mock::{
    MockRemoteConnector, MockRemoteListener, MockRemoteTransport,
    MockRemoteTransportProvider, MOCK_ADAPTER,
};

/// channel 名 → 已绑定 listener 的入站队列。
type Registry = Mutex<HashMap<String, mpsc::UnboundedSender<Box<dyn GuiConnection>>>>;

/// 进程内 Transport：同一实例既可作为 Server（`bind`）也可作为 Client（`connect`）。
#[derive(Debug)]
pub struct MemoryTransport {
    registry: Arc<Registry>,
    next_id: AtomicU64,
}

impl MemoryTransport {
    /// 指定连接 id 前缀的计数起点（测试确定性 id 用）。
    pub fn new() -> Self {
        Self {
            registry: Arc::new(Mutex::new(HashMap::new())),
            next_id: AtomicU64::new(0),
        }
    }

    fn next_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }
}

impl Default for MemoryTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for MemoryTransport {
    fn clone(&self) -> Self {
        Self {
            registry: Arc::clone(&self.registry),
            next_id: AtomicU64::new(0),
        }
    }
}

#[async_trait]
impl GuiTransportServer for MemoryTransport {
    async fn bind(
        &self,
        endpoint: TransportEndpoint,
    ) -> Result<Box<dyn GuiListener>, TransportError> {
        let TransportEndpoint::Memory { channel } = endpoint else {
            return Err(transport_error(
                TransportErrorKind::InvalidEndpoint,
                "MemoryTransport requires TransportEndpoint::Memory",
            ));
        };
        let mut registry = lock(&self.registry);
        if registry.contains_key(&channel) {
            return Err(transport_error(
                TransportErrorKind::BindFailed,
                format!("memory channel {channel:?} is already bound"),
            ));
        }
        let (tx, rx) = mpsc::unbounded_channel::<Box<dyn GuiConnection>>();
        registry.insert(channel.clone(), tx);
        drop(registry);
        Ok(Box::new(MemoryListener {
            registry: Arc::clone(&self.registry),
            channel,
            rx: tokio::sync::Mutex::new(rx),
            closed: AtomicBool::new(false),
        }))
    }
}

#[async_trait]
impl GuiTransportClient for MemoryTransport {
    async fn connect(
        &self,
        endpoint: TransportEndpoint,
        options: ConnectOptions,
    ) -> Result<Box<dyn GuiConnection>, TransportError> {
        let TransportEndpoint::Memory { channel } = endpoint else {
            return Err(transport_error(
                TransportErrorKind::InvalidEndpoint,
                "MemoryTransport requires TransportEndpoint::Memory",
            ));
        };
        let max_frame_bytes = options.max_frame_bytes;
        let tx = {
            let registry = lock(&self.registry);
            registry.get(&channel).cloned().ok_or_else(|| {
                transport_error(
                    TransportErrorKind::ConnectionFailed,
                    format!("no memory listener is bound to channel {channel:?}"),
                )
            })?
        };
        let id = self.next_id();
        let (client_tx, server_rx) = mpsc::unbounded_channel::<TransportFrame>();
        let (server_tx, client_rx) = mpsc::unbounded_channel::<TransportFrame>();
        let client_conn = Box::new(MemoryConnection::new(
            Some(client_tx),
            client_rx,
            ConnectionInfo {
                connection_id: format!("memory-client-{id}"),
                locality: ConnectionLocality::InProcess,
                peer_label: options.client_label,
                encrypted: false,
                max_frame_bytes,
            },
        ));
        let server_conn = Box::new(MemoryConnection::new(
            Some(server_tx),
            server_rx,
            ConnectionInfo {
                connection_id: format!("memory-server-{id}"),
                locality: ConnectionLocality::InProcess,
                peer_label: None,
                encrypted: false,
                max_frame_bytes,
            },
        ));
        tx.send(server_conn).map_err(|_| {
            transport_error(
                TransportErrorKind::ConnectionFailed,
                format!("memory listener for channel {channel:?} is closed"),
            )
        })?;
        Ok(client_conn)
    }
}

/// 已绑定的内存 listener：`accept` 弹出 `connect` 推入的连接。
pub struct MemoryListener {
    registry: Arc<Registry>,
    channel: String,
    rx: tokio::sync::Mutex<mpsc::UnboundedReceiver<Box<dyn GuiConnection>>>,
    closed: AtomicBool,
}

#[async_trait]
impl GuiListener for MemoryListener {
    async fn accept(&self) -> Result<Box<dyn GuiConnection>, TransportError> {
        if self.closed.load(Ordering::Acquire) {
            return Err(connection_closed("listener is closed"));
        }
        let mut rx = self.rx.lock().await;
        match rx.recv().await {
            Some(connection) => Ok(connection),
            None => Err(connection_closed("memory channel is closed")),
        }
    }

    async fn close(&self) -> Result<(), TransportError> {
        self.closed.store(true, Ordering::Release);
        lock(&self.registry).remove(&self.channel);
        Ok(())
    }
}

/// 单向 channel 对的一端：`send` 写向对端，`receive` 读自对端。
struct MemoryConnection {
    tx: Mutex<Option<mpsc::UnboundedSender<TransportFrame>>>,
    rx: tokio::sync::Mutex<mpsc::UnboundedReceiver<TransportFrame>>,
    info: ConnectionInfo,
    closed: AtomicBool,
}

impl MemoryConnection {
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
impl GuiConnection for MemoryConnection {
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
        let tx = self.tx.lock().expect("memory tx lock").as_ref().cloned();
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
        self.tx.lock().expect("memory tx lock").take();
        Ok(())
    }

    fn info(&self) -> ConnectionInfo {
        self.info.clone()
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

fn lock(
    inner: &Registry,
) -> MutexGuard<'_, HashMap<String, mpsc::UnboundedSender<Box<dyn GuiConnection>>>> {
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
            client_label: Some("memory-test".into()),
            max_frame_bytes,
        }
    }

    fn memory_endpoint(channel: &str) -> TransportEndpoint {
        TransportEndpoint::Memory {
            channel: channel.into(),
        }
    }

    #[tokio::test]
    async fn round_trip_both_directions_with_inprocess_locality() {
        let transport = MemoryTransport::new();
        let listener = transport
            .bind(memory_endpoint("round-trip"))
            .await
            .expect("bind");
        let accept = tokio::spawn(async move { listener.accept().await });
        let client = transport
            .connect(memory_endpoint("round-trip"), options(1024))
            .await
            .expect("connect");
        let server = accept.await.expect("accept task").expect("accept");

        assert_eq!(server.info().locality, ConnectionLocality::InProcess);
        assert_eq!(client.info().locality, ConnectionLocality::InProcess);
        assert_eq!(client.info().peer_label.as_deref(), Some("memory-test"));
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
    async fn connect_to_unbound_channel_fails() {
        let transport = MemoryTransport::new();
        let error = match transport
            .connect(memory_endpoint("missing"), options(1024))
            .await
        {
            Err(error) => error,
            Ok(_) => panic!("connect to unbound channel must fail"),
        };
        assert_eq!(error.kind, TransportErrorKind::ConnectionFailed);
    }

    #[tokio::test]
    async fn duplicate_bind_fails() {
        let transport = MemoryTransport::new();
        let listener = transport
            .bind(memory_endpoint("dup"))
            .await
            .expect("first bind");
        let error = match transport.bind(memory_endpoint("dup")).await {
            Err(error) => error,
            Ok(_) => panic!("duplicate bind must fail"),
        };
        assert_eq!(error.kind, TransportErrorKind::BindFailed);
        listener.close().await.expect("listener close");
    }

    #[tokio::test]
    async fn peer_close_and_listener_close_semantics() {
        let transport = MemoryTransport::new();
        let listener = transport
            .bind(memory_endpoint("close"))
            .await
            .expect("bind");
        let listener: Arc<dyn GuiListener> = Arc::from(listener);
        let accept = tokio::spawn({
            let listener = Arc::clone(&listener);
            async move { listener.accept().await }
        });
        let client = transport
            .connect(memory_endpoint("close"), options(1024))
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
            .connect(memory_endpoint("close"), options(1024))
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
        let transport = MemoryTransport::new();
        let listener = transport
            .bind(memory_endpoint("bound"))
            .await
            .expect("bind");
        let accept = tokio::spawn(async move { listener.accept().await });
        let client = transport
            .connect(memory_endpoint("bound"), options(16))
            .await
            .expect("connect");
        let server = accept.await.expect("accept task").expect("accept");

        let error = client
            .send(TransportFrame::new(vec![0u8; 17]))
            .await
            .expect_err("must reject");
        assert_eq!(error.kind, TransportErrorKind::FrameTooLarge);

        client.close().await.expect("client close");
        server.close().await.expect("server close");
    }

    #[tokio::test]
    async fn invalid_endpoint_kind_is_rejected() {
        let transport = MemoryTransport::new();
        let error = match transport
            .bind(TransportEndpoint::Local {
                address: "nope".into(),
            })
            .await
        {
            Err(error) => error,
            Ok(_) => panic!("must reject non-memory endpoint"),
        };
        assert_eq!(error.kind, TransportErrorKind::InvalidEndpoint);
    }
}
