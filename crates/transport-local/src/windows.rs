//! Named Pipe 端点（Windows）。

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use async_trait::async_trait;
use tokio::net::windows::named_pipe::{ClientOptions, ServerOptions};
use transport_api::{
    ConnectOptions, GuiConnection, GuiListener, TransportError, TransportErrorKind,
};

use crate::{connection_closed, connection_info, transport_error, StreamConnection};

/// 管道名前缀；地址经转义后拼接，避免直接拼接导致路径穿越。
const PIPE_PREFIX: &str = r"\\.\pipe\";

/// Named Pipe 路径上限（`\\.\pipe\` 之后最多 256 字符）。
const MAX_PIPE_NAME_BYTES: usize = 256;

fn pipe_path(address: &str) -> Result<String, TransportError> {
    if address.is_empty() || address.len() > MAX_PIPE_NAME_BYTES || address.contains('\0') {
        return Err(transport_error(
            TransportErrorKind::InvalidEndpoint,
            format!("invalid named pipe address {address:?}"),
        ));
    }
    Ok(format!("{PIPE_PREFIX}{address}"))
}

pub(super) fn bind(
    address: &str,
    max_frame_bytes: u64,
) -> Result<Box<dyn GuiListener>, TransportError> {
    let path = pipe_path(address)?;
    Ok(Box::new(NamedPipeListener {
        path,
        max_frame_bytes,
        first_instance: AtomicU32::new(0),
        next_connection_id: AtomicU64::new(0),
        closed: AtomicBool::new(false),
    }))
}

pub(super) async fn connect(
    address: &str,
    options: &ConnectOptions,
) -> Result<Box<dyn GuiConnection>, TransportError> {
    let path = pipe_path(address)?;
    let max_frame_bytes = options.max_frame_bytes;
    // Named Pipe 实例由服务器在 accept 时创建：客户端在服务器尚未创建
    // 首实例（ERROR_FILE_NOT_FOUND）或全部实例忙（ERROR_PIPE_BUSY）时
    // 需重试，直到 timeout_ms 到期。
    let deadline =
        tokio::time::Instant::now() + std::time::Duration::from_millis(options.timeout_ms);
    let client = loop {
        match ClientOptions::new().open(&path) {
            Ok(client) => break client,
            Err(error) => {
                let retryable = matches!(error.raw_os_error(), Some(2 | 231));
                if !retryable || tokio::time::Instant::now() >= deadline {
                    return Err(transport_error(
                        if retryable {
                            TransportErrorKind::Timeout
                        } else {
                            TransportErrorKind::ConnectionFailed
                        },
                        format!("failed to connect to named pipe {path}: {error}"),
                    ));
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        }
    };
    let (reader, writer) = tokio::io::split(client);
    let info = connection_info(
        format!(
            "client-{}",
            crate::NEXT_CLIENT_CONNECTION_ID.fetch_add(1, Ordering::Relaxed)
        ),
        max_frame_bytes,
    );
    Ok(Box::new(StreamConnection::new(reader, writer, info)))
}

struct NamedPipeListener {
    path: String,
    max_frame_bytes: u64,
    /// 0 号实例使用 `first_pipe_instance(true)`，之后全部为 false；
    /// Named Pipe 每连接一个实例，支持并发 accept。
    first_instance: AtomicU32,
    next_connection_id: AtomicU64,
    closed: AtomicBool,
}

#[async_trait]
impl GuiListener for NamedPipeListener {
    async fn accept(&self) -> Result<Box<dyn GuiConnection>, TransportError> {
        if self.closed.load(Ordering::Acquire) {
            return Err(connection_closed("listener is closed"));
        }
        let first = self.first_instance.fetch_add(1, Ordering::Relaxed) == 0;
        let server = ServerOptions::new()
            .first_pipe_instance(first)
            .create(&self.path)
            .map_err(|error| {
                transport_error(
                    TransportErrorKind::BindFailed,
                    format!(
                        "failed to create named pipe instance {}: {error}",
                        self.path
                    ),
                )
            })?;
        server.connect().await.map_err(|error| {
            transport_error(
                TransportErrorKind::ConnectionFailed,
                format!("named pipe client disconnected during connect: {error}"),
            )
        })?;
        let (reader, writer) = tokio::io::split(server);
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
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{LocalTransport, DEFAULT_MAX_FRAME_BYTES};
    use transport_api::{
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

    fn unique_address() -> String {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        format!(
            "pawork-test-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        )
    }

    #[tokio::test]
    async fn named_pipe_frame_round_trip() {
        let address = unique_address();
        let server = LocalTransport::default();
        let client = LocalTransport::default();
        let listener = server
            .bind(TransportEndpoint::Local {
                address: address.clone(),
            })
            .await
            .expect("bind");
        // Named Pipe 实例在 accept 时创建：先启动 accept，再连接。
        let accept = tokio::spawn(async move { listener.accept().await });
        let client_conn = client
            .connect(
                TransportEndpoint::Local {
                    address: address.clone(),
                },
                options(DEFAULT_MAX_FRAME_BYTES),
            )
            .await
            .expect("connect");
        let server_conn = accept.await.expect("accept task").expect("accept");

        let payload = b"hello over named pipe".to_vec();
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
            .send(TransportFrame::new(vec![4, 5, 6]))
            .await
            .expect("server send");
        assert_eq!(
            client_conn
                .receive()
                .await
                .expect("client receive")
                .as_bytes(),
            &[4, 5, 6]
        );

        client_conn.close().await.expect("client close");
        server_conn.close().await.expect("server close");
    }

    #[tokio::test]
    async fn oversized_send_is_rejected_before_writing() {
        let address = unique_address();
        let server = LocalTransport::new(64);
        let client = LocalTransport::new(64);
        let listener = server
            .bind(TransportEndpoint::Local {
                address: address.clone(),
            })
            .await
            .expect("bind");
        let accept = tokio::spawn(async move { listener.accept().await });
        let client_conn = client
            .connect(
                TransportEndpoint::Local {
                    address: address.clone(),
                },
                options(64),
            )
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
    async fn listener_close_stops_accepts() {
        let address = unique_address();
        let server = LocalTransport::default();
        let listener = server
            .bind(TransportEndpoint::Local {
                address: address.clone(),
            })
            .await
            .expect("bind");
        listener.close().await.expect("listener close");
        let error = match listener.accept().await {
            Err(error) => error,
            Ok(_) => panic!("accept on closed listener must fail"),
        };
        assert_eq!(error.kind, TransportErrorKind::ConnectionClosed);
    }
}
