//! Pawork Model Context Protocol client.
//!
//! The MCP SDK stays inside this crate. Agent Core consumes canonical
//! [`pawork_domain`] tools and transport-independent capability snapshots.

use std::time::Duration;

use pawork_domain::CancellationToken;
use async_trait::async_trait;
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
    Registry(#[from] crate::ToolRegistryError),
}

impl McpError {
    /// Build a secret-safe error from an authentication failure.
    pub fn from_auth(error: pawork_auth::AuthError) -> Self {
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
mod tests {
    use std::fs;
    use std::path::Path;
    use std::sync::Arc;

    use async_trait::async_trait;
    use pawork_domain::{
        AgentTool, ToolError, ToolEventSink, ToolExecutionContext, ToolRequest, ToolResult,
    };
    use pawork_domain::{
        CancellationToken, ToolCapability, ToolDescriptor, ToolHosting, ToolKind,
    };
    use crate::ToolRegistry;
    use serde_json::json;

    use crate::mcp::capabilities::{register_server_tools, McpToolAdapter};
    use crate::mcp::config::McpPermissions;
    use crate::mcp::{McpError, McpPeer, McpServerCapabilities, McpToolCall, McpToolInfo};

    struct BuiltinMock;

    #[async_trait]
    impl AgentTool for BuiltinMock {
        fn descriptor(&self) -> ToolDescriptor {
            ToolDescriptor {
                name: "builtin_echo".into(),
                description: "built-in mock".into(),
                input_schema: json!({"type": "object"}),
                capability: ToolCapability::ReadOnly,
                kind: ToolKind::ClientFunction,
                hosting: ToolHosting::Local,
                capabilities: Vec::new(),
                requires_approval: false,
                read_only: true,
                supports_concurrency: true,
                default_timeout_ms: None,
                max_output_bytes: 1024,
                allowed_in_untrusted_workspace: true,
            }
        }

        async fn execute(
            &self,
            _request: ToolRequest,
            _context: ToolExecutionContext,
            _sink: &dyn ToolEventSink,
            _cancel: CancellationToken,
        ) -> Result<ToolResult, ToolError> {
            Ok(ToolResult::success(Vec::new()))
        }
    }

    struct RegistryPeer;

    #[async_trait]
    impl McpPeer for RegistryPeer {
        async fn server_capabilities(&self) -> Result<McpServerCapabilities, McpError> {
            Ok(McpServerCapabilities {
                tools: true,
                resources: false,
                prompts: false,
            })
        }

        async fn list_tools(&self) -> Result<Vec<McpToolInfo>, McpError> {
            Ok(vec![McpToolInfo {
                name: "search".into(),
                description: "mcp search".into(),
                input_schema: json!({"type": "object"}),
                read_only: true,
            }])
        }

        async fn call_tool(
            &self,
            _call: McpToolCall,
            _cancel: CancellationToken,
        ) -> Result<ToolResult, McpError> {
            Ok(ToolResult::success(Vec::new()))
        }
    }

    #[test]
    fn public_sources_do_not_mention_rmcp() {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/mcp");
        let mut scanned = 0usize;
        for entry in fs::read_dir(&src).expect("src dir") {
            let entry = entry.expect("entry");
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("rs") {
                continue;
            }
            if path.file_name().and_then(|name| name.to_str()) == Some("codec.rs") {
                continue;
            }
            let contents = fs::read_to_string(&path).expect("read source");
            let sdk = "rmcp";
            let path_ref = format!("{sdk}::");
            let reexport = format!("pub use {sdk}");
            assert!(
                !contents.contains(&path_ref),
                "{} must not mention the MCP SDK path",
                path.display()
            );
            assert!(
                !contents.contains(&reexport),
                "{} must not re-export the MCP SDK",
                path.display()
            );
            scanned += 1;
        }
        assert!(scanned >= 7, "expected to scan public source files");
    }

    #[tokio::test]
    async fn builtin_and_mcp_tools_share_one_registry() {
        let mut registry = ToolRegistry::new();
        registry
            .register(Arc::new(BuiltinMock))
            .expect("register builtin");

        let descriptors = register_server_tools(
            &mut registry,
            "github",
            Arc::new(RegistryPeer),
            McpPermissions::default(),
            false,
            false,
        )
        .await
        .expect("register mcp");

        assert_eq!(descriptors[0].name, "github.search");
        assert!(registry.get("builtin_echo").is_some());
        assert!(registry.get("github.search").is_some());
        let _ = McpToolAdapter::namespaced_name;
    }
}
