//! Pawork Model Context Protocol client.
//!
//! The MCP SDK stays inside this crate. Agent Core consumes canonical
//! [`pawork_domain`] tools and transport-independent capability snapshots.

use std::time::Duration;

use async_trait::async_trait;
use pawork_domain::CancellationToken;
use serde_json::{Map, Value};

pub mod capabilities;
mod codec;
pub mod config;
pub mod manager;
pub mod oauth;
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
    #[error("MCP OAuth failed: {0}")]
    OAuth(String),
    #[error("MCP tool registration rejected by registry: {0}")]
    Registry(#[from] pawork_tools::ToolRegistryError),
}

impl McpError {
    /// Build a secret-safe error from an authentication failure.
    pub(crate) fn from_auth(error: pawork_auth::AuthError) -> Self {
        Self::OAuth(error.to_string())
    }
}

/// Capability families advertised during the MCP initialize handshake.
///
/// The permissive all-true default exists purely for test peers and custom
/// host adapters in this crate; production peers override it with the server's
/// actual initialize result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct McpServerCapabilities {
    pub tools: bool,
    pub resources: bool,
    pub prompts: bool,
}

impl Default for McpServerCapabilities {
    fn default() -> Self {
        // All-true is a test-peer convenience, not a production claim.
        Self {
            tools: true,
            resources: true,
            prompts: true,
        }
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
