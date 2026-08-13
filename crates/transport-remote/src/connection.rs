//! 连接封装：帧循环（reader task）、有序确认、按字节 + 帧双重有界入队与
//! 关闭语义。
//!
//! - DATA 帧带发送方单调递增 `seq`；接收方在 [`GuiConnection::receive`]
//!   交付给上层时才回 Ack（交付即确认），未交付的帧留在有界队列里并形成
//!   TCP 背压；服务端据此维护按会话隔离的有界重放窗口
//!   （见 [`crate::session::SendWindow`]）。
//! - Ack 载荷携带被确认帧的 payload 摘要（[`crate::wire::encode_ack`]）；
//!   服务端只接受本连接实际发送且摘要一致的确认，恶意 / 跨会话确认按协议
//!   违规断开（见 [`crate::session::AckError`]）。
//! - 入队侧按帧数与字节数双重有界（[`InboundBudget`]）：上层消费慢时形成
//!   背压而不是无限缓冲。
//! - 服务端 → 客户端方向支持断线续传；客户端 → 服务端方向是请求 / 响应，
//!   不做重放（命令不重发）。
//! - 关闭语义：任一方主动 `close` / 收到 `Close` / 对端 EOF / 端点被 revoke
//!   都会置位 `closed`，此后 `send` / `receive` 返回
//!   [`TransportErrorKind::ConnectionClosed`]。
//!
//! 本模块不接触业务帧内容（opaque [`TransportFrame`]），也不记录任何 Secret。

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use sha2::{Digest, Sha256};
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt, ReadHalf, WriteHalf};
use tokio::net::TcpStream;
use tokio::sync::mpsc::{self, error::TrySendError};
use tokio::sync::Notify;
use tokio_rustls::client::TlsStream as ClientTlsStream;
use tokio_rustls::server::TlsStream as ServerTlsStream;

use transport_api::{ConnectionInfo, TransportError, TransportErrorKind, TransportFrame};

use crate::session::{AckError, EndpointState, SendWindow};
use crate::wire::{self, connection_closed, transport_error, FrameKind};
use crate::ResumeState;

/// reader 轮询关闭 / revoke 标志的间隔（让 `close` / `revoke` 在
/// 空闲连接上也尽快生效）。
const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(100);
/// 入队帧数上限：上层消费慢时形成背压，而不是无限缓冲。
const INBOUND_CAPACITY: usize = 64;

/// 入队字节预算：与帧数上限一起构成双重有界（字节上限由
/// [`crate::RealRemoteTransportConfig::max_buffered_bytes`] 提供）。
#[derive(Debug)]
struct InboundBudget {
    bytes: u64,
    cap_bytes: u64,
}

impl InboundBudget {
    fn new(cap_bytes: u64) -> Self {
        Self {
            bytes: 0,
            cap_bytes,
        }
    }

    /// 尝试预留 `len` 字节；不超限则计入并返回 `true`。
    fn try_reserve(&mut self, len: u64) -> bool {
        if self.bytes + len <= self.cap_bytes {
            self.bytes += len;
            true
        } else {
            false
        }
    }

    /// 释放 `len` 字节（接收方交付后调用）。
    fn release(&mut self, len: u64) {
        self.bytes = self.bytes.saturating_sub(len);
    }
}

/// 续传应答的语义（客户端可经 [`ClientConnection::resume_outcome`] 查询）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResumeOutcome {
    /// 服务端已按序补发 `from_seq` 之后的缓冲帧。
    ResumedFrom(u64),
    /// 客户端已收到全部已发帧，无需补发。
    UpToDate,
    /// 缺口超出可重放窗口：传输层不再补发，上层需要重新对齐（Snapshot）。
    SnapshotRequired,
}

/// 连接两侧共享的传输核心。
struct Shared<S> {
    closed: AtomicBool,
    writer: tokio::sync::Mutex<WriteHalf<S>>,
    inbound_rx: tokio::sync::Mutex<Option<mpsc::Receiver<(TransportFrame, u64)>>>,
    budget: tokio::sync::Mutex<InboundBudget>,
    info: ConnectionInfo,
}

impl<S> Shared<S> {
    fn new(
        writer: WriteHalf<S>,
        receiver: mpsc::Receiver<(TransportFrame, u64)>,
        cap_bytes: u64,
        info: ConnectionInfo,
    ) -> Arc<Self> {
        Arc::new(Self {
            closed: AtomicBool::new(false),
            writer: tokio::sync::Mutex::new(writer),
            inbound_rx: tokio::sync::Mutex::new(Some(receiver)),
            budget: tokio::sync::Mutex::new(InboundBudget::new(cap_bytes)),
            info,
        })
    }
}

/// 服务端连接：持有**本连接会话**的重放窗口、revoke 标志与存活标志。
pub(crate) struct ServerConnection {
    shared: Arc<Shared<ServerTlsStream<TcpStream>>>,
    window: Arc<Mutex<SendWindow>>,
    session_alive: Arc<AtomicBool>,
    window_changed: Arc<Notify>,
    send_lock: tokio::sync::Mutex<()>,
}

/// 客户端连接：持有按 (端点地址, label) 维护的 `last_acked`（跨重连存活，
/// 同进程内不同客户端互不共享）。
///
/// 具体类型公开以便消费方查询 [`ClientConnection::resume_outcome`]
/// （续传 / 快照信号）；连接句柄本身仍以 [`GuiConnection`] 使用。
pub struct ClientConnection {
    shared: Arc<Shared<ClientTlsStream<TcpStream>>>,
    next_seq: AtomicU64,
    resume: ResumeState,
    address: String,
    label: String,
    outcome: Mutex<ResumeOutcome>,
    send_lock: tokio::sync::Mutex<()>,
}

impl ServerConnection {
    pub(crate) fn new(
        read: ReadHalf<ServerTlsStream<TcpStream>>,
        write: WriteHalf<ServerTlsStream<TcpStream>>,
        state: Arc<EndpointState>,
        window: Arc<Mutex<SendWindow>>,
        session_alive: Arc<AtomicBool>,
        info: ConnectionInfo,
    ) -> Self {
        let (inbound_tx, inbound_rx) = mpsc::channel(INBOUND_CAPACITY);
        let shared = Shared::new(write, inbound_rx, state.max_buffered_bytes, info);
        let window_changed = Arc::new(Notify::new());
        let connection = Self {
            shared: Arc::clone(&shared),
            window: Arc::clone(&window),
            session_alive: Arc::clone(&session_alive),
            window_changed: Arc::clone(&window_changed),
            send_lock: tokio::sync::Mutex::new(()),
        };
        tokio::spawn(server_reader_loop(
            read,
            shared,
            state,
            window,
            session_alive,
            window_changed,
            inbound_tx,
        ));
        connection
    }
}

impl Drop for ServerConnection {
    fn drop(&mut self) {
        // 连接消亡：释放会话占用，允许同 label 后续连接安全继承窗口。
        self.session_alive.store(false, Ordering::Release);
    }
}

impl ClientConnection {
    #[allow(clippy::too_many_arguments)] // 全部为独立构造输入，聚类反而降低可读性。
    pub(crate) fn new(
        read: ReadHalf<ClientTlsStream<TcpStream>>,
        write: WriteHalf<ClientTlsStream<TcpStream>>,
        resume: ResumeState,
        address: String,
        label: String,
        outcome: ResumeOutcome,
        info: ConnectionInfo,
        max_buffered_bytes: u64,
    ) -> Self {
        let (inbound_tx, inbound_rx) = mpsc::channel(INBOUND_CAPACITY);
        let shared = Shared::new(write, inbound_rx, max_buffered_bytes, info);
        let connection = Self {
            shared: Arc::clone(&shared),
            next_seq: AtomicU64::new(0),
            resume,
            address,
            label,
            outcome: Mutex::new(outcome),
            send_lock: tokio::sync::Mutex::new(()),
        };
        tokio::spawn(client_reader_loop(read, shared, inbound_tx));
        connection
    }

    /// 本次连接建立时服务端返回的续传结论（见 [`ResumeOutcome`]）。
    pub fn resume_outcome(&self) -> ResumeOutcome {
        self.outcome.lock().expect("resume outcome lock").clone()
    }
}

// ---------- 帧循环 ----------

/// 服务端 reader：交付 DATA、校验 Ack（本会话窗口 + payload 摘要）、观察
/// revoke。
async fn server_reader_loop<S>(
    mut read: ReadHalf<S>,
    shared: Arc<Shared<S>>,
    state: Arc<EndpointState>,
    window: Arc<Mutex<SendWindow>>,
    session_alive: Arc<AtomicBool>,
    window_changed: Arc<Notify>,
    inbound_tx: mpsc::Sender<(TransportFrame, u64)>,
) where
    S: AsyncRead + AsyncWrite + Unpin,
{
    loop {
        if shared.closed.load(Ordering::Acquire) {
            break;
        }
        if state.revoked.load(Ordering::Acquire) || !state.published.load(Ordering::Acquire) {
            // 端点已撤销：主动发 Close + TLS shutdown，让对端（客户端
            // reader）也能感知连接关闭，而不是只在本侧置位标志。
            let _ = close_connection(&shared).await;
            break;
        }
        let envelope = tokio::select! {
            _ = tokio::time::sleep(POLL_INTERVAL) => continue,
            result = wire::read_envelope(&mut read, state.max_frame_bytes) => match result {
                Ok(envelope) => envelope,
                Err(error) => {
                    tracing::debug!(
                        connection_id = %shared.info.connection_id,
                        endpoint = %state.id,
                        %error,
                        "server reader exiting"
                    );
                    break;
                }
            },
        };
        match envelope.kind {
            FrameKind::Data => {
                enqueue_bounded(
                    &shared,
                    &inbound_tx,
                    TransportFrame::new(envelope.payload),
                    envelope.seq,
                    || {
                        shared.closed.load(Ordering::Acquire)
                            || state.revoked.load(Ordering::Acquire)
                            || !state.published.load(Ordering::Acquire)
                    },
                )
                .await;
            }
            FrameKind::Ack => {
                let verdict: Result<(), AckError> = match wire::decode_ack(&envelope.payload) {
                    Ok((seq, digest)) if seq == envelope.seq => {
                        window.lock().expect("window lock").ack(seq, &digest)
                    }
                    Ok((seq, _)) => Err(AckError::HeaderMismatch {
                        header_seq: envelope.seq,
                        payload_seq: seq,
                    }),
                    Err(error) => {
                        tracing::warn!(
                            connection_id = %shared.info.connection_id,
                            endpoint = %state.id,
                            %error,
                            "malformed ack payload; closing connection"
                        );
                        let _ = close_connection(&shared).await;
                        break;
                    }
                };
                if let Err(error) = verdict {
                    tracing::warn!(
                        connection_id = %shared.info.connection_id,
                        endpoint = %state.id,
                        %error,
                        "invalid or malicious ack; closing connection"
                    );
                    let _ = close_connection(&shared).await;
                    break;
                }
                window_changed.notify_one();
            }
            FrameKind::Close => break,
            other => {
                tracing::warn!(
                    connection_id = %shared.info.connection_id,
                    kind = ?other,
                    "protocol violation in server reader"
                );
                break;
            }
        }
    }
    shared.closed.store(true, Ordering::Release);
    session_alive.store(false, Ordering::Release);
    window_changed.notify_one();
}

/// 客户端 reader：交付 DATA、观察对端关闭。
async fn client_reader_loop<S>(
    mut read: ReadHalf<S>,
    shared: Arc<Shared<S>>,
    inbound_tx: mpsc::Sender<(TransportFrame, u64)>,
) where
    S: AsyncRead + AsyncWrite + Unpin,
{
    loop {
        if shared.closed.load(Ordering::Acquire) {
            break;
        }
        let envelope = tokio::select! {
            _ = tokio::time::sleep(POLL_INTERVAL) => continue,
            result = wire::read_envelope(&mut read, shared.info.max_frame_bytes) => match result {
                Ok(envelope) => envelope,
                Err(error) => {
                    tracing::debug!(
                        connection_id = %shared.info.connection_id,
                        %error,
                        "client reader exiting"
                    );
                    break;
                }
            },
        };
        match envelope.kind {
            FrameKind::Data => {
                enqueue_bounded(
                    &shared,
                    &inbound_tx,
                    TransportFrame::new(envelope.payload),
                    envelope.seq,
                    || shared.closed.load(Ordering::Acquire),
                )
                .await;
            }
            FrameKind::Close => break,
            other => {
                tracing::warn!(
                    connection_id = %shared.info.connection_id,
                    kind = ?other,
                    "protocol violation in client reader"
                );
                break;
            }
        }
    }
    shared.closed.store(true, Ordering::Release);
}

/// 有界入队：帧数与字节数双重上限。
///
/// 先按字节预算预留（`max_buffered_bytes`），再写入固定容量通道
/// （[`INBOUND_CAPACITY`] 帧）；两者任一饱和即让出重试，期间观察关闭 /
/// revoke；消费者死亡（通道关闭）即退出。帧保留在本次循环内，不丢失。
async fn enqueue_bounded<S>(
    shared: &Arc<Shared<S>>,
    inbound_tx: &mpsc::Sender<(TransportFrame, u64)>,
    frame: TransportFrame,
    seq: u64,
    should_abort: impl Fn() -> bool,
) where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let mut frame = frame;
    let mut seq = seq;
    loop {
        if should_abort() {
            break;
        }
        let frame_len = frame.as_bytes().len() as u64;
        let reserved = {
            let mut budget = shared.budget.lock().await;
            budget.try_reserve(frame_len)
        };
        if !reserved {
            tokio::time::sleep(POLL_INTERVAL).await;
            continue;
        }
        match inbound_tx.try_send((frame, seq)) {
            Ok(()) => break,
            Err(TrySendError::Full(payload)) => {
                (frame, seq) = payload;
                shared.budget.lock().await.release(frame_len);
                tokio::time::sleep(POLL_INTERVAL).await;
            }
            Err(TrySendError::Closed(_)) => {
                shared.budget.lock().await.release(frame_len);
                break;
            }
        }
    }
}

// ---------- GuiConnection 实现 ----------

async fn send_data<S>(
    shared: &Arc<Shared<S>>,
    seq: u64,
    frame: TransportFrame,
) -> Result<(), TransportError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    if shared.closed.load(Ordering::Acquire) {
        return Err(connection_closed("connection is closed"));
    }
    if frame.as_bytes().len() > shared.info.max_frame_bytes as usize {
        return Err(transport_error(
            TransportErrorKind::FrameTooLarge,
            format!(
                "frame is {} bytes, limit {}",
                frame.as_bytes().len(),
                shared.info.max_frame_bytes
            ),
        ));
    }
    let mut writer = shared.writer.lock().await;
    let result = wire::write_envelope(&mut *writer, FrameKind::Data, seq, frame.as_bytes()).await;
    if result.is_err() {
        shared.closed.store(true, Ordering::Release);
    }
    result
}

/// 从入队接收一帧；消费者关闭或 reader 退出时返回 `ConnectionClosed`。
async fn recv_one<S>(shared: &Arc<Shared<S>>) -> Result<(TransportFrame, u64), TransportError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    if shared.closed.load(Ordering::Acquire) {
        return Err(connection_closed("connection is closed"));
    }
    let mut rx = shared.inbound_rx.lock().await;
    let receiver = rx
        .as_mut()
        .ok_or_else(|| connection_closed("connection is closed"))?;
    let (frame, seq) = match receiver.recv().await {
        Some(item) => item,
        None => {
            shared.closed.store(true, Ordering::Release);
            return Err(connection_closed("peer closed the connection"));
        }
    };
    // 交付后释放字节预算，让 reader 继续入队。
    shared
        .budget
        .lock()
        .await
        .release(frame.as_bytes().len() as u64);
    Ok((frame, seq))
}

async fn close_connection<S>(shared: &Arc<Shared<S>>) -> Result<(), TransportError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    if shared.closed.swap(true, Ordering::AcqRel) {
        return Ok(());
    }
    let mut writer = shared.writer.lock().await;
    // 尽力而为：Close 信封 + TLS close_notify；忽略错误（连接可能已断）。
    let _ = wire::write_envelope(&mut *writer, FrameKind::Close, 0, &[]).await;
    let _ = writer.shutdown().await;
    Ok(())
}

#[async_trait::async_trait]
impl transport_api::GuiConnection for ServerConnection {
    async fn send(&self, frame: TransportFrame) -> Result<(), TransportError> {
        // 序号分配、窗口登记与 TLS 写入必须保持同一顺序；并发 send 不得让
        // seq=N+1 先于 seq=N 上线，否则合法逐帧 ACK 会被误判为跳跃。
        let _send_guard = self.send_lock.lock().await;
        if !self
            .window
            .lock()
            .expect("window lock")
            .can_ever_fit(&frame)
        {
            return Err(transport_error(
                TransportErrorKind::FrameTooLarge,
                "frame exceeds the bounded resend window",
            ));
        }
        loop {
            if self.shared.closed.load(Ordering::Acquire) {
                return Err(connection_closed("connection is closed"));
            }
            let notified = self.window_changed.notified();
            let seq = self.window.lock().expect("window lock").try_append(&frame);
            if let Some(seq) = seq {
                return send_data(&self.shared, seq, frame).await;
            }
            tokio::select! {
                _ = notified => {}
                _ = tokio::time::sleep(POLL_INTERVAL) => {}
            }
        }
    }

    async fn receive(&self) -> Result<TransportFrame, TransportError> {
        // 客户端 → 服务端不做重放，无需回 Ack。
        recv_one(&self.shared).await.map(|(frame, _)| frame)
    }

    async fn close(&self) -> Result<(), TransportError> {
        let result = close_connection(&self.shared).await;
        self.window_changed.notify_one();
        result
    }

    fn info(&self) -> ConnectionInfo {
        self.shared.info.clone()
    }
}

#[async_trait::async_trait]
impl transport_api::GuiConnection for ClientConnection {
    async fn send(&self, frame: TransportFrame) -> Result<(), TransportError> {
        let _send_guard = self.send_lock.lock().await;
        let seq = self.next_seq.fetch_add(1, Ordering::Relaxed) + 1;
        send_data(&self.shared, seq, frame).await
    }

    async fn receive(&self) -> Result<TransportFrame, TransportError> {
        let (frame, seq) = recv_one(&self.shared).await?;
        // 交付即确认：登记 (address, label) 的 last_acked，回 Ack 时携带
        // 被确认帧的 payload 摘要（尽力而为；服务端据此校验）。
        {
            let mut map = self.resume.lock().expect("resume map lock");
            let key = (self.address.clone(), self.label.clone());
            if let Some((last, identity)) = map.get_mut(&key) {
                let _ = identity;
                let expected = last.saturating_add(1);
                if seq != expected {
                    return Err(transport_error(
                        TransportErrorKind::ProtocolViolation,
                        format!(
                            "received out-of-order server frame seq {seq} (expected {expected})"
                        ),
                    ));
                }
                *last = seq;
            }
        }
        let digest: [u8; 32] = Sha256::digest(frame.as_bytes()).into();
        let mut writer = self.shared.writer.lock().await;
        if let Err(error) = wire::write_envelope(
            &mut *writer,
            FrameKind::Ack,
            seq,
            &wire::encode_ack(seq, &digest),
        )
        .await
        {
            tracing::debug!(
                connection_id = %self.shared.info.connection_id,
                %error,
                "ack delivery failed"
            );
        }
        Ok(frame)
    }

    async fn close(&self) -> Result<(), TransportError> {
        close_connection(&self.shared).await
    }

    fn info(&self) -> ConnectionInfo {
        self.shared.info.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inbound_budget_is_bounded_by_bytes_and_releases() {
        let mut budget = InboundBudget::new(100);
        assert!(budget.try_reserve(60));
        assert!(budget.try_reserve(40));
        assert!(!budget.try_reserve(1), "over budget must be rejected");
        assert_eq!(budget.bytes, 100);

        budget.release(60);
        assert!(budget.try_reserve(60));
        budget.release(100);
        budget.release(10); // 过度释放被钳制，不产生下溢。
        assert_eq!(budget.bytes, 0);
    }
}
