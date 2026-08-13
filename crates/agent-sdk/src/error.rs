//! SDK 结构化错误。

use std::fmt;

use agent_domain::ErrorContext;
use headless_json::ProtocolErrorKind;
use thiserror::Error;

/// 错误类别（供匹配，不携带内部帧）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SdkErrorKind {
    /// 无法启动 `pawork` 进程。
    Spawn,
    /// 进程/stdio IO 失败或连接关闭。
    Io,
    /// 收到的行不是合法 JSONL 帧。
    MalformedFrame,
    /// 收到无法识别的响应帧类型（显式 unknown）。
    UnknownResponseType,
    /// Host 不支持请求的能力（显式 unsupported）。
    UnsupportedCapability,
    /// Host 与 SDK 的协议 major 不兼容。
    IncompatibleApiVersion,
    /// Host 返回业务错误（`AppResponse::Error`）。
    RequestFailed,
    /// 事件通道溢出（背压策略为 `Error` 时产生）。
    Backpressure,
    /// 操作被取消（订阅关闭 / 进程退出）。
    Cancelled,
    /// 操作超时。
    Timeout,
    /// Host 显式 error 帧中的其他类别。
    Protocol(ProtocolErrorKind),
}

impl SdkErrorKind {
    /// 是否值得重试（传输层错误；协议/业务错误不重试）。
    pub fn is_retryable(self) -> bool {
        matches!(self, Self::Spawn | Self::Io | Self::Timeout)
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Spawn => "spawn",
            Self::Io => "io",
            Self::MalformedFrame => "malformed_frame",
            Self::UnknownResponseType => "unknown_response_type",
            Self::UnsupportedCapability => "unsupported_capability",
            Self::IncompatibleApiVersion => "incompatible_api_version",
            Self::RequestFailed => "request_failed",
            Self::Backpressure => "backpressure",
            Self::Cancelled => "cancelled",
            Self::Timeout => "timeout",
            Self::Protocol(kind) => match kind {
                ProtocolErrorKind::UnknownRequestType => "unknown_request_type",
                ProtocolErrorKind::UnsupportedCapability => "unsupported_capability",
                ProtocolErrorKind::IncompatibleApiVersion => "incompatible_api_version",
                ProtocolErrorKind::NotHandshaked => "not_handshaked",
                ProtocolErrorKind::MalformedFrame => "malformed_frame",
                ProtocolErrorKind::TooLarge => "too_large",
                ProtocolErrorKind::CompatRejected => "compat_rejected",
                ProtocolErrorKind::Backpressure => "backpressure",
                ProtocolErrorKind::Internal => "internal",
            },
        }
    }
}

/// SDK 统一错误。
#[derive(Debug, Error)]
pub enum SdkError {
    #[error("failed to spawn pawork: {0}")]
    Spawn(String),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("malformed frame: {0}")]
    MalformedFrame(String),

    #[error("unknown response frame type `{0}`")]
    UnknownResponseType(String),

    #[error("capability `{0}` is not supported by the host")]
    UnsupportedCapability(String),

    #[error("incompatible api version: host={host:?} sdk={sdk:?}")]
    IncompatibleApiVersion {
        host: core_api::ApiVersion,
        sdk: core_api::ApiVersion,
    },

    #[error("request failed: {}", .0.message)]
    RequestFailed(ErrorContext),

    #[error("event channel overflowed: {0}")]
    Backpressure(String),

    #[error("operation cancelled: {0}")]
    Cancelled(String),

    #[error("operation timed out after {0:?}")]
    Timeout(std::time::Duration),

    #[error("host protocol error {kind:?}: {message}")]
    HostProtocol {
        kind: ProtocolErrorKind,
        message: String,
    },

    #[error("connection closed: {0}")]
    Closed(String),
}

impl SdkError {
    pub fn kind(&self) -> SdkErrorKind {
        match self {
            Self::Spawn(_) => SdkErrorKind::Spawn,
            Self::Io(_) => SdkErrorKind::Io,
            Self::MalformedFrame(_) => SdkErrorKind::MalformedFrame,
            Self::UnknownResponseType(_) => SdkErrorKind::UnknownResponseType,
            Self::UnsupportedCapability(_) => SdkErrorKind::UnsupportedCapability,
            Self::IncompatibleApiVersion { .. } => SdkErrorKind::IncompatibleApiVersion,
            Self::RequestFailed(_) => SdkErrorKind::RequestFailed,
            Self::Backpressure(_) => SdkErrorKind::Backpressure,
            Self::Cancelled(_) => SdkErrorKind::Cancelled,
            Self::Timeout(_) => SdkErrorKind::Timeout,
            Self::HostProtocol { kind, .. } => SdkErrorKind::Protocol(*kind),
            Self::Closed(_) => SdkErrorKind::Io,
        }
    }

    /// 从 Host 显式 `error` 帧构造错误。
    pub fn from_error_frame(kind: ProtocolErrorKind, message: String) -> Self {
        Self::HostProtocol { kind, message }
    }

    pub fn timeout(after: std::time::Duration) -> Self {
        Self::Timeout(after)
    }
}

impl fmt::Display for SdkErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}
