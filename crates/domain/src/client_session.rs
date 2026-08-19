//! Client session registry 词汇：记录类型与 store trait。
//!
//! S13-F15 / ADR-037：从 `pawork-protocol::adapter` 下沉，供 session 实现
//! SQLite store、protocol 保留 `SessionRegistry` 时不再形成 session→protocol
//! 反向依赖。本模块不引入 tokio / SQLite / 具体协议编解码。

use std::collections::BTreeSet;
use std::error::Error;
use std::fmt;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::{ConnectionId, SessionId, Timestamp};

/// Client adapter 记录 schema。与 protocol 线框版本对齐，值为 1。
pub const CLIENT_ADAPTER_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ClientSessionId(pub String);

impl ClientSessionId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ClientProtocol(pub String);

impl ClientProtocol {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ClientCapability(pub String);

impl ClientCapability {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilitySnapshot {
    pub schema_version: u32,
    pub protocol: ClientProtocol,
    pub protocol_version: String,
    pub client_version: String,
    pub revision: u64,
    #[serde(default)]
    pub capabilities: BTreeSet<ClientCapability>,
}

impl CapabilitySnapshot {
    pub fn supports(&self, capability: &ClientCapability) -> bool {
        self.capabilities.contains(capability)
    }

    pub fn validate(&self) -> Result<(), SessionRegistryError> {
        if self.schema_version != CLIENT_ADAPTER_SCHEMA_VERSION {
            return Err(SessionRegistryError::UnsupportedSchema {
                found: self.schema_version,
                supported: CLIENT_ADAPTER_SCHEMA_VERSION,
            });
        }
        if self.protocol.0.trim().is_empty()
            || self.protocol_version.trim().is_empty()
            || self.client_version.trim().is_empty()
        {
            return Err(SessionRegistryError::InvalidRecord(
                "protocol and version fields must be non-empty".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClientSessionState {
    Loaded,
    Subscribed,
    Executing,
    Disconnected,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientSessionRecord {
    pub schema_version: u32,
    pub protocol: ClientProtocol,
    pub client_session_id: ClientSessionId,
    pub core_session_id: SessionId,
    pub connection_id: ConnectionId,
    pub ownership_epoch: u64,
    pub revision: u64,
    pub state: ClientSessionState,
    pub capabilities: CapabilitySnapshot,
    pub updated_at: Timestamp,
}

/// Store 负责原子 ownership compare-and-swap；冲突时返回最新权威记录供重同步。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RegistryWriteOutcome {
    Applied,
    Conflict(Box<Option<ClientSessionRecord>>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionRegistryError {
    Unavailable(String),
    InvalidRecord(String),
    UnsupportedSchema { found: u32, supported: u32 },
}

impl fmt::Display for SessionRegistryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable(message) => write!(f, "session registry unavailable: {message}"),
            Self::InvalidRecord(message) => write!(f, "invalid session registry record: {message}"),
            Self::UnsupportedSchema { found, supported } => write!(
                f,
                "adapter schema {found} is unsupported (expected {supported})"
            ),
        }
    }
}

impl Error for SessionRegistryError {}

#[async_trait]
pub trait SessionRegistryStore: Send + Sync {
    async fn load_all(&self) -> Result<Vec<ClientSessionRecord>, SessionRegistryError>;
    async fn insert(
        &self,
        record: &ClientSessionRecord,
    ) -> Result<RegistryWriteOutcome, SessionRegistryError>;
    async fn compare_and_swap(
        &self,
        expected_epoch: u64,
        expected_revision: u64,
        record: &ClientSessionRecord,
    ) -> Result<RegistryWriteOutcome, SessionRegistryError>;
    async fn remove_if_owner(
        &self,
        client_session_id: &ClientSessionId,
        expected_epoch: u64,
        expected_revision: u64,
    ) -> Result<RegistryWriteOutcome, SessionRegistryError>;
}
