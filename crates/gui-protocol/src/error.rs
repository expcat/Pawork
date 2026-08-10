//! 线上结构化错误的构造与 IncompatibleVersion 产生路径。

use crate::{ProtocolCodecError, ProtocolError, ProtocolErrorCode};

impl ProtocolError {
    pub fn incompatible_version(message: impl Into<String>) -> Self {
        Self {
            code: ProtocolErrorCode::IncompatibleVersion,
            message: message.into(),
            retryable: false,
        }
    }

    pub fn authentication_failed(message: impl Into<String>) -> Self {
        Self {
            code: ProtocolErrorCode::AuthenticationFailed,
            message: message.into(),
            retryable: false,
        }
    }

    pub fn invalid_frame(message: impl Into<String>) -> Self {
        Self {
            code: ProtocolErrorCode::InvalidFrame,
            message: message.into(),
            retryable: false,
        }
    }

    pub fn frame_too_large(actual: usize, limit: usize) -> Self {
        Self {
            code: ProtocolErrorCode::FrameTooLarge,
            message: format!("protocol frame is too large: {actual} bytes, limit {limit}"),
            retryable: false,
        }
    }
}

/// 编解码错误 → 线上协议错误（帧类型/帧头损坏按 `InvalidFrame`，超限按
/// `FrameTooLarge`），供解码校验路径直接进入 `ServerFrame::Error`。
impl From<ProtocolCodecError> for ProtocolError {
    fn from(error: ProtocolCodecError) -> Self {
        match error {
            ProtocolCodecError::InvalidJson(_) => {
                Self::invalid_frame("invalid protocol frame JSON")
            }
            ProtocolCodecError::FrameTooLarge { actual, limit } => {
                Self::frame_too_large(actual, limit)
            }
            ProtocolCodecError::ArtifactChunkTooLarge { .. } => {
                Self::invalid_frame("artifact chunk exceeds the size limit")
            }
            ProtocolCodecError::SnapshotSectionDataTooLarge { .. } => {
                Self::invalid_frame("snapshot section data exceeds the size limit")
            }
            ProtocolCodecError::AmbiguousSnapshotSection => {
                Self::invalid_frame("snapshot section must not set both data and artifact_id")
            }
            ProtocolCodecError::EmptySnapshotSection => {
                Self::invalid_frame("snapshot section must set exactly one of data or artifact_id")
            }
            ProtocolCodecError::TruncatedFrame
            | ProtocolCodecError::FrameLengthMismatch { .. }
            | ProtocolCodecError::Io(_) => Self::invalid_frame("malformed or truncated frame"),
        }
    }
}
