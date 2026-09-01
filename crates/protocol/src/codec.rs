//! 有界 JSON 编解码与 u32 LE 长度前缀分帧。
//!
//! 线上分帧格式为 `[u32 LE payload_len][payload]`。长度前缀在分配缓冲区之前
//! 校验，防止损坏或恶意的帧头声明超大长度；payload 是
//! [`MAX_PROTOCOL_FRAME_BYTES`](crate::MAX_PROTOCOL_FRAME_BYTES) 内的 JSON。
//! Transport（如 `transport-api` 的各实现）只搬运字节，本模块是唯一负责
//! 帧编解码的地方。

use std::io::{Read, Write};

use serde::{de::DeserializeOwned, Serialize};
use thiserror::Error;

use crate::{ClientFrame, ServerFrame, MAX_PROTOCOL_FRAME_BYTES};

/// 长度前缀字节数（u32 little-endian）。
pub const FRAME_LENGTH_PREFIX_BYTES: usize = 4;

/// 把 `ClientFrame` 编码为有界 JSON（不含长度前缀）。
pub fn encode_client_frame(frame: &ClientFrame) -> Result<Vec<u8>, ProtocolCodecError> {
    encode_bounded(frame)
}

/// 解码有界 JSON 为 `ClientFrame`。
pub fn decode_client_frame(bytes: &[u8]) -> Result<ClientFrame, ProtocolCodecError> {
    decode_bounded(bytes)
}

/// 把 `ServerFrame` 编码为有界 JSON（不含长度前缀），并校验重负载变体
/// （ArtifactChunk / Snapshot）。
pub fn encode_server_frame(frame: &ServerFrame) -> Result<Vec<u8>, ProtocolCodecError> {
    validate_heavy_variant(frame)?;
    encode_bounded(frame)
}

/// 解码有界 JSON 为 `ServerFrame`，并校验重负载变体。
pub fn decode_server_frame(bytes: &[u8]) -> Result<ServerFrame, ProtocolCodecError> {
    let frame: ServerFrame = decode_bounded(bytes)?;
    validate_heavy_variant(&frame)?;
    Ok(frame)
}

/// 编码为 `[u32 LE len][payload]` 的完整帧字节。
pub fn encode_length_prefixed<T: Serialize>(value: &T) -> Result<Vec<u8>, ProtocolCodecError> {
    let payload = encode_bounded(value)?;
    let length = u32::try_from(payload.len()).expect("frame payload is capped below u32::MAX");
    let mut framed = Vec::with_capacity(FRAME_LENGTH_PREFIX_BYTES + payload.len());
    framed.extend_from_slice(&length.to_le_bytes());
    framed.extend_from_slice(&payload);
    Ok(framed)
}

/// 解码 `[u32 LE len][payload]` 完整帧字节，校验长度前缀与 payload 一致。
pub fn decode_length_prefixed<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, ProtocolCodecError> {
    if bytes.len() < FRAME_LENGTH_PREFIX_BYTES {
        return Err(ProtocolCodecError::TruncatedFrame);
    }
    let header: [u8; FRAME_LENGTH_PREFIX_BYTES] = bytes[..FRAME_LENGTH_PREFIX_BYTES]
        .try_into()
        .expect("4-byte header");
    let declared = u32::from_le_bytes(header);
    let actual = bytes.len() - FRAME_LENGTH_PREFIX_BYTES;
    if declared as usize != actual {
        return Err(ProtocolCodecError::FrameLengthMismatch { declared, actual });
    }
    ensure_frame_size(declared as usize)?;
    decode_bounded(&bytes[FRAME_LENGTH_PREFIX_BYTES..])
}

/// 向 `writer` 写入 `[u32 LE len][payload]` 帧。
pub fn write_frame<W: Write>(writer: &mut W, payload: &[u8]) -> Result<(), ProtocolCodecError> {
    ensure_frame_size(payload.len())?;
    let length = u32::try_from(payload.len()).expect("frame payload is capped below u32::MAX");
    writer
        .write_all(&length.to_le_bytes())
        .map_err(ProtocolCodecError::Io)?;
    writer.write_all(payload).map_err(ProtocolCodecError::Io)?;
    Ok(())
}

/// 从 `reader` 读入一帧 `[u32 LE len][payload]`，返回 payload 字节。
///
/// 长度前缀声明的长度超过 [`MAX_PROTOCOL_FRAME_BYTES`](crate::MAX_PROTOCOL_FRAME_BYTES)
/// 时在分配前拒绝。
pub fn read_frame<R: Read>(reader: &mut R) -> Result<Vec<u8>, ProtocolCodecError> {
    let mut header = [0u8; FRAME_LENGTH_PREFIX_BYTES];
    reader
        .read_exact(&mut header)
        .map_err(ProtocolCodecError::Io)?;
    let declared = u32::from_le_bytes(header) as usize;
    ensure_frame_size(declared)?;
    let mut payload = vec![0u8; declared];
    reader
        .read_exact(&mut payload)
        .map_err(ProtocolCodecError::Io)?;
    Ok(payload)
}

/// 编码并写入一帧 `ClientFrame`。
pub fn write_client_frame<W: Write>(
    writer: &mut W,
    frame: &ClientFrame,
) -> Result<(), ProtocolCodecError> {
    let payload = encode_client_frame(frame)?;
    write_frame(writer, &payload)
}

/// 读入并解码一帧 `ClientFrame`。
pub fn read_client_frame<R: Read>(reader: &mut R) -> Result<ClientFrame, ProtocolCodecError> {
    let payload = read_frame(reader)?;
    decode_client_frame(&payload)
}

/// 编码并写入一帧 `ServerFrame`。
pub fn write_server_frame<W: Write>(
    writer: &mut W,
    frame: &ServerFrame,
) -> Result<(), ProtocolCodecError> {
    let payload = encode_server_frame(frame)?;
    write_frame(writer, &payload)
}

/// 读入并解码一帧 `ServerFrame`。
pub fn read_server_frame<R: Read>(reader: &mut R) -> Result<ServerFrame, ProtocolCodecError> {
    let payload = read_frame(reader)?;
    decode_server_frame(&payload)
}

fn validate_heavy_variant(frame: &ServerFrame) -> Result<(), ProtocolCodecError> {
    match frame {
        ServerFrame::ArtifactChunk(chunk) => chunk.validate(),
        ServerFrame::Snapshot(snapshot) => snapshot.validate(),
        _ => Ok(()),
    }
}

fn encode_bounded<T: Serialize>(value: &T) -> Result<Vec<u8>, ProtocolCodecError> {
    let bytes = serde_json::to_vec(value).map_err(ProtocolCodecError::InvalidJson)?;
    ensure_frame_size(bytes.len())?;
    Ok(bytes)
}

fn decode_bounded<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, ProtocolCodecError> {
    ensure_frame_size(bytes.len())?;
    serde_json::from_slice(bytes).map_err(ProtocolCodecError::InvalidJson)
}

fn ensure_frame_size(actual: usize) -> Result<(), ProtocolCodecError> {
    if actual > MAX_PROTOCOL_FRAME_BYTES {
        return Err(ProtocolCodecError::FrameTooLarge {
            actual,
            limit: MAX_PROTOCOL_FRAME_BYTES,
        });
    }
    Ok(())
}

/// 帧编解码错误。线上协议错误（[`crate::ProtocolError`]）由
/// [`crate::error`] 的 `From` 转换产生。
#[derive(Debug, Error)]
pub enum ProtocolCodecError {
    #[error("invalid protocol JSON: {0}")]
    InvalidJson(serde_json::Error),
    #[error("protocol frame is too large: {actual} bytes, limit {limit}")]
    FrameTooLarge { actual: usize, limit: usize },
    #[error("artifact chunk is too large: {actual} bytes, limit {limit}")]
    ArtifactChunkTooLarge { actual: usize, limit: usize },
    #[error("snapshot section data is too large: {actual} bytes, limit {limit}")]
    SnapshotSectionDataTooLarge { actual: usize, limit: usize },
    #[error("snapshot section must not set both data and artifact_id")]
    AmbiguousSnapshotSection,
    #[error("snapshot section must set exactly one of data or artifact_id")]
    EmptySnapshotSection,
    #[error("frame is truncated")]
    TruncatedFrame,
    #[error("frame length prefix mismatch: declared {declared}, actual {actual}")]
    FrameLengthMismatch { declared: u32, actual: usize },
    #[error("frame io error: {0}")]
    Io(#[from] std::io::Error),
}

/// 异步写入 `[u32 LE len][payload]`，形态与同步 [`write_frame`] 一致。
pub async fn write_frame_async<W>(writer: &mut W, payload: &[u8]) -> Result<(), ProtocolCodecError>
where
    W: tokio::io::AsyncWrite + Unpin,
{
    use tokio::io::AsyncWriteExt;
    ensure_frame_size(payload.len())?;
    let length = u32::try_from(payload.len()).expect("frame payload is capped below u32::MAX");
    writer
        .write_all(&length.to_le_bytes())
        .await
        .map_err(ProtocolCodecError::Io)?;
    writer
        .write_all(payload)
        .await
        .map_err(ProtocolCodecError::Io)?;
    Ok(())
}

/// 异步读入一帧 `[u32 LE len][payload]`，长度前缀超限时在分配前拒绝。
pub async fn read_frame_async<R>(reader: &mut R) -> Result<Vec<u8>, ProtocolCodecError>
where
    R: tokio::io::AsyncRead + Unpin,
{
    use tokio::io::AsyncReadExt;
    let mut header = [0u8; FRAME_LENGTH_PREFIX_BYTES];
    reader
        .read_exact(&mut header)
        .await
        .map_err(ProtocolCodecError::Io)?;
    let declared = u32::from_le_bytes(header) as usize;
    ensure_frame_size(declared)?;
    let mut payload = vec![0u8; declared];
    reader
        .read_exact(&mut payload)
        .await
        .map_err(ProtocolCodecError::Io)?;
    Ok(payload)
}
