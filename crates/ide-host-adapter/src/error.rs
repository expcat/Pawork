//! IDE Host Adapter 统一错误。

use client_adapter_api::{AdapterError, AdapterErrorFrame, ClientCapability, ClientSessionId};
use thiserror::Error;

/// IDE Host Adapter 错误。
///
/// 协议/翻译层错误显式分类；SDK 通道与 adapter registry 错误原样上抛，
/// 不做业务猜测。
#[derive(Debug, Error)]
pub enum IdeAdapterError {
    #[error("protocol method is unsupported: {0}")]
    ProtocolUnsupported(String),

    #[error("client capability is unsupported: {0:?}")]
    CapabilityUnsupported(ClientCapability),

    #[error("invalid ide frame: {0}")]
    InvalidFrame(String),

    #[error("unknown client session: {0:?}")]
    UnknownSession(ClientSessionId),

    #[error("adapter host unavailable: {0}")]
    HostUnavailable(String),

    #[error("not connected to pawork host")]
    NotConnected,

    #[error("connection lost: {0}")]
    ConnectionLost(String),

    #[error("event bus closed (no consumer)")]
    EventBusClosed,

    #[error("lsp result provider failed: {0}")]
    LspProvider(String),

    #[error("sdk error: {0}")]
    Sdk(#[from] agent_sdk::SdkError),

    #[error("client adapter error: {0}")]
    Adapter(#[from] AdapterError),
}

impl IdeAdapterError {
    /// 翻译为 adapter error frame（供扩展契约 `error` 事件/帧使用）。
    pub fn frame(&self) -> AdapterErrorFrame {
        match self {
            Self::ProtocolUnsupported(method) => AdapterErrorFrame {
                code: "protocol_unsupported".into(),
                message: format!("protocol method is unsupported: {method}"),
                capability: None,
            },
            Self::CapabilityUnsupported(capability) => AdapterErrorFrame {
                code: "capability_unsupported".into(),
                message: format!("client capability is unsupported: {capability:?}"),
                capability: Some(capability.clone()),
            },
            Self::InvalidFrame(message) => AdapterErrorFrame {
                code: "invalid_frame".into(),
                message: format!("invalid ide frame: {message}"),
                capability: None,
            },
            Self::UnknownSession(id) => AdapterErrorFrame {
                code: "unknown_session".into(),
                message: format!("unknown client session: {id:?}"),
                capability: None,
            },
            Self::HostUnavailable(message) => AdapterErrorFrame {
                code: "host_unavailable".into(),
                message: message.clone(),
                capability: None,
            },
            Self::NotConnected => AdapterErrorFrame {
                code: "host_unavailable".into(),
                message: "not connected to pawork host".into(),
                capability: None,
            },
            Self::ConnectionLost(message) => AdapterErrorFrame {
                code: "connection_lost".into(),
                message: message.clone(),
                capability: None,
            },
            Self::EventBusClosed => AdapterErrorFrame {
                code: "event_bus_closed".into(),
                message: "event bus closed (no consumer)".into(),
                capability: None,
            },
            Self::LspProvider(message) => AdapterErrorFrame {
                code: "lsp_provider".into(),
                message: format!("lsp result provider failed: {message}"),
                capability: None,
            },
            Self::Sdk(error) => AdapterErrorFrame {
                code: "sdk_error".into(),
                message: format!("sdk error: {error}"),
                capability: None,
            },
            Self::Adapter(error) => error.frame(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_are_explicit() {
        let error = IdeAdapterError::ProtocolUnsupported("ide.nope".into());
        assert_eq!(error.frame().code, "protocol_unsupported");

        let capability = ClientCapability::new("lifecycle");
        let error = IdeAdapterError::CapabilityUnsupported(capability.clone());
        assert_eq!(error.frame().code, "capability_unsupported");
        assert_eq!(error.frame().capability, Some(capability));

        let error =
            IdeAdapterError::Adapter(AdapterError::UnknownSession(ClientSessionId::new("s")));
        assert_eq!(error.frame().code, "unknown_session");
    }
}
