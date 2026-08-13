//! 会话建立：TLS 握手 → 信封级认证 → 续传对齐。
//!
//! 服务端流程（`accept` 路径）：TLS 1.3 握手（rustls）→ 读取
//! [`FrameKind::Auth`] 并校验 `pawork-token` 凭证（按端点独立凭证
//! constant-time 比较；凭证在 `publish` 时按端点单独生成，见
//! [`crate::RealRemoteTransport::publish_endpoint`]）→ 读取
//! [`FrameKind::ResumeRequest`]，在 **按服务端签发 resume identity 隔离** 的
//! 有界重放窗口上决定补发 / 免补发 / 快照信号，然后才把连接交给上层。
//!
//! 客户端流程（`connect` 路径）：TLS 握手（按端点指纹 pin 证书）→ 发送
//! Auth → 收到 AuthOk → 发送 ResumeRequest(last_acked) → 收到 ResumeReply，
//! 并把续传结论记录为 [`ResumeOutcome`]（快照信号）。
//!
//! 安全约定：认证凭证（token）只存在于信封 payload 与 [`TokenStore`] 文件里，
//! 不实现 `Display` / `Debug` 输出，绝不进日志；拒绝原因使用固定文案，
//! 不携带任何 Secret（[ADR-014]）。确认（Ack）必须携带被确认帧的
//! payload 摘要，且只对本会话窗口内实际发送过的序号有效 —— 跨会话 /
//! 凭空确认一律按协议违规断开连接，杜绝跨客户端操纵重放窗口。

use std::collections::HashMap;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use client_auth::{Token, TokenStore, TOKEN_SCHEME};
use sha2::{Digest, Sha256};
use tokio::io::AsyncRead;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Notify;
use tokio_rustls::{TlsAcceptor, TlsConnector};

use transport_api::{
    ConnectOptions, ConnectionInfo, ConnectionLocality, TransportError, TransportErrorKind,
    TransportFrame,
};

use crate::connection::{ClientConnection, ResumeOutcome, ServerConnection};
use crate::tls::{client_config, server_config, TlsIdentity};
use crate::wire::{
    connection_closed, decode_auth, decode_resume_reply, decode_resume_request, encode_auth,
    encode_resume_reply, encode_resume_request, read_envelope, transport_error, write_envelope,
    Envelope, FrameKind, ResumeIdentity, ResumeStatus,
};
use crate::ResumeState;

/// 服务端握手单步超时（TLS / Auth / Resume 各步共用）。
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// 单个服务端签发 resume identity 对应会话的发送方向有界重放窗口。
///
/// 每条 DATA 帧带递增 `seq`；客户端交付即回 Ack。窗口只保留未确认帧，
/// 超出帧数 / 字节上限时丢弃最旧帧 —— 重连时若缺口在窗口内则按序补发，
/// 否则判定 [`ResumeStatus::SnapshotRequired`]。
///
/// 确认校验（[`Self::ack`]）：Ack 载荷为 `[seq][payload sha256]`，只有
/// **本会话窗口实际发送过** 且摘要与所存帧 payload 一致的序号才会推进水位；
/// ACK 必须严格等于 `acked + 1`、目标仍在 buffer 且摘要匹配；跳跃、重复、
/// 中间帧或已淘汰目标均返回 [`AckError`] 由连接层断开。
#[derive(Debug)]
pub(crate) struct SendWindow {
    next_seq: u64,
    acked: u64,
    buffer: VecDeque<(u64, TransportFrame)>,
    /// 有界窗口淘汰造成不可恢复缺口；一旦出现，旧连接后续 ACK 不能再推进
    /// 水位，重连必须 SnapshotRequired 后重新建立基线。
    replay_gap: bool,
    buffered_bytes: u64,
    cap_frames: usize,
    cap_bytes: u64,
}

impl SendWindow {
    pub(crate) fn new(cap_frames: usize, cap_bytes: u64) -> Self {
        Self {
            next_seq: 1,
            acked: 0,
            buffer: VecDeque::new(),
            replay_gap: false,
            buffered_bytes: 0,
            cap_frames,
            cap_bytes,
        }
    }

    /// 分配序号并缓冲一帧；返回帧序号。超过帧数 / 字节容量时丢弃最旧帧
    /// （双重有界）。
    #[cfg(test)]
    pub(crate) fn append(&mut self, frame: &TransportFrame) -> u64 {
        let seq = self.next_seq;
        self.next_seq += 1;
        self.buffered_bytes += frame.as_bytes().len() as u64;
        self.buffer.push_back((seq, frame.clone()));
        while self.buffer.len() > self.cap_frames || self.buffered_bytes > self.cap_bytes {
            let Some((evicted_seq, evicted)) = self.buffer.pop_front() else {
                break;
            };
            if evicted_seq > self.acked {
                self.replay_gap = true;
            }
            self.buffered_bytes = self
                .buffered_bytes
                .saturating_sub(evicted.as_bytes().len() as u64);
        }
        seq
    }

    /// 生产发送路径：窗口有足够帧数与字节预算时追加；否则返回 `None`，
    /// 调用方等待 ACK 释放预算，形成有界背压而非淘汰仍在途帧。
    pub(crate) fn try_append(&mut self, frame: &TransportFrame) -> Option<u64> {
        let frame_bytes = frame.as_bytes().len() as u64;
        if self.buffer.len() >= self.cap_frames
            || self.buffered_bytes.saturating_add(frame_bytes) > self.cap_bytes
        {
            return None;
        }
        let seq = self.next_seq;
        self.next_seq += 1;
        self.buffered_bytes += frame_bytes;
        self.buffer.push_back((seq, frame.clone()));
        Some(seq)
    }

    pub(crate) fn can_ever_fit(&self, frame: &TransportFrame) -> bool {
        self.cap_frames > 0 && frame.as_bytes().len() as u64 <= self.cap_bytes
    }

    /// 处理客户端 Ack：只接受本会话实际发送过且 payload 摘要一致的确认。
    ///
    /// 只有 `seq == acked + 1` 且该帧仍在 buffer、摘要一致才接受。
    pub(crate) fn ack(&mut self, seq: u64, digest: &[u8; 32]) -> Result<(), AckError> {
        let expected = self.acked.saturating_add(1);
        if self.replay_gap {
            return Err(AckError::NotBuffered { seq });
        }
        if seq != expected {
            return Err(AckError::OutOfOrder { seq, expected });
        }
        let Some((_, frame)) = self.buffer.iter().find(|(buffered, _)| *buffered == seq) else {
            return Err(AckError::NotBuffered { seq });
        };
        let actual: [u8; 32] = Sha256::digest(frame.as_bytes()).into();
        if &actual != digest {
            return Err(AckError::DigestMismatch { seq });
        }
        self.acked = seq;
        self.trim_acked();
        Ok(())
    }

    fn trim_acked(&mut self) {
        while let Some((front, _frame)) = self.buffer.front() {
            if *front <= self.acked {
                let (_, evicted) = self.buffer.pop_front().expect("front checked above");
                self.buffered_bytes = self
                    .buffered_bytes
                    .saturating_sub(evicted.as_bytes().len() as u64);
            } else {
                break;
            }
        }
    }

    /// 依据客户端 `last_acked` 决定续传方案（见 [`ResumePlan`]）。
    ///
    /// 安全约束：
    /// - `last_acked == 0`：对端声称从未交付过任何帧（或全新连接）——
    ///   统一回 [`ResumePlan::SnapshotRequired`]，由上层显式重新对齐，
    ///   传输层不猜测补发；
    /// - `last_acked > self.acked`：客户端声称的水位超出本会话服务端已记录
    ///   的确认水位 —— 可能来自其它会话的序号（跨客户端），拒绝补发并回
    ///   快照信号，避免把别的主体的帧回放给当前连接。
    pub(crate) fn resume(&mut self, last_acked: u64) -> ResumePlan {
        if last_acked == 0 || last_acked > self.acked || self.replay_gap {
            self.buffer.clear();
            self.buffered_bytes = 0;
            self.acked = self.next_seq.saturating_sub(1);
            self.replay_gap = false;
            return ResumePlan::SnapshotRequired {
                next_seq: self.next_seq,
            };
        }
        // 丢弃客户端已经交付的（Ack 可能仍在途）。
        while self
            .buffer
            .front()
            .is_some_and(|(front, _)| *front <= last_acked)
        {
            let (_, evicted) = self.buffer.pop_front().expect("front checked above");
            self.buffered_bytes = self
                .buffered_bytes
                .saturating_sub(evicted.as_bytes().len() as u64);
        }
        match self.buffer.front() {
            None => ResumePlan::UpToDate {
                next_seq: self.next_seq,
            },
            Some((front, _)) if *front <= last_acked + 1 => {
                // 重放目标仍须留在 window，直至客户端逐帧 ACK；否则严格 ACK
                // 无法验证重放帧确实属于当前会话且 payload 一致。
                let frames: Vec<(u64, TransportFrame)> = self.buffer.iter().cloned().collect();
                ResumePlan::ResumeFrom {
                    from_seq: last_acked + 1,
                    next_seq: self.next_seq,
                    frames,
                }
            }
            Some(_) => {
                // 缺口超出窗口：清空过期缓冲，要求上层重新对齐。
                self.buffer.clear();
                self.buffered_bytes = 0;
                self.acked = self.next_seq.saturating_sub(1);
                self.replay_gap = false;
                ResumePlan::SnapshotRequired {
                    next_seq: self.next_seq,
                }
            }
        }
    }

    /// 当前已确认水位（诊断 / 测试用）。
    pub(crate) fn acked(&self) -> u64 {
        self.acked
    }

    /// 已缓冲未确认帧数（诊断 / 测试用）。
    pub(crate) fn buffered(&self) -> usize {
        self.buffer.len()
    }

    /// 当前缓冲未确认帧的总字节数（诊断 / 测试用）。
    pub(crate) fn buffered_bytes(&self) -> u64 {
        self.buffered_bytes
    }
}

/// [`SendWindow::ack`] 拒绝的确认（连接层按协议违规断开）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AckError {
    /// ACK 不是严格的下一帧（包括重复、迟到、跳跃与中间帧）。
    OutOfOrder { seq: u64, expected: u64 },
    /// 目标帧已淘汰或从未进入本会话 buffer。
    NotBuffered { seq: u64 },
    /// 序号在本会话发送过，但载荷摘要与所存帧不一致（对端并未收到该帧）。
    DigestMismatch { seq: u64 },
    /// ACK 信封 header 与 payload 声称的序号不一致。
    HeaderMismatch { header_seq: u64, payload_seq: u64 },
}

impl std::fmt::Display for AckError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AckError::OutOfOrder { seq, expected } => {
                write!(
                    formatter,
                    "out-of-order ack for seq {seq} (expected {expected})"
                )
            }
            AckError::NotBuffered { seq } => {
                write!(formatter, "ack target seq {seq} is not in the send window")
            }
            AckError::DigestMismatch { seq } => {
                write!(formatter, "ack for seq {seq} has a payload digest mismatch")
            }
            AckError::HeaderMismatch {
                header_seq,
                payload_seq,
            } => write!(
                formatter,
                "ack header seq {header_seq} does not match payload seq {payload_seq}"
            ),
        }
    }
}

/// [`SendWindow::resume`] 的结论。
#[derive(Debug)]
pub(crate) enum ResumePlan {
    /// 按序补发 `frames`（原序号不变），之后新帧继续递增。
    ResumeFrom {
        from_seq: u64,
        next_seq: u64,
        frames: Vec<(u64, TransportFrame)>,
    },
    /// 无缺失。
    UpToDate { next_seq: u64 },
    /// 缺口超出窗口：不补发，上层需要 Snapshot。
    SnapshotRequired { next_seq: u64 },
}

/// 单个客户端会话的服务端状态：重放窗口与所属连接存活标志。
#[derive(Debug)]
pub(crate) struct SessionRecord {
    /// 首次签发 identity 时绑定的可读 label（只作元数据与诊断；恢复授权由
    /// identity 完成）。identity 携带者改报其它 label 时不得恢复本记录。
    pub(crate) label: String,
    pub(crate) window: Arc<Mutex<SendWindow>>,
    /// 当前占用该会话的连接是否存活。连接关闭后置 false；只有同时持有该
    /// resume identity 且 label 匹配的后续连接才能继承窗口。仍有存活连接时，
    /// 新连接获得全新 identity/window，与旧连接互不共享。
    pub(crate) alive: Arc<AtomicBool>,
}

/// 端点内的会话表：按服务端随机签发的 resume identity 隔离，FIFO 有界；
/// `Auth` label 只是 identity 的附加绑定与诊断元数据，不能单独恢复会话。
#[derive(Debug)]
pub(crate) struct SessionTable {
    records: HashMap<ResumeIdentity, SessionRecord>,
    order: VecDeque<ResumeIdentity>,
    window_frames: usize,
    window_bytes: u64,
}

impl SessionTable {
    fn new(window_frames: usize, window_bytes: u64) -> Self {
        Self {
            records: HashMap::new(),
            order: VecDeque::new(),
            window_frames,
            window_bytes,
        }
    }

    /// 取得（或创建）服务端签发 identity 对应会话的重放窗口与存活标志。
    ///
    /// - 已有会话且其连接已死：复用其窗口（可恢复续传），标记存活；
    /// - 已有会话但其连接仍存活：签发全新 identity + 窗口（并发连接互不
    ///   共享窗口，杜绝跨连接确认 / 回放）；
    /// - 会话数超上限：按 FIFO 淘汰最旧记录（存活连接仍持有自己的窗口 Arc，
    ///   只丢失其续传能力，重连按快照信号重对齐）。
    pub(crate) fn acquire(
        &mut self,
        label: &str,
        requested: Option<ResumeIdentity>,
    ) -> (ResumeIdentity, Arc<Mutex<SendWindow>>, Arc<AtomicBool>) {
        if let Some(identity) = requested {
            if let Some(record) = self.records.get(&identity) {
                if record.label == label && !record.alive.load(Ordering::Acquire) {
                    record.alive.store(true, Ordering::Release);
                    return (
                        identity,
                        Arc::clone(&record.window),
                        Arc::clone(&record.alive),
                    );
                }
            }
        }
        let identity = loop {
            let candidate = ResumeIdentity::generate();
            if !self.records.contains_key(&candidate) {
                break candidate;
            }
        };
        while self.order.len() >= MAX_SESSIONS_PER_ENDPOINT {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            self.records.remove(&oldest);
        }
        let window = Arc::new(Mutex::new(SendWindow::new(
            self.window_frames,
            self.window_bytes,
        )));
        let alive = Arc::new(AtomicBool::new(true));
        self.records.insert(
            identity,
            SessionRecord {
                label: label.to_string(),
                window: Arc::clone(&window),
                alive: Arc::clone(&alive),
            },
        );
        self.order.push_back(identity);
        (identity, window, alive)
    }

    pub(crate) fn window_for_label(&self, label: &str) -> Option<Arc<Mutex<SendWindow>>> {
        self.order.iter().rev().find_map(|identity| {
            self.records
                .get(identity)
                .and_then(|record| (record.label == label).then(|| Arc::clone(&record.window)))
        })
    }
}

/// 每个端点保留的会话记录上限（FIFO 淘汰，防 label 泛洪）。
pub(crate) const MAX_SESSIONS_PER_ENDPOINT: usize = 64;

/// 已发布端点的共享状态：TLS 身份、独立凭证、会话重放窗口与 revoke。
#[derive(Debug)]
pub(crate) struct EndpointState {
    pub(crate) id: String,
    pub(crate) address: String,
    pub(crate) identity: TlsIdentity,
    /// 端点独立凭证（`publish` 时生成；与其它端点互不相同）。
    pub(crate) credential: Token,
    /// 凭证文件句柄：`revoke` 时删除文件，凭证真正失效。
    pub(crate) credential_file: TokenStore,
    pub(crate) revoked: AtomicBool,
    /// 端点是否仍在注册表（`unpublish` 置 false）。
    pub(crate) published: AtomicBool,
    /// 监听 socket 槽位：`bind` 后由 listener 按 accept 借用；`unpublish`
    /// 直接取走并关闭，使新 TCP 连接立即被拒绝。
    pub(crate) listener_slot: Mutex<Option<TcpListener>>,
    /// close/unpublish/revoke 唤醒 pending accept；仅置 closed flag 不足以打断
    /// 已在 socket 上等待的 accept future。
    pub(crate) listener_closed: Notify,
    /// 当前是否有活跃 listener（单占用：同一端点同时只允许一个 bind）。
    pub(crate) bound: AtomicBool,
    pub(crate) sessions: Mutex<SessionTable>,
    pub(crate) max_frame_bytes: u64,
    pub(crate) max_buffered_bytes: u64,
    pub(crate) server_connections: AtomicU64,
}

impl EndpointState {
    #[allow(clippy::too_many_arguments)] // 端点创建时一次性注入的独立配置。
    pub(crate) fn new(
        id: String,
        address: String,
        identity: TlsIdentity,
        credential: Token,
        credential_file: TokenStore,
        max_frame_bytes: u64,
        max_buffered_bytes: u64,
        resend_window_frames: usize,
    ) -> Self {
        Self {
            id,
            address,
            identity,
            credential,
            credential_file,
            revoked: AtomicBool::new(false),
            published: AtomicBool::new(true),
            listener_slot: Mutex::new(None),
            listener_closed: Notify::new(),
            bound: AtomicBool::new(false),
            sessions: Mutex::new(SessionTable::new(resend_window_frames, max_buffered_bytes)),
            max_frame_bytes,
            max_buffered_bytes,
            server_connections: AtomicU64::new(0),
        }
    }

    /// 取得（或创建）服务端签发 identity 对应会话窗口。
    pub(crate) fn acquire_session(
        &self,
        label: &str,
        identity: Option<ResumeIdentity>,
    ) -> (ResumeIdentity, Arc<Mutex<SendWindow>>, Arc<AtomicBool>) {
        self.sessions
            .lock()
            .expect("sessions lock")
            .acquire(label, identity)
    }

    /// 会话窗口查询（诊断 / 测试用）。
    pub(crate) fn session_window(&self, label: &str) -> Option<Arc<Mutex<SendWindow>>> {
        self.sessions
            .lock()
            .expect("sessions lock")
            .window_for_label(label)
    }
}

impl Drop for EndpointState {
    fn drop(&mut self) {
        // 进程退出 / transport Drop 时幂等清凭证，避免同名重启撞上
        // TokenStore::generate 的 create_new。显式 unpublish/revoke 已删过
        // 文件时 NotFound 被忽略。
        self.published.store(false, Ordering::Release);
        match self.listener_slot.lock() {
            Ok(mut slot) => drop(slot.take()),
            Err(poisoned) => drop(poisoned.into_inner().take()),
        }
        self.listener_closed.notify_waiters();
        let _ = self.credential_file.delete();
    }
}

// ---------- 服务端 ----------

/// 接受一条已建立的 TCP 连接：TLS → 认证（端点独立凭证）→ 续传，返回就绪
/// 的服务端连接。
pub(crate) async fn server_handshake(
    tcp: TcpStream,
    state: Arc<EndpointState>,
) -> Result<ServerConnection, TransportError> {
    if state.revoked.load(Ordering::Acquire) {
        return Err(connection_closed("endpoint is revoked"));
    }
    if !state.published.load(Ordering::Acquire) {
        return Err(connection_closed("endpoint is no longer published"));
    }
    let peer = tcp.peer_addr().map_err(|error| {
        transport_error(TransportErrorKind::Internal, format!("peer_addr: {error}"))
    })?;

    let acceptor = TlsAcceptor::from(server_config(&state.identity)?);
    let tls = match tokio::time::timeout(HANDSHAKE_TIMEOUT, acceptor.accept(tcp)).await {
        Ok(Ok(tls)) => tls,
        Ok(Err(error)) => {
            return Err(transport_error(
                TransportErrorKind::ConnectionFailed,
                format!("TLS handshake failed: {error}"),
            ));
        }
        Err(_) => {
            return Err(transport_error(
                TransportErrorKind::Timeout,
                "TLS handshake timed out",
            ));
        }
    };
    let (mut read, mut write) = tokio::io::split(tls);

    // 认证：Auth 帧（scheme / label / proof）。
    let auth = read_handshake_envelope(&mut read, state.max_frame_bytes, "authentication").await?;
    if auth.kind != FrameKind::Auth {
        let _ = write_envelope(
            &mut write,
            FrameKind::AuthRejected,
            0,
            b"authentication required",
        )
        .await;
        return Err(transport_error(
            TransportErrorKind::AuthenticationFailed,
            "peer did not authenticate",
        ));
    }
    let (scheme, label, proof) = match decode_auth(&auth.payload) {
        Ok(parts) => parts,
        Err(error) => {
            // 尽力而为：先回拒绝信封再关闭，失败不覆盖认证错误。
            let _ = write_envelope(
                &mut write,
                FrameKind::AuthRejected,
                0,
                b"malformed authentication",
            )
            .await;
            return Err(error);
        }
    };
    let rejection = authenticate(&state, scheme, proof);
    if let Some(reason) = rejection {
        let reason_bytes = reason.as_bytes();
        let _ = write_envelope(&mut write, FrameKind::AuthRejected, 0, reason_bytes).await;
        tracing::warn!(
            endpoint = %state.id,
            peer = %peer,
            scheme,
            reason,
            "remote authentication rejected"
        );
        return Err(transport_error(
            TransportErrorKind::AuthenticationFailed,
            reason,
        ));
    }
    let _ = write_envelope(&mut write, FrameKind::AuthOk, 0, &[]).await;
    tracing::info!(
        endpoint = %state.id,
        peer = %peer,
        label = %label,
        "remote connection authenticated"
    );

    // 续传对齐：ResumeRequest(last_acked) → ResumeReply。
    if state.revoked.load(Ordering::Acquire) {
        return Err(connection_closed("endpoint is revoked"));
    }
    let resume_request =
        read_handshake_envelope(&mut read, state.max_frame_bytes, "resume").await?;
    if resume_request.kind != FrameKind::ResumeRequest {
        return Err(transport_error(
            TransportErrorKind::ProtocolViolation,
            "expected resume request",
        ));
    }
    let (last_acked, requested_identity) = decode_resume_request(&resume_request.payload)?;
    let (resume_identity, window, session_alive) = state.acquire_session(label, requested_identity);
    let plan = window.lock().expect("window lock").resume(last_acked);
    match &plan {
        ResumePlan::ResumeFrom {
            from_seq,
            next_seq,
            frames,
        } => {
            let reply = encode_resume_reply(ResumeStatus::ResumeFrom, *next_seq, &resume_identity);
            let _ = write_envelope(&mut write, FrameKind::ResumeReply, 0, &reply).await;
            tracing::debug!(
                endpoint = %state.id,
                from_seq,
                replaying = frames.len(),
                "transport resume replay in progress"
            );
            for (seq, frame) in frames {
                if let Err(error) =
                    write_envelope(&mut write, FrameKind::Data, *seq, frame.as_bytes()).await
                {
                    tracing::debug!(endpoint = %state.id, %error, "resume replay interrupted");
                    break;
                }
            }
        }
        ResumePlan::UpToDate { next_seq } => {
            let reply = encode_resume_reply(ResumeStatus::UpToDate, *next_seq, &resume_identity);
            let _ = write_envelope(&mut write, FrameKind::ResumeReply, 0, &reply).await;
        }
        ResumePlan::SnapshotRequired { next_seq } => {
            let reply =
                encode_resume_reply(ResumeStatus::SnapshotRequired, *next_seq, &resume_identity);
            let _ = write_envelope(&mut write, FrameKind::ResumeReply, 0, &reply).await;
            tracing::warn!(
                endpoint = %state.id,
                peer = %peer,
                last_acked,
                "resume window exceeded; snapshot required"
            );
        }
    }

    let connection_id = format!(
        "remote-server-{}",
        state.server_connections.fetch_add(1, Ordering::Relaxed)
    );
    let info = ConnectionInfo {
        connection_id,
        locality: ConnectionLocality::Remote,
        peer_label: Some(label.to_string()),
        encrypted: true,
        max_frame_bytes: state.max_frame_bytes,
    };
    Ok(ServerConnection::new(
        read,
        write,
        state,
        window,
        session_alive,
        info,
    ))
}

/// 校验认证三元组：只接受本端点独立凭证；返回拒绝原因（固定文案，不含
/// Secret）。
fn authenticate(state: &EndpointState, scheme: &str, proof: &str) -> Option<String> {
    if scheme != TOKEN_SCHEME {
        return Some(format!("unsupported authentication scheme {scheme:?}"));
    }
    if state.credential.constant_time_eq(proof) {
        None
    } else {
        Some("invalid token".into())
    }
}

// ---------- 客户端 ----------

/// 客户端建立连接：TCP → TLS（pin 指纹）→ Auth → Resume。
#[allow(clippy::too_many_arguments)] // 握手上下文输入，均为独立来源。
pub(crate) async fn client_handshake(
    tcp: TcpStream,
    fingerprint: [u8; 32],
    token: &Token,
    options: &ConnectOptions,
    resume: ResumeState,
    address: String,
    next_connection_id: &AtomicU64,
    max_buffered_bytes: u64,
) -> Result<ClientConnection, TransportError> {
    let server_name = rustls::pki_types::ServerName::IpAddress(rustls::pki_types::IpAddr::from(
        std::net::Ipv4Addr::LOCALHOST,
    ));
    let connector = TlsConnector::from(client_config(fingerprint)?);
    let tls =
        match tokio::time::timeout(HANDSHAKE_TIMEOUT, connector.connect(server_name, tcp)).await {
            Ok(Ok(tls)) => tls,
            Ok(Err(error)) => {
                return Err(transport_error(
                    TransportErrorKind::ConnectionFailed,
                    format!("TLS handshake failed: {error}"),
                ));
            }
            Err(_) => {
                return Err(transport_error(
                    TransportErrorKind::Timeout,
                    "TLS handshake timed out",
                ));
            }
        };
    let (mut read, mut write) = tokio::io::split(tls);

    // 认证：Auth 帧。
    let label = options.client_label.clone().unwrap_or_default();
    let auth_payload = encode_auth(TOKEN_SCHEME, &label, token.as_str())?;
    write_envelope(&mut write, FrameKind::Auth, 0, &auth_payload).await?;
    let auth_reply =
        read_handshake_envelope(&mut read, options.max_frame_bytes, "authentication").await?;
    match auth_reply.kind {
        FrameKind::AuthOk => {}
        FrameKind::AuthRejected => {
            let reason = String::from_utf8_lossy(&auth_reply.payload);
            return Err(transport_error(
                TransportErrorKind::AuthenticationFailed,
                reason.into_owned(),
            ));
        }
        other => {
            return Err(transport_error(
                TransportErrorKind::AuthenticationFailed,
                format!("unexpected authentication reply {other:?}"),
            ));
        }
    }

    // 续传：ResumeRequest(last_acked) → ResumeReply。
    let label = options.client_label.clone().unwrap_or_default();
    let prior = resume
        .lock()
        .expect("resume map lock")
        .get(&(address.clone(), label.clone()))
        .copied();
    let last_acked = prior.map_or(0, |(last_acked, _)| last_acked);
    write_envelope(
        &mut write,
        FrameKind::ResumeRequest,
        0,
        &encode_resume_request(last_acked, prior.as_ref().map(|(_, identity)| identity)),
    )
    .await?;
    let reply = read_handshake_envelope(&mut read, options.max_frame_bytes, "resume").await?;
    if reply.kind != FrameKind::ResumeReply {
        return Err(transport_error(
            TransportErrorKind::ProtocolViolation,
            "expected resume reply",
        ));
    }
    let (status, next_seq, resume_identity) = decode_resume_reply(&reply.payload)?;
    // SnapshotRequired 表示服务端已丢弃旧传输窗口并以 next_seq-1 为新基线；
    // 客户端必须同步重置本地 transport 水位，否则下一帧会被误判为乱序。
    let aligned_last_acked = if status == ResumeStatus::SnapshotRequired {
        next_seq.saturating_sub(1)
    } else {
        last_acked
    };
    resume.lock().expect("resume map lock").insert(
        (address.clone(), label.clone()),
        (aligned_last_acked, resume_identity),
    );
    let outcome = match status {
        ResumeStatus::ResumeFrom => ResumeOutcome::ResumedFrom(last_acked + 1),
        ResumeStatus::UpToDate => ResumeOutcome::UpToDate,
        ResumeStatus::SnapshotRequired => ResumeOutcome::SnapshotRequired,
    };
    if status == ResumeStatus::SnapshotRequired {
        tracing::warn!(
            endpoint = %address,
            last_acked,
            "transport resume window exceeded; upper layer must snapshot"
        );
    }

    let connection_id = format!(
        "remote-client-{}",
        next_connection_id.fetch_add(1, Ordering::Relaxed)
    );
    let info = ConnectionInfo {
        connection_id,
        locality: ConnectionLocality::Remote,
        peer_label: options.client_label.clone(),
        encrypted: true,
        max_frame_bytes: options.max_frame_bytes,
    };
    Ok(ClientConnection::new(
        read,
        write,
        resume,
        address,
        label,
        outcome,
        info,
        max_buffered_bytes,
    ))
}

/// 握手期信封读取：超时映射为 [`TransportErrorKind::Timeout`]。
async fn read_handshake_envelope<R>(
    read: &mut R,
    max_frame_bytes: u64,
    what: &str,
) -> Result<Envelope, TransportError>
where
    R: AsyncRead + Unpin,
{
    tokio::time::timeout(HANDSHAKE_TIMEOUT, read_envelope(read, max_frame_bytes))
        .await
        .map_err(|_| transport_error(TransportErrorKind::Timeout, format!("{what} timed out")))?
}

#[cfg(test)]
mod tests {
    use super::*;

    fn digest_of(frame: &TransportFrame) -> [u8; 32] {
        Sha256::digest(frame.as_bytes()).into()
    }

    fn frame(payload: &str) -> TransportFrame {
        TransportFrame::new(payload.as_bytes().to_vec())
    }

    #[test]
    fn ack_accepts_only_sent_frames_with_matching_digest() {
        let mut window = SendWindow::new(16, 4096);
        let first = frame("alpha");
        let second = frame("beta");
        let seq1 = window.append(&first);
        let seq2 = window.append(&second);
        assert_eq!((seq1, seq2), (1, 2));

        // 正确摘要：接受并推进水位。
        window.ack(1, &digest_of(&first)).expect("valid ack");
        assert_eq!(window.acked(), 1);
        assert_eq!(window.buffered(), 1);

        // 摘要不一致：拒绝（对端未真正收到该帧）。
        let error = window.ack(2, &[0u8; 32]).expect_err("digest mismatch");
        assert_eq!(error, AckError::DigestMismatch { seq: 2 });
        assert_eq!(window.acked(), 1);

        // 跳跃 / 从未发送过的序号：拒绝。
        let error = window.ack(99, &digest_of(&first)).expect_err("never sent");
        assert_eq!(
            error,
            AckError::OutOfOrder {
                seq: 99,
                expected: 2
            }
        );
        assert_eq!(window.acked(), 1);

        // 重复 / 迟到确认也拒绝。
        assert_eq!(
            window.ack(1, &[0xEE; 32]).expect_err("duplicate ack"),
            AckError::OutOfOrder {
                seq: 1,
                expected: 2
            }
        );
        assert_eq!(window.acked(), 1);

        // 正确确认第二帧后水位推进、缓冲清空。
        window.ack(2, &digest_of(&second)).expect("valid ack");
        assert_eq!(window.acked(), 2);
        assert_eq!(window.buffered(), 0);
    }

    #[test]
    fn ack_of_evicted_frame_and_jump_are_rejected() {
        let mut window = SendWindow::new(2, 4096);
        let frames: Vec<TransportFrame> = (1..=3).map(|i| frame(&format!("payload-{i}"))).collect();
        for f in &frames {
            window.append(f);
        }
        // 帧 1 已被有界淘汰（cap 2）。
        assert_eq!(window.buffered(), 2);
        assert_eq!(window.buffer.front().expect("front").0, 2);

        // 已淘汰帧即使是下一帧也不再可信：目标必须仍在 buffer。
        assert_eq!(
            window.ack(1, &digest_of(&frames[0])).expect_err("evicted"),
            AckError::NotBuffered { seq: 1 }
        );
        assert_eq!(window.acked(), 0);

        // 跳过中间帧确认 seq 2：拒绝。
        let error = window.ack(2, &digest_of(&frames[1])).expect_err("jump");
        assert_eq!(error, AckError::NotBuffered { seq: 2 });
    }

    #[test]
    fn buffer_is_bounded_by_frames_and_bytes() {
        let mut window = SendWindow::new(4, 10);
        let mut total: u64 = 0;
        for _i in 1..=8u64 {
            let payload = "xxxxxxxxxx"; // 10 bytes
            total += payload.len() as u64;
            window.append(&frame(payload));
        }
        assert!(window.buffered() <= 4, "frame cap violated");
        assert!(
            window.buffered_bytes() <= 10,
            "byte cap violated: {}",
            window.buffered_bytes()
        );
        // 已淘汰的字节数不被计入。
        assert_eq!(window.buffered_bytes(), 10);
        assert_eq!(window.next_seq, 9);
        assert!(total > 10);
    }

    #[test]
    fn resume_zero_or_beyond_watermark_requires_snapshot() {
        let mut window = SendWindow::new(16, 4096);
        for i in 1..=3u64 {
            window.append(&frame(&format!("frame-{i}")));
        }
        window.ack(1, &digest_of(&frame("frame-1"))).unwrap();

        // last_acked == 0：显式快照信号（不猜测补发）。
        assert!(matches!(
            window.resume(0),
            ResumePlan::SnapshotRequired { .. }
        ));

        // 重建窗口状态。
        let mut window = SendWindow::new(16, 4096);
        for i in 1..=3u64 {
            window.append(&frame(&format!("frame-{i}")));
        }
        window.ack(1, &digest_of(&frame("frame-1"))).unwrap();

        // last_acked 超出本会话已记录水位（可能来自其它会话）：快照信号。
        assert!(matches!(
            window.resume(2),
            ResumePlan::SnapshotRequired { .. }
        ));
        assert!(matches!(
            window.resume(50),
            ResumePlan::SnapshotRequired { .. }
        ));

        // 水位内且无缺口：按序补发。
        let mut window = SendWindow::new(16, 4096);
        for i in 1..=3u64 {
            window.append(&frame(&format!("frame-{i}")));
        }
        window.ack(1, &digest_of(&frame("frame-1"))).unwrap();
        match window.resume(1) {
            ResumePlan::ResumeFrom {
                from_seq: 2,
                frames,
                ..
            } => {
                assert_eq!(frames.len(), 2);
                assert_eq!(frames[0].0, 2);
                assert_eq!(frames[1].0, 3);
            }
            other => panic!("expected ResumeFrom, got {other:?}"),
        }

        // 水位内缺口超出窗口：快照信号。
        let mut window = SendWindow::new(2, 4096);
        for i in 1..=4u64 {
            window.append(&frame(&format!("frame-{i}")));
        }
        assert!(matches!(
            window.resume(1),
            ResumePlan::SnapshotRequired { .. }
        ));

        // 全部确认：免补发。
        let mut window = SendWindow::new(16, 4096);
        let frames: Vec<TransportFrame> = (1..=2).map(|i| frame(&format!("frame-{i}"))).collect();
        for f in &frames {
            window.append(f);
        }
        window.ack(1, &digest_of(&frames[0])).unwrap();
        window.ack(2, &digest_of(&frames[1])).unwrap();
        assert!(matches!(window.resume(2), ResumePlan::UpToDate { .. }));
    }

    #[test]
    fn session_table_isolates_labels_and_reuses_only_dead_sessions() {
        let mut table = SessionTable::new(16, 4096);
        let (identity_a, window_a, _) = table.acquire("client-a", None);
        let (_identity_b, window_b, _) = table.acquire("client-b", None);
        assert!(!Arc::ptr_eq(&window_a, &window_b));

        // 存活会话不被复用：并发同 label 连接获得全新窗口。
        let (identity_a2, window_a2, alive_a2) = table.acquire("client-a", Some(identity_a));
        assert!(!Arc::ptr_eq(&window_a, &window_a2));
        assert_ne!(identity_a2, identity_a);

        // 当前记录对应 a2 连接；其死亡后同 label 新连接复用该窗口（可恢复续传）。
        alive_a2.store(false, Ordering::Release);
        let (identity_a3, window_a3, _) = table.acquire("client-a", Some(identity_a2));
        assert_eq!(identity_a3, identity_a2);
        assert!(Arc::ptr_eq(&window_a2, &window_a3));
        // 旧连接窗口与复用窗口互不共享。
        assert!(!Arc::ptr_eq(&window_a, &window_a3));
    }

    #[test]
    fn resume_identity_cannot_be_used_with_another_label() {
        let mut table = SessionTable::new(16, 4096);
        let (identity, original, alive) = table.acquire("owner", None);
        alive.store(false, Ordering::Release);

        let (issued, impostor, _) = table.acquire("impostor", Some(identity));
        assert_ne!(
            issued, identity,
            "label mismatch must mint a fresh identity"
        );
        assert!(!Arc::ptr_eq(&original, &impostor));
    }

    #[test]
    fn unknown_resume_identity_cannot_select_an_existing_session() {
        let mut table = SessionTable::new(16, 4096);
        let (owner_identity, owner_window, owner_alive) = table.acquire("shared-label", None);
        owner_alive.store(false, Ordering::Release);

        let forged = loop {
            let candidate = ResumeIdentity::generate();
            if candidate != owner_identity {
                break candidate;
            }
        };
        let (issued, forged_window, _) = table.acquire("shared-label", Some(forged));
        assert_ne!(issued, owner_identity);
        assert_ne!(issued, forged);
        assert!(!Arc::ptr_eq(&owner_window, &forged_window));

        let (resumed, resumed_window, _) = table.acquire("shared-label", Some(owner_identity));
        assert_eq!(resumed, owner_identity);
        assert!(Arc::ptr_eq(&owner_window, &resumed_window));
    }

    #[test]
    fn session_table_evicts_oldest_when_full() {
        let mut table = SessionTable::new(16, 4096);
        table.records = HashMap::new();
        table.order = VecDeque::new();
        // 先建最旧会话，再填满表。
        let (oldest_identity, window_oldest, alive_oldest) = table.acquire("label-0", None);
        for i in 1..MAX_SESSIONS_PER_ENDPOINT {
            let _ = table.acquire(&format!("label-{i}"), None);
        }
        assert_eq!(table.records.len(), MAX_SESSIONS_PER_ENDPOINT);
        // 表满后新 label 到来：按 FIFO 淘汰最旧记录（label-0）。
        let (new_identity, _, _) = table.acquire("label-new", None);
        assert!(!table.records.contains_key(&oldest_identity));
        assert!(table.records.contains_key(&new_identity));
        assert_eq!(table.records.len(), MAX_SESSIONS_PER_ENDPOINT);
        // 被淘汰会话的连接仍持有自己的窗口 Arc（仅失去续传能力）。
        assert!(alive_oldest.load(Ordering::Acquire));
        window_oldest.lock().unwrap().append(&frame("still-alive"));
    }
}
