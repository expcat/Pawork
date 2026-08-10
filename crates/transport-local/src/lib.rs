//! 本地 GUI Transport（P13-4）：Unix Domain Socket（macOS/Linux）与
//! Named Pipe（Windows）。
//!
//! 帧边界遵循 `gui-protocol` 的线上分帧约定：`[u32 LE payload_len][payload]`
//! （见 `gui-protocol::codec` 的 `FRAME_LENGTH_PREFIX_BYTES` /
//! [`write_frame`] 文档）。本 crate 只搬运字节、不解析 JSON；长度前缀在分配
//! 缓冲区之前按上限校验（有界帧），上限与
//! [`gui-protocol::MAX_PROTOCOL_FRAME_BYTES`] 保持一致。
//!
//! [`write_frame`]: gui-protocol::codec::write_frame

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use async_trait::async_trait;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use transport_api::{
    ConnectOptions, ConnectionInfo, ConnectionLocality, GuiConnection, GuiListener,
    GuiTransportClient, GuiTransportServer, TransportEndpoint, TransportError, TransportErrorKind,
    TransportFrame,
};

#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

/// 默认单帧上限，与 `gui-protocol::MAX_PROTOCOL_FRAME_BYTES`（1 MiB）一致，
/// 保证传输层不会截断协议层允许的帧。
pub const DEFAULT_MAX_FRAME_BYTES: u64 = 1024 * 1024;

/// 长度前缀字节数（u32 little-endian），与 `gui-protocol::FRAME_LENGTH_PREFIX_BYTES` 一致。
const LENGTH_PREFIX_BYTES: usize = 4;

/// 本地 Transport：一个类型同时充当 Server 与 Client。
#[derive(Clone, Debug)]
pub struct LocalTransport {
    max_frame_bytes: u64,
}

impl LocalTransport {
    /// 指定服务端单帧上限（字节）。客户端上限来自 [`ConnectOptions::max_frame_bytes`]。
    pub fn new(max_frame_bytes: u64) -> Self {
        Self { max_frame_bytes }
    }
}

impl Default for LocalTransport {
    fn default() -> Self {
        Self::new(DEFAULT_MAX_FRAME_BYTES)
    }
}

#[async_trait]
impl GuiTransportServer for LocalTransport {
    async fn bind(
        &self,
        endpoint: TransportEndpoint,
    ) -> Result<Box<dyn GuiListener>, TransportError> {
        match endpoint {
            TransportEndpoint::Local { address } => {
                #[cfg(unix)]
                {
                    unix::bind(&address, self.max_frame_bytes)
                }
                #[cfg(windows)]
                {
                    windows::bind(&address, self.max_frame_bytes)
                }
            }
            other => Err(transport_error(
                TransportErrorKind::InvalidEndpoint,
                format!("LocalTransport requires TransportEndpoint::Local, got {other:?}"),
            )),
        }
    }
}

#[async_trait]
impl GuiTransportClient for LocalTransport {
    async fn connect(
        &self,
        endpoint: TransportEndpoint,
        options: ConnectOptions,
    ) -> Result<Box<dyn GuiConnection>, TransportError> {
        match endpoint {
            TransportEndpoint::Local { address } => {
                #[cfg(unix)]
                {
                    unix::connect(&address, &options).await
                }
                #[cfg(windows)]
                {
                    windows::connect(&address, &options).await
                }
            }
            other => Err(transport_error(
                TransportErrorKind::InvalidEndpoint,
                format!("LocalTransport requires TransportEndpoint::Local, got {other:?}"),
            )),
        }
    }
}

/// 基于 tokio 分拆读写半部的有界分帧连接。
///
/// 读写各持一把 `tokio::sync::Mutex`，保证 `send` / `receive` / `close` 以
/// `&self` 并发调用安全；同一时刻只允许一个进行中的写（或读）。
pub(crate) struct StreamConnection<R, W> {
    reader: tokio::sync::Mutex<R>,
    writer: tokio::sync::Mutex<W>,
    info: ConnectionInfo,
    closed: AtomicBool,
}

impl<R, W> StreamConnection<R, W>
where
    R: AsyncRead + Unpin + Send,
    W: AsyncWrite + Unpin + Send,
{
    pub(crate) fn new(reader: R, writer: W, info: ConnectionInfo) -> Self {
        Self {
            reader: tokio::sync::Mutex::new(reader),
            writer: tokio::sync::Mutex::new(writer),
            info,
            closed: AtomicBool::new(false),
        }
    }
}

#[async_trait]
impl<R, W> GuiConnection for StreamConnection<R, W>
where
    R: AsyncRead + Unpin + Send + Sync + 'static,
    W: AsyncWrite + Unpin + Send + Sync + 'static,
{
    async fn send(&self, frame: TransportFrame) -> Result<(), TransportError> {
        if self.closed.load(Ordering::Acquire) {
            return Err(connection_closed("connection is closed"));
        }
        let payload = frame.as_bytes();
        if payload.len() > self.info.max_frame_bytes as usize {
            return Err(frame_too_large(payload.len(), self.info.max_frame_bytes));
        }
        let mut writer = self.writer.lock().await;
        let length = u32::try_from(payload.len())
            .map_err(|_| frame_too_large(payload.len(), self.info.max_frame_bytes))?;
        let result = async {
            writer.write_all(&length.to_le_bytes()).await?;
            writer.write_all(payload).await?;
            Ok(())
        }
        .await;
        match result {
            Ok(()) => Ok(()),
            Err(error) if is_peer_gone(&error) => {
                self.closed.store(true, Ordering::Release);
                Err(connection_closed(&format!("send failed: {error}")))
            }
            Err(error) => Err(transport_error(
                TransportErrorKind::Internal,
                format!("send failed: {error}"),
            )),
        }
    }

    async fn receive(&self) -> Result<TransportFrame, TransportError> {
        if self.closed.load(Ordering::Acquire) {
            return Err(connection_closed("connection is closed"));
        }
        let mut reader = self.reader.lock().await;
        let mut header = [0u8; LENGTH_PREFIX_BYTES];
        let mut filled = 0usize;
        while filled < LENGTH_PREFIX_BYTES {
            match reader.read(&mut header[filled..]).await {
                Ok(0) if filled == 0 => {
                    // 帧边界上的干净 EOF：对端正常关闭。
                    self.closed.store(true, Ordering::Release);
                    return Err(connection_closed("peer closed the connection"));
                }
                Ok(0) => {
                    return Err(protocol_violation("truncated frame header"));
                }
                Ok(n) => filled += n,
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) if filled == 0 && is_peer_gone(&error) => {
                    self.closed.store(true, Ordering::Release);
                    return Err(connection_closed(&format!("peer closed: {error}")));
                }
                Err(error) => {
                    return Err(protocol_violation(&format!(
                        "failed to read frame header: {error}"
                    )));
                }
            }
        }
        let declared = u32::from_le_bytes(header) as u64;
        if declared > self.info.max_frame_bytes {
            // 声明长度超限：拒绝（在分配缓冲区之前），流已错位，标记关闭。
            self.closed.store(true, Ordering::Release);
            return Err(frame_too_large(
                declared as usize,
                self.info.max_frame_bytes,
            ));
        }
        let mut payload = vec![0u8; declared as usize];
        reader
            .read_exact(&mut payload)
            .await
            .map_err(|error| protocol_violation(&format!("truncated frame payload: {error}")))?;
        Ok(TransportFrame::new(payload))
    }

    async fn close(&self) -> Result<(), TransportError> {
        if self.closed.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        let mut writer = self.writer.lock().await;
        let _ = writer.shutdown().await;
        Ok(())
    }

    fn info(&self) -> ConnectionInfo {
        self.info.clone()
    }
}

fn is_peer_gone(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::BrokenPipe
            | std::io::ErrorKind::ConnectionReset
            | std::io::ErrorKind::ConnectionAborted
            | std::io::ErrorKind::NotConnected
            | std::io::ErrorKind::UnexpectedEof
    )
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

fn protocol_violation(message: &str) -> TransportError {
    transport_error(TransportErrorKind::ProtocolViolation, message)
}

fn frame_too_large(actual: usize, limit: u64) -> TransportError {
    transport_error(
        TransportErrorKind::FrameTooLarge,
        format!("frame is {actual} bytes, limit {limit}"),
    )
}

fn connection_info(connection_id: String, max_frame_bytes: u64) -> ConnectionInfo {
    ConnectionInfo {
        connection_id,
        locality: ConnectionLocality::Local,
        peer_label: None,
        encrypted: false,
        max_frame_bytes,
    }
}

/// 客户端连接 id 计数器（客户端侧信息展示用）。
static NEXT_CLIENT_CONNECTION_ID: AtomicU64 = AtomicU64::new(0);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_max_frame_matches_protocol_limit() {
        assert_eq!(DEFAULT_MAX_FRAME_BYTES, 1024 * 1024);
    }

    #[tokio::test]
    async fn non_local_endpoint_is_rejected() {
        let transport = LocalTransport::default();
        let result = transport
            .bind(TransportEndpoint::Memory {
                channel: "x".into(),
            })
            .await;
        let error = match result {
            Err(error) => error,
            Ok(_) => panic!("must reject non-local endpoint"),
        };
        assert_eq!(error.kind, TransportErrorKind::InvalidEndpoint);
    }
}
