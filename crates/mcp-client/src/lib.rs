//! Pawork Model Context Protocol client.
//!
//! `rmcp` is isolated inside this crate. Agent Core consumes canonical [`tool_api`] tools and
//! transport-independent capability snapshots rather than depending on SDK-specific types.

use std::time::Duration;

use agent_domain::CancellationToken;
use async_trait::async_trait;
use rmcp::model::{
    CallToolRequestParams, CallToolResult, GetPromptRequestParams, GetPromptResult, Prompt,
    ReadResourceRequestParams, ReadResourceResult, Resource, ResourceTemplate, Tool,
};

pub mod capabilities;
pub mod config;
pub mod manager;
pub mod oauth;
pub mod security;
pub mod transport;

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
}

impl McpError {
    /// Build a secret-safe error from an authentication failure.
    pub fn from_auth(error: auth_service::AuthError) -> Self {
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

/// A connected MCP peer. The rmcp model stays behind this crate boundary so SDK upgrades
/// do not leak into Agent Core or provider code.
#[async_trait]
pub trait McpPeer: Send + Sync {
    async fn server_capabilities(&self) -> Result<McpServerCapabilities, McpError> {
        Ok(McpServerCapabilities::default())
    }

    async fn list_tools(&self) -> Result<Vec<Tool>, McpError>;

    /// DEFERRED-CONSUMER: no adapter consumes this yet (only tools are adapted;
    /// resources/prompts land at P15 Canonical Tool v2 / P19 GUI Resources·MCP).
    async fn list_resources(&self) -> Result<Vec<Resource>, McpError>;

    /// DEFERRED-CONSUMER: no adapter consumes this yet (see [`McpPeer::list_resources`]).
    async fn list_resource_templates(&self) -> Result<Vec<ResourceTemplate>, McpError>;

    /// DEFERRED-CONSUMER: no adapter consumes this yet (see [`McpPeer::list_resources`]).
    async fn list_prompts(&self) -> Result<Vec<Prompt>, McpError>;

    /// DEFERRED-CONSUMER: no adapter consumes this yet (see [`McpPeer::list_resources`]).
    async fn read_resource(
        &self,
        params: ReadResourceRequestParams,
    ) -> Result<ReadResourceResult, McpError>;

    /// DEFERRED-CONSUMER: no adapter consumes this yet (see [`McpPeer::list_resources`]).
    async fn get_prompt(&self, params: GetPromptRequestParams)
        -> Result<GetPromptResult, McpError>;

    async fn call_tool(
        &self,
        params: CallToolRequestParams,
        cancel: CancellationToken,
    ) -> Result<CallToolResult, McpError>;
}
