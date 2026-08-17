//! Unix Domain Socket 端点（macOS/Linux）。

#[cfg(unix)]
use std::os::unix::fs::FileTypeExt;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use async_trait::async_trait;
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::Mutex;
use crate::{
    ConnectOptions, GuiConnection, GuiListener, TransportError, TransportErrorKind,
};

use super::{connection_closed, connection_info, transport_error, StreamConnection};

pub(super) fn bind(
    address: &str,
    max_frame_bytes: u64,
) -> Result<Box<dyn GuiListener>, TransportError> {
    let path = Path::new(address);
    match std::fs::metadata(path) {
        Ok(metadata) if metadata.file_type().is_socket() => {
            // 清理上一次进程遗留的 socket 文件。
            std::fs::remove_file(path).map_err(|error| {
                transport_error(
                    TransportErrorKind::BindFailed,
                    format!("failed to remove stale socket {address}: {error}"),
                )
            })?;
        }
        Ok(_) => {
            return Err(transport_error(
                TransportErrorKind::BindFailed,
                format!("path {address} exists and is not a socket file"),
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(transport_error(
                TransportErrorKind::BindFailed,
                format!("failed to inspect socket path {address}: {error}"),
            ));
        }
    }
    let listener = UnixListener::bind(path).map_err(|error| {
        transport_error(
            TransportErrorKind::BindFailed,
            format!("failed to bind unix socket {address}: {error}"),
        )
    })?;
    Ok(Box::new(UnixSocketListener {
        path: path.to_path_buf(),
        listener: Mutex::new(Some(listener)),
        max_frame_bytes,
        next_connection_id: AtomicU64::new(0),
        closed: AtomicBool::new(false),
    }))
}

pub(super) async fn connect(
    address: &str,
    options: &ConnectOptions,
) -> Result<Box<dyn GuiConnection>, TransportError> {
    let max_frame_bytes = options.max_frame_bytes;
    let stream = tokio::time::timeout(
        std::time::Duration::from_millis(options.timeout_ms),
        UnixStream::connect(Path::new(address)),
    )
    .await
    .map_err(|_| {
        transport_error(
            TransportErrorKind::Timeout,
            format!("connect to unix socket {address} timed out"),
        )
    })?
    .map_err(|error| {
        transport_error(
            TransportErrorKind::ConnectionFailed,
            format!("failed to connect to unix socket {address}: {error}"),
        )
    })?;
    let (reader, writer) = tokio::io::split(stream);
    let info = connection_info(
        format!(
            "client-{}",
            super::NEXT_CLIENT_CONNECTION_ID.fetch_add(1, Ordering::Relaxed)
        ),
        max_frame_bytes,
    );
    Ok(Box::new(StreamConnection::new(reader, writer, info)))
}

struct UnixSocketListener {
    path: std::path::PathBuf,
    listener: Mutex<Option<UnixListener>>,
    max_frame_bytes: u64,
    next_connection_id: AtomicU64,
    closed: AtomicBool,
}

#[async_trait]
impl GuiListener for UnixSocketListener {
    async fn accept(&self) -> Result<Box<dyn GuiConnection>, TransportError> {
        if self.closed.load(Ordering::Acquire) {
            return Err(connection_closed("listener is closed"));
        }
        let guard = self.listener.lock().await;
        let listener = guard
            .as_ref()
            .ok_or_else(|| connection_closed("listener is closed"))?;
        let (stream, _peer_address) = listener.accept().await.map_err(|error| {
            transport_error(
                TransportErrorKind::ConnectionFailed,
                format!("accept failed: {error}"),
            )
        })?;
        let (reader, writer) = tokio::io::split(stream);
        let info = connection_info(
            format!(
                "connection-{}",
                self.next_connection_id.fetch_add(1, Ordering::Relaxed)
            ),
            self.max_frame_bytes,
        );
        Ok(Box::new(StreamConnection::new(reader, writer, info)))
    }

    async fn close(&self) -> Result<(), TransportError> {
        self.closed.store(true, Ordering::Release);
        let mut guard = self.listener.lock().await;
        guard.take(); // drop 监听器，停止接受新连接
        drop(guard);
        std::fs::remove_file(&self.path).map_err(|error| {
            transport_error(
                TransportErrorKind::Internal,
                format!(
                    "failed to remove socket file {}: {error}",
                    self.path.display()
                ),
            )
        })?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::{LocalTransport, DEFAULT_MAX_FRAME_BYTES};
    use crate::{
        ConnectOptions, GuiTransportClient, GuiTransportServer, TransportEndpoint,
        TransportErrorKind, TransportFrame,
    };

    fn options(max_frame_bytes: u64) -> ConnectOptions {
        ConnectOptions {
            timeout_ms: 5_000,
            client_label: None,
            max_frame_bytes,
        }
    }

    fn local_endpoint(path: &std::path::Path) -> TransportEndpoint {
        TransportEndpoint::Local {
            address: path.to_string_lossy().into_owned(),
        }
    }

    #[tokio::test]
    async fn unix_socket_frame_round_trip() {
        let temp = tempfile::tempdir().expect("tempdir");
        let endpoint = local_endpoint(&temp.path().join("gui.sock"));
        let server = LocalTransport::default();
        let client = LocalTransport::default();
        let listener = server.bind(endpoint.clone()).await.expect("bind");
        let accept = tokio::spawn(async move { listener.accept().await });
        let client_conn = client
            .connect(endpoint.clone(), options(DEFAULT_MAX_FRAME_BYTES))
            .await
            .expect("connect");
        let server_conn = accept.await.expect("accept task").expect("accept");

        let payload = b"hello over unix socket".to_vec();
        client_conn
            .send(TransportFrame::new(payload.clone()))
            .await
            .expect("client send");
        assert_eq!(
            server_conn
                .receive()
                .await
                .expect("server receive")
                .as_bytes(),
            &payload
        );

        server_conn
            .send(TransportFrame::new(vec![1, 2, 3]))
            .await
            .expect("server send");
        assert_eq!(
            client_conn
                .receive()
                .await
                .expect("client receive")
                .as_bytes(),
            &[1, 2, 3]
        );

        client_conn.close().await.expect("client close");
        server_conn.close().await.expect("server close");
    }

    #[tokio::test]
    async fn oversized_send_is_rejected_before_writing() {
        let temp = tempfile::tempdir().expect("tempdir");
        let endpoint = local_endpoint(&temp.path().join("gui.sock"));
        let server = LocalTransport::new(64);
        let client = LocalTransport::new(64);
        let listener = server.bind(endpoint.clone()).await.expect("bind");
        let accept = tokio::spawn(async move { listener.accept().await });
        let client_conn = client
            .connect(endpoint, options(64))
            .await
            .expect("connect");
        let server_conn = accept.await.expect("accept task").expect("accept");

        let error = client_conn
            .send(TransportFrame::new(vec![0u8; 100]))
            .await
            .expect_err("oversized frame must be rejected");
        assert_eq!(error.kind, TransportErrorKind::FrameTooLarge);

        server_conn.close().await.expect("server close");
        client_conn.close().await.expect("client close");
    }

    #[tokio::test]
    async fn oversized_declared_length_is_rejected_before_allocation() {
        let temp = tempfile::tempdir().expect("tempdir");
        let endpoint = local_endpoint(&temp.path().join("gui.sock"));
        let server = LocalTransport::default();
        let listener = server.bind(endpoint.clone()).await.expect("bind");
        let accept = tokio::spawn(async move { listener.accept().await });

        // 用原始 std socket 直接写帧头，绕过客户端 send 侧的校验。
        let mut raw = std::os::unix::net::UnixStream::connect(temp.path().join("gui.sock"))
            .expect("raw connect");
        let server_conn = accept.await.expect("accept task").expect("accept");
        let declared = (DEFAULT_MAX_FRAME_BYTES + 1) as u32;
        std::io::Write::write_all(&mut raw, &declared.to_le_bytes()).expect("write header");

        let error = server_conn.receive().await.expect_err("must reject");
        assert_eq!(error.kind, TransportErrorKind::FrameTooLarge);
        drop(raw);
        server_conn.close().await.expect("server close");
    }

    #[tokio::test]
    async fn peer_close_returns_connection_closed_and_listener_close_stops_accepts() {
        let temp = tempfile::tempdir().expect("tempdir");
        let endpoint = local_endpoint(&temp.path().join("gui.sock"));
        let server = LocalTransport::default();
        let client = LocalTransport::default();
        let listener = server.bind(endpoint.clone()).await.expect("bind");
        let accept = tokio::spawn(async move { listener.accept().await });
        let client_conn = client
            .connect(endpoint, options(DEFAULT_MAX_FRAME_BYTES))
            .await
            .expect("connect");
        let server_conn = accept.await.expect("accept task").expect("accept");

        client_conn.close().await.expect("client close");
        let error = server_conn.receive().await.expect_err("peer closed");
        assert_eq!(error.kind, TransportErrorKind::ConnectionClosed);

        let listener = server
            .bind(local_endpoint(&temp.path().join("gui2.sock")))
            .await
            .expect("rebind");
        listener.close().await.expect("listener close");
        let error = match listener.accept().await {
            Err(error) => error,
            Ok(_) => panic!("closed listener"),
        };
        assert_eq!(error.kind, TransportErrorKind::ConnectionClosed);
    }
}
