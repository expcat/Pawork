//! 真实远程 Transport 的线上信封协议。
//!
//! 业务帧以 opaque 字节流搬运（[`transport_api::TransportFrame`]），本模块只
//! 负责传输层的认证 / 排序 / 确认 / 续传信封，不解析业务内容（[ADR-027] /
//! [ADR-028]）。线上格式：
//!
//! ```text
//! [magic u16 LE][version u8][kind u8][seq u64 LE][len u32 LE][payload]
//! ```
//!
//! - `magic` 固定为 `0x5057`（"PW"），用于快速识别非本协议流量；
//! - `version` 当前为 3，不匹配即拒绝（协议版本校验；v2 起 Ack 载荷为
//!   `[seq u64][payload sha256]`，服务端据此校验确认只针对本连接实际发送
//!   且 payload 一致的帧；v3 起 resume 使用服务端签发的 opaque identity）；
//! - `kind` 见 [`FrameKind`]；
//! - `seq` 为发送方单调递增序号（DATA 帧有效，控制帧置 0）；
//! - `len` 为 payload 字节数；读取侧在分配缓冲区之前按有界上限校验。
//!
//! [ADR-027]: ../../../docs/adr/ADR-027-local-remote-same-protocol.md
//! [ADR-028]: ../../../docs/adr/ADR-028-replaceable-remote-transport.md

use rand::RngCore;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use transport_api::{TransportError, TransportErrorKind};

/// 信封魔数（"PW" 的 little-endian 编码）。
pub(crate) const MAGIC: u16 = 0x5057;
/// 信封协议版本（v3：Ack 摘要 + 服务端签发 resume identity）。
pub(crate) const VERSION: u8 = 3;
/// 信封头部字节数：magic(2) + version(1) + kind(1) + seq(8) + len(4)。
pub(crate) const HEADER_BYTES: usize = 16;
/// 控制帧（认证 / 续传等）payload 上限；与业务帧上限分开的有界校验。
pub(crate) const CONTROL_MAX_BYTES: u64 = 1024;
/// Ack 载荷字节数：seq(8) + sha256(payload)(32)。
pub(crate) const ACK_PAYLOAD_BYTES: usize = 8 + 32;

/// 信封帧类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FrameKind {
    /// 业务帧（opaque payload，携带单调递增 `seq`）。
    Data,
    /// 接收方确认：payload 为 `[seq u64 LE][sha256(payload) 32B]`
    /// （见 [`encode_ack`] / [`decode_ack`]）。
    Ack,
    /// 客户端认证请求：payload 为认证三元组（scheme / label / proof）。
    Auth,
    /// 服务端认证通过：payload 为空。
    AuthOk,
    /// 服务端认证拒绝：payload 为 `[u8 len][reason]`，reason 不含任何 secret。
    AuthRejected,
    /// 客户端续传请求：payload 为 `u64` 最后已交付序号（小端）。
    ResumeRequest,
    /// 服务端续传应答：payload 为 `[u8 status][u64 next_seq]`（小端）。
    ResumeReply,
    /// 对端主动关闭：payload 为空。
    Close,
}

impl FrameKind {
    pub(crate) fn as_byte(self) -> u8 {
        match self {
            FrameKind::Data => 1,
            FrameKind::Ack => 2,
            FrameKind::Auth => 3,
            FrameKind::AuthOk => 4,
            FrameKind::AuthRejected => 5,
            FrameKind::ResumeRequest => 6,
            FrameKind::ResumeReply => 7,
            FrameKind::Close => 8,
        }
    }

    pub(crate) fn from_byte(byte: u8) -> Option<Self> {
        match byte {
            1 => Some(FrameKind::Data),
            2 => Some(FrameKind::Ack),
            3 => Some(FrameKind::Auth),
            4 => Some(FrameKind::AuthOk),
            5 => Some(FrameKind::AuthRejected),
            6 => Some(FrameKind::ResumeRequest),
            7 => Some(FrameKind::ResumeReply),
            8 => Some(FrameKind::Close),
            _ => None,
        }
    }
}

/// 已解析的信封。
#[derive(Debug)]
pub(crate) struct Envelope {
    pub(crate) kind: FrameKind,
    pub(crate) seq: u64,
    pub(crate) payload: Vec<u8>,
}

/// 读取信封：先读头部，校验 magic / version / kind 与长度上界，再读 payload。
///
/// `data_max_bytes` 为 DATA 帧上限；控制帧统一按 [`CONTROL_MAX_BYTES`] 校验。
/// 长度上界校验发生在分配缓冲区之前，防止损坏或恶意帧头声明超大长度。
pub(crate) async fn read_envelope<R>(
    reader: &mut R,
    data_max_bytes: u64,
) -> Result<Envelope, TransportError>
where
    R: AsyncRead + Unpin,
{
    let mut header = [0u8; HEADER_BYTES];
    reader.read_exact(&mut header).await.map_err(|error| {
        if is_eof(&error) {
            io_error(TransportErrorKind::ConnectionClosed, "peer closed", &error)
        } else {
            io_error(
                TransportErrorKind::ProtocolViolation,
                "failed to read frame header",
                &error,
            )
        }
    })?;
    let magic = u16::from_le_bytes([header[0], header[1]]);
    if magic != MAGIC {
        return Err(protocol_error("bad envelope magic"));
    }
    if header[2] != VERSION {
        return Err(protocol_error(format!(
            "unsupported envelope version {}",
            header[2]
        )));
    }
    let kind = FrameKind::from_byte(header[3])
        .ok_or_else(|| protocol_error(format!("unknown envelope kind {}", header[3])))?;
    let seq = u64::from_le_bytes(header[4..12].try_into().expect("8-byte seq"));
    let len = u32::from_le_bytes(header[12..16].try_into().expect("4-byte len")) as u64;
    let limit = if kind == FrameKind::Data {
        data_max_bytes
    } else {
        CONTROL_MAX_BYTES
    };
    if len > limit {
        return Err(transport_error(
            TransportErrorKind::FrameTooLarge,
            format!("envelope {kind:?} declares {len} bytes, limit {limit}"),
        ));
    }
    let mut payload = vec![0u8; len as usize];
    reader.read_exact(&mut payload).await.map_err(|error| {
        if is_eof(&error) {
            io_error(
                TransportErrorKind::ConnectionClosed,
                "peer closed mid-frame",
                &error,
            )
        } else {
            io_error(
                TransportErrorKind::ProtocolViolation,
                "failed to read frame payload",
                &error,
            )
        }
    })?;
    Ok(Envelope { kind, seq, payload })
}

/// 写入信封并 flush（TLS 层有内部缓冲，必须 flush 才会落到 TCP）。
pub(crate) async fn write_envelope<W>(
    writer: &mut W,
    kind: FrameKind,
    seq: u64,
    payload: &[u8],
) -> Result<(), TransportError>
where
    W: AsyncWrite + Unpin,
{
    let len = u32::try_from(payload.len())
        .map_err(|_| transport_error(TransportErrorKind::FrameTooLarge, "frame too large"))?;
    let mut header = [0u8; HEADER_BYTES];
    header[0..2].copy_from_slice(&MAGIC.to_le_bytes());
    header[2] = VERSION;
    header[3] = kind.as_byte();
    header[4..12].copy_from_slice(&seq.to_le_bytes());
    header[12..16].copy_from_slice(&len.to_le_bytes());
    writer
        .write_all(&header)
        .await
        .map_err(|error| io_error(TransportErrorKind::ConnectionClosed, "send failed", &error))?;
    writer
        .write_all(payload)
        .await
        .map_err(|error| io_error(TransportErrorKind::ConnectionClosed, "send failed", &error))?;
    writer.flush().await.map_err(|error| {
        io_error(
            TransportErrorKind::ConnectionClosed,
            "send flush failed",
            &error,
        )
    })?;
    Ok(())
}

/// 编码确认载荷：`[seq u64 LE][payload sha256 32B]`。
///
/// 摘要覆盖被确认帧的 payload（opaque 字节）。服务端只接受针对本连接
/// 实际发送且摘要一致的帧的确认，杜绝跨会话 / 凭空确认。
pub(crate) fn encode_ack(seq: u64, digest: &[u8; 32]) -> [u8; ACK_PAYLOAD_BYTES] {
    let mut payload = [0u8; ACK_PAYLOAD_BYTES];
    payload[..8].copy_from_slice(&seq.to_le_bytes());
    payload[8..].copy_from_slice(digest);
    payload
}

/// 解析确认载荷，返回 `(seq, digest)`。
pub(crate) fn decode_ack(payload: &[u8]) -> Result<(u64, [u8; 32]), TransportError> {
    let bytes: [u8; ACK_PAYLOAD_BYTES] = payload
        .try_into()
        .map_err(|_| protocol_error("malformed ack payload"))?;
    let seq = u64::from_le_bytes(bytes[..8].try_into().expect("8-byte seq"));
    let digest: [u8; 32] = bytes[8..].try_into().expect("32-byte digest");
    Ok((seq, digest))
}

/// 认证载荷：`scheme\0label\0proof` 三段，NUL 分隔。
///
/// `proof` 是配对凭证（token），只作为信封 payload 的字节存在：不实现
/// `Display` / `Debug` 输出，绝不进入日志（[ADR-014]）。
/// 客户端 label 上限（防控制帧超界，实际远小于 [`CONTROL_MAX_BYTES`]）。
pub(crate) const AUTH_LABEL_MAX_BYTES: usize = 256;

/// 服务端签发的不可预测 resume bearer identity。identity 只在 TLS 控制帧
/// 内传输且日志中始终脱敏；仅持有 endpoint token 无法凭 label 冒用会话。
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct ResumeIdentity([u8; 32]);

impl ResumeIdentity {
    pub(crate) fn generate() -> Self {
        let mut bytes = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut bytes);
        Self(bytes)
    }

    fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub(crate) fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl std::fmt::Debug for ResumeIdentity {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ResumeIdentity(\"***\")")
    }
}

/// ResumeRequest payload：`last_acked u64 LE` + identity presence byte +
/// optional 32-byte identity。首连没有 identity；服务端在 ResumeReply 中签发。
pub(crate) fn encode_resume_request(last_acked: u64, identity: Option<&ResumeIdentity>) -> Vec<u8> {
    let mut payload = Vec::with_capacity(41);
    payload.extend_from_slice(&last_acked.to_le_bytes());
    match identity {
        Some(identity) => {
            payload.push(1);
            payload.extend_from_slice(identity.as_bytes());
        }
        None => payload.push(0),
    }
    payload
}

pub(crate) fn decode_resume_request(
    payload: &[u8],
) -> Result<(u64, Option<ResumeIdentity>), TransportError> {
    if payload.len() != 9 && payload.len() != 41 {
        return Err(protocol_error("malformed resume request payload"));
    }
    let last_acked = u64::from_le_bytes(payload[..8].try_into().expect("8-byte seq"));
    match (payload[8], payload.len()) {
        (0, 9) => Ok((last_acked, None)),
        (1, 41) => Ok((
            last_acked,
            Some(ResumeIdentity::from_bytes(
                payload[9..].try_into().expect("32-byte identity"),
            )),
        )),
        _ => Err(protocol_error("malformed resume identity")),
    }
}

pub(crate) fn encode_auth(
    scheme: &str,
    label: &str,
    proof: &str,
) -> Result<Vec<u8>, TransportError> {
    if label.len() > AUTH_LABEL_MAX_BYTES {
        return Err(transport_error(
            TransportErrorKind::ProtocolViolation,
            "authentication label is too long",
        ));
    }
    let mut payload = Vec::with_capacity(scheme.len() + label.len() + proof.len() + 2);
    payload.extend_from_slice(scheme.as_bytes());
    payload.push(0);
    payload.extend_from_slice(label.as_bytes());
    payload.push(0);
    payload.extend_from_slice(proof.as_bytes());
    Ok(payload)
}

/// 解析认证载荷；返回 `(scheme, label, proof)`。
pub(crate) fn decode_auth(payload: &[u8]) -> Result<(&str, &str, &str), TransportError> {
    let mut fields = payload.split(|byte| *byte == 0);
    let scheme = fields
        .next()
        .ok_or_else(|| protocol_error("empty authentication payload"))?;
    let label = fields
        .next()
        .ok_or_else(|| protocol_error("authentication payload missing label"))?;
    let proof = fields
        .next()
        .ok_or_else(|| protocol_error("authentication payload missing proof"))?;
    if fields.next().is_some() {
        return Err(protocol_error("authentication payload has extra fields"));
    }
    let scheme = std::str::from_utf8(scheme)
        .map_err(|_| protocol_error("authentication scheme is not UTF-8"))?;
    let label = std::str::from_utf8(label)
        .map_err(|_| protocol_error("authentication label is not UTF-8"))?;
    let proof = std::str::from_utf8(proof)
        .map_err(|_| protocol_error("authentication proof is not UTF-8"))?;
    if scheme.is_empty() || proof.is_empty() {
        return Err(protocol_error(
            "authentication scheme and proof must be non-empty",
        ));
    }
    Ok((scheme, label, proof))
}

/// 续传应答状态。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ResumeStatus {
    /// 服务端将按序补发 `last_acked` 之后的缓冲帧。
    ResumeFrom,
    /// 客户端已收到全部已发帧，无需补发。
    UpToDate,
    /// 缺口超出可重放窗口，需要上层重新对齐（Snapshot）。
    SnapshotRequired,
}

impl ResumeStatus {
    pub(crate) fn as_byte(self) -> u8 {
        match self {
            ResumeStatus::ResumeFrom => 0,
            ResumeStatus::UpToDate => 1,
            ResumeStatus::SnapshotRequired => 2,
        }
    }

    fn from_byte(byte: u8) -> Result<Self, TransportError> {
        match byte {
            0 => Ok(ResumeStatus::ResumeFrom),
            1 => Ok(ResumeStatus::UpToDate),
            2 => Ok(ResumeStatus::SnapshotRequired),
            _ => Err(protocol_error(format!("unknown resume status {byte}"))),
        }
    }
}

/// 编码续传应答：`[status u8][next_seq u64 LE][resume identity 32B]`。
pub(crate) fn encode_resume_reply(
    status: ResumeStatus,
    next_seq: u64,
    identity: &ResumeIdentity,
) -> [u8; 41] {
    let mut payload = [0u8; 41];
    payload[0] = status.as_byte();
    payload[1..9].copy_from_slice(&next_seq.to_le_bytes());
    payload[9..].copy_from_slice(identity.as_bytes());
    payload
}

/// 解析续传应答，返回 `(status, next_seq, identity)`。
pub(crate) fn decode_resume_reply(
    payload: &[u8],
) -> Result<(ResumeStatus, u64, ResumeIdentity), TransportError> {
    if payload.len() != 41 {
        return Err(protocol_error("malformed resume reply payload"));
    }
    let status = ResumeStatus::from_byte(payload[0])?;
    let next_seq = u64::from_le_bytes(payload[1..9].try_into().expect("8-byte next seq"));
    let identity =
        ResumeIdentity::from_bytes(payload[9..].try_into().expect("32-byte resume identity"));
    Ok((status, next_seq, identity))
}

fn is_eof(error: &std::io::Error) -> bool {
    error.kind() == std::io::ErrorKind::UnexpectedEof
        || error.kind() == std::io::ErrorKind::ConnectionReset
        || error.kind() == std::io::ErrorKind::BrokenPipe
}

fn io_error(kind: TransportErrorKind, message: &str, source: &std::io::Error) -> TransportError {
    transport_error(kind, format!("{message}: {source}"))
}

fn protocol_error(message: impl Into<String>) -> TransportError {
    transport_error(TransportErrorKind::ProtocolViolation, message)
}

pub(crate) fn transport_error(
    kind: TransportErrorKind,
    message: impl Into<String>,
) -> TransportError {
    let retryable = matches!(
        &kind,
        TransportErrorKind::ConnectionFailed
            | TransportErrorKind::ConnectionClosed
            | TransportErrorKind::Timeout
    );
    TransportError {
        kind,
        message: message.into(),
        retryable,
    }
}

pub(crate) fn connection_closed(message: &str) -> TransportError {
    transport_error(TransportErrorKind::ConnectionClosed, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(kind: FrameKind, seq: u64, payload: &[u8]) {
        let mut bytes = Vec::new();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        runtime.block_on(async {
            write_envelope(&mut bytes, kind, seq, payload)
                .await
                .expect("write");
            let mut reader = bytes.as_slice();
            let envelope = read_envelope(&mut reader, 1024).await.expect("read");
            assert_eq!(envelope.kind, kind);
            assert_eq!(envelope.seq, seq);
            assert_eq!(envelope.payload, payload);
            assert!(reader.is_empty());
        });
    }

    #[test]
    fn envelope_round_trip_all_kinds() {
        round_trip(FrameKind::Data, 42, b"opaque payload");
        round_trip(FrameKind::Ack, 0, &encode_ack(7, &[0xAB; 32]));
        round_trip(FrameKind::Auth, 0, b"scheme\x00label\x00proof");
        round_trip(FrameKind::AuthOk, 0, b"");
        round_trip(FrameKind::AuthRejected, 0, b"invalid token");
        let identity = ResumeIdentity::generate();
        round_trip(
            FrameKind::ResumeRequest,
            0,
            &encode_resume_request(3, Some(&identity)),
        );
        round_trip(
            FrameKind::ResumeReply,
            0,
            &encode_resume_reply(ResumeStatus::ResumeFrom, 1, &identity),
        );
        round_trip(FrameKind::Close, 0, b"");
    }

    #[test]
    fn auth_payload_round_trip_and_rejects_malformed() {
        let payload = encode_auth("pawork-token", "my-gui", "deadbeef").expect("encode auth");
        let (scheme, label, proof) = decode_auth(&payload).expect("decode auth");
        assert_eq!(scheme, "pawork-token");
        assert_eq!(label, "my-gui");
        assert_eq!(proof, "deadbeef");

        assert!(decode_auth(b"").is_err());
        assert!(decode_auth(b"a\x00b").is_err());
        assert!(decode_auth(b"a\x00b\x00c\x00d").is_err());
        assert!(decode_auth(b"\x00\x00").is_err());
        assert!(decode_auth(b"a\x00b\x00").is_err());
        assert!(decode_auth(b"a\x00b\x00\xff").is_err());
        assert!(encode_auth("a", &"x".repeat(300), "b").is_err());
    }

    #[test]
    fn resume_reply_round_trip_all_statuses() {
        let identity = ResumeIdentity::generate();
        for status in [
            ResumeStatus::ResumeFrom,
            ResumeStatus::UpToDate,
            ResumeStatus::SnapshotRequired,
        ] {
            let payload = encode_resume_reply(status, 7, &identity);
            let (decoded, next_seq, decoded_identity) =
                decode_resume_reply(&payload).expect("decode");
            assert_eq!(decoded, status);
            assert_eq!(next_seq, 7);
            assert_eq!(decoded_identity, identity);
        }
        assert!(decode_resume_reply(&[0]).is_err());
        assert!(decode_resume_reply(&[9, 0, 0, 0, 0, 0, 0, 0, 0]).is_err());
        assert!(decode_resume_reply(&[0, 0, 0, 0]).is_err());
    }

    #[test]
    fn resume_request_round_trip_and_rejects_malformed_identity() {
        let identity = ResumeIdentity::generate();
        let payload = encode_resume_request(9, Some(&identity));
        let (acked, decoded) = decode_resume_request(&payload).expect("decode");
        assert_eq!(acked, 9);
        assert_eq!(decoded, Some(identity));
        assert_eq!(
            decode_resume_request(&encode_resume_request(0, None)).expect("fresh"),
            (0, None)
        );
        assert!(decode_resume_request(&[0; 8]).is_err());
        assert!(decode_resume_request(&[0; 10]).is_err());
    }

    #[test]
    fn ack_payload_round_trip_and_rejects_malformed() {
        let digest = [0x11u8; 32];
        let payload = encode_ack(42, &digest);
        assert_eq!(payload.len(), ACK_PAYLOAD_BYTES);
        let (seq, decoded) = decode_ack(&payload).expect("decode ack");
        assert_eq!(seq, 42);
        assert_eq!(decoded, digest);

        assert!(decode_ack(&[]).is_err());
        assert!(decode_ack(&payload[..ACK_PAYLOAD_BYTES - 1]).is_err());
        let mut wrong_len = payload.to_vec();
        wrong_len.push(0);
        assert!(decode_ack(&wrong_len).is_err());
    }

    #[test]
    fn bad_magic_version_and_kind_are_rejected() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        runtime.block_on(async {
            let mut bad_magic = vec![0u8; HEADER_BYTES];
            bad_magic[0] = 0x41;
            let error = read_envelope(&mut bad_magic.as_slice(), 1024)
                .await
                .expect_err("bad magic");
            assert_eq!(error.kind, TransportErrorKind::ProtocolViolation);

            let mut bad_version = vec![0u8; HEADER_BYTES];
            bad_version[0..2].copy_from_slice(&MAGIC.to_le_bytes());
            bad_version[2] = 99;
            let error = read_envelope(&mut bad_version.as_slice(), 1024)
                .await
                .expect_err("bad version");
            assert_eq!(error.kind, TransportErrorKind::ProtocolViolation);

            let mut bad_kind = vec![0u8; HEADER_BYTES];
            bad_kind[0..2].copy_from_slice(&MAGIC.to_le_bytes());
            bad_kind[2] = VERSION;
            bad_kind[3] = 77;
            let error = read_envelope(&mut bad_kind.as_slice(), 1024)
                .await
                .expect_err("bad kind");
            assert_eq!(error.kind, TransportErrorKind::ProtocolViolation);
        });
    }

    #[test]
    fn declared_length_over_limit_is_rejected_before_allocation() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        runtime.block_on(async {
            let mut header = vec![0u8; HEADER_BYTES];
            header[0..2].copy_from_slice(&MAGIC.to_le_bytes());
            header[2] = VERSION;
            header[3] = FrameKind::Data.as_byte();
            header[12..16].copy_from_slice(&(4096u32).to_le_bytes());
            let error = read_envelope(&mut header.as_slice(), 1024)
                .await
                .expect_err("oversized");
            assert_eq!(error.kind, TransportErrorKind::FrameTooLarge);
        });
    }

    #[test]
    fn truncated_header_reports_closed() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        runtime.block_on(async {
            let error = read_envelope(&mut (&[0u8, 1][..]), 1024)
                .await
                .expect_err("truncated");
            assert_eq!(error.kind, TransportErrorKind::ConnectionClosed);
        });
    }
}
