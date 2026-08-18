//! GUI Transport 的业务无关抽象。
//!
//! Transport 只搬运有界字节帧。GUI Connection Protocol 的编解码位于
//! `pawork-protocol`，因此 Local/Remote Adapter 不依赖任何 Agent 领域类型。
//! Remote 契约（trait / DTO）仍保留，生产 TLS 实现已归档。

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// 默认单帧上限，与 `pawork-protocol::MAX_PROTOCOL_FRAME_BYTES`（1 MiB）一致，
/// 保证传输层不会截断协议层允许的帧。
pub const DEFAULT_MAX_FRAME_BYTES: u64 = 1024 * 1024;

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

// ---------- 可替换远程 Adapter 契约（单一来源，P17-14 / S10） ----------
//
// 远程连接（内网穿透 / NAT / 中继 / 加密）由可替换 Adapter 实现：CLI 侧经
// RemoteGuiTransportProvider 发布 / 撤销远程端点，GUI 侧经
// RemoteGuiConnector 连接已发布端点。契约集中在本 crate，生产实现
//（feature `remote`）与 Mock / 测试支持（feature `memory`）共用同一
// trait / DTO，避免生产路径依赖 mock。

/// Provider 的描述信息（CLI 输出与日志用）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteTransportDescription {
    /// Adapter 名（如 mock / remote）。
    pub adapter: String,
    /// 人类可读名称。
    pub display_name: String,
}

/// publish 的输入：端点描述。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemotePublishRequest {
    /// 端点名称（用户可读）。
    pub name: String,
}

/// 已发布远程端点的句柄：id 供 unpublish 使用，endpoint 供 GUI Server
/// 绑定与 GUI Connector 连接。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemotePublishHandle {
    pub id: String,
    pub endpoint: TransportEndpoint,
}

/// CLI 侧的远程端点生命周期 Adapter（可替换）。
///
/// 实现只负责端点发布/撤销与描述，不含 Agent 业务逻辑；发布后的实际监听由
/// CLI 把 RemotePublishHandle.endpoint 交给 GUI Server 绑定。
#[async_trait]
pub trait RemoteGuiTransportProvider: Send + Sync {
    /// Adapter 描述信息。
    fn describe(&self) -> RemoteTransportDescription;

    /// 发布远程端点，返回句柄（含端点描述）。
    async fn publish(
        &self,
        request: RemotePublishRequest,
    ) -> Result<RemotePublishHandle, TransportError>;

    /// 撤销已发布端点（按 publish 返回的 handle id）。
    async fn unpublish(&self, handle_id: &str) -> Result<(), TransportError>;

    /// 撤销已发布端点（按 publish 返回的 handle id）：关闭已绑定的监听器、
    /// 销毁端点凭证并使凭证立即失效；实现按各自策略断开已建立连接。撤销后
    /// 对该端点的 connect 必须失败。
    async fn revoke(&self, handle_id: &str) -> Result<(), TransportError>;
}

/// GUI 侧的远程连接 Adapter（可替换）。
///
/// connect 返回的 GuiConnection 与本地 Transport 返回的是同一抽象，
/// GUI 侧协议流程与本地完全一致。
#[async_trait]
pub trait RemoteGuiConnector: Send + Sync {
    async fn connect(
        &self,
        endpoint: &TransportEndpoint,
        options: ConnectOptions,
    ) -> Result<Box<dyn GuiConnection>, TransportError>;
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
