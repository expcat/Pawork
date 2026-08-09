//! MCP client errors. Error text must remain safe for logs and persistence.

use std::time::Duration;

/// Errors exposed by the Pawork MCP boundary.
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
