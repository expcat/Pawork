//! GUI Transport 的业务无关抽象。
//!
//! Transport 只搬运有界字节帧。GUI Connection Protocol 的编解码位于
//! `pawork-protocol`，因此 Local/Remote Adapter 不依赖任何 Agent 领域类型。

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransportFrame {
    bytes: Vec<u8>,
}

impl TransportFrame {
    pub fn new(bytes: Vec<u8>) -> Self {
        Self { bytes }
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TransportEndpoint {
    Local { address: String },
    Remote { address: String, adapter: String },
    Memory { channel: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectOptions {
    pub timeout_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_label: Option<String>,
    pub max_frame_bytes: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectionInfo {
    pub connection_id: String,
    pub locality: ConnectionLocality,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peer_label: Option<String>,
    pub encrypted: bool,
    pub max_frame_bytes: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionLocality {
    Local,
    Remote,
    InProcess,
}

#[async_trait]
pub trait GuiTransportServer: Send + Sync {
    async fn bind(
        &self,
        endpoint: TransportEndpoint,
    ) -> Result<Box<dyn GuiListener>, TransportError>;
}

#[async_trait]
pub trait GuiListener: Send + Sync {
    async fn accept(&self) -> Result<Box<dyn GuiConnection>, TransportError>;
    async fn close(&self) -> Result<(), TransportError>;
}

#[async_trait]
pub trait GuiConnection: Send + Sync {
    async fn send(&self, frame: TransportFrame) -> Result<(), TransportError>;
    async fn receive(&self) -> Result<TransportFrame, TransportError>;
    async fn close(&self) -> Result<(), TransportError>;
    fn info(&self) -> ConnectionInfo;
}

#[async_trait]
pub trait GuiTransportClient: Send + Sync {
    async fn connect(
        &self,
        endpoint: TransportEndpoint,
        options: ConnectOptions,
    ) -> Result<Box<dyn GuiConnection>, TransportError>;
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransportErrorKind {
    InvalidEndpoint,
    BindFailed,
    ConnectionFailed,
    ConnectionClosed,
    Timeout,
    FrameTooLarge,
    ProtocolViolation,
    AuthenticationFailed,
    Unsupported,
    Internal,
}

#[derive(Clone, Debug, Error, PartialEq, Eq, Serialize, Deserialize)]
#[error("{kind:?}: {message}")]
pub struct TransportError {
    pub kind: TransportErrorKind,
    pub message: String,
    #[serde(default)]
    pub retryable: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_round_trip_does_not_require_protocol_types() {
        let endpoint = TransportEndpoint::Remote {
            address: "opaque://peer".into(),
            adapter: "mock".into(),
        };
        let encoded = serde_json::to_string(&endpoint).expect("serialize endpoint");
        let decoded: TransportEndpoint =
            serde_json::from_str(&encoded).expect("deserialize endpoint");

        assert_eq!(decoded, endpoint);
    }

    #[test]
    fn frame_owns_only_bytes() {
        let frame = TransportFrame::new(vec![1, 2, 3]);
        assert_eq!(frame.as_bytes(), &[1, 2, 3]);
        assert_eq!(frame.into_bytes(), vec![1, 2, 3]);
    }
}
