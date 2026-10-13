//! Pawork Model Context Protocol client.
//!
//! The MCP SDK stays inside this crate. Agent Core consumes canonical
//! [`pawork_domain`] tools and transport-independent capability snapshots.
//!
//! B13（2026-10-08）：未接入 Host 的 MCP OAuth 模块已归档删除。
//! 当前布局的完整源码在 `3c235c91a07ad348dd68c4dc18c01f4290bb9b2f` 的
//! `crates/mcp/src/oauth.rs`。tag `v2-final` 只有旧路径
//! `extensions/mcp/src/oauth.rs`，blob 不同，不作为当前布局恢复点。
//! 不要把该源码拷回主干当作第二套认证装配。

use std::time::Duration;

use async_trait::async_trait;
use pawork_domain::CancellationToken;
use serde_json::{Map, Value};

pub mod capabilities;
mod codec;
pub mod config;
pub mod manager;
pub mod sandbox;
pub mod security;
mod transport;

pub use sandbox::{SandboxedStdioSpawner, SpawnedStdio, StdioSpawner};

/// Errors exposed by the Pawork MCP boundary. Error text must remain safe for logs and persistence.
#[derive(Debug, thiserror::Error)]
pub enum McpError {
    #[error("invalid MCP configuration: {0}")]
    Config(String),
    #[error("MCP transport failed: {0}")]
    Transport(String),
    #[error("MCP protocol failed: {0}")]
    Protocol(String),
    #[error("MCP server is disconnected: {0}")]
    Disconnected(String),
    #[error("MCP operation timed out after {0:?}")]
    Timeout(Duration),
    #[error("MCP operation was cancelled")]
    Cancelled,
    #[error("MCP permission denied: {0}")]
    PermissionDenied(String),
    #[error("MCP secret could not be resolved: {0}")]
    Secret(String),
    #[error("MCP tool registration rejected by registry: {0}")]
    Registry(#[from] pawork_tools::ToolRegistryError),
}

/// Tools capability advertised during the MCP initialize handshake.
///
/// The permissive default exists purely for test peers and custom host adapters
/// in this crate; production peers override it with the server's actual
/// initialize result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct McpServerCapabilities {
    pub tools: bool,
}

impl Default for McpServerCapabilities {
    fn default() -> Self {
        // tools: true is a test-peer convenience, not a production claim.
        Self { tools: true }
    }
}

/// Discovered MCP tool metadata. The SDK model stays behind [`codec`].
#[derive(Clone, Debug, PartialEq)]
pub struct McpToolInfo {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
    pub read_only: bool,
}

/// Canonical tool invocation sent to an MCP peer.
#[derive(Clone, Debug, PartialEq)]
pub struct McpToolCall {
    pub name: String,
    pub arguments: Map<String, Value>,
}

/// A connected MCP peer. The SDK model stays behind this crate boundary so SDK upgrades
/// do not leak into Agent Core or provider code.
#[async_trait]
pub trait McpPeer: Send + Sync {
    async fn server_capabilities(&self) -> Result<McpServerCapabilities, McpError> {
        Ok(McpServerCapabilities::default())
    }

    async fn list_tools(&self) -> Result<Vec<McpToolInfo>, McpError>;

    async fn call_tool(
        &self,
        call: McpToolCall,
        cancel: CancellationToken,
    ) -> Result<pawork_domain::ToolResult, McpError>;
}

#[cfg(test)]
mod tests;
