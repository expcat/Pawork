//! Transport-independent MCP peer contract used by capability adapters.

use agent_domain::CancellationToken;
use async_trait::async_trait;
use rmcp::model::{
    CallToolRequestParams, CallToolResult, GetPromptRequestParams, GetPromptResult, Prompt,
    ReadResourceRequestParams, ReadResourceResult, Resource, ResourceTemplate, Tool,
};

use crate::McpError;

/// Capability families advertised during the MCP initialize handshake.
///
/// The permissive default keeps lightweight test peers and custom host adapters
/// source-compatible; production peers override this with the server's actual
/// initialize result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct McpServerCapabilities {
    pub tools: bool,
    pub resources: bool,
    pub prompts: bool,
}

impl Default for McpServerCapabilities {
    fn default() -> Self {
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
    async fn list_resources(&self) -> Result<Vec<Resource>, McpError>;
    async fn list_resource_templates(&self) -> Result<Vec<ResourceTemplate>, McpError>;
    async fn list_prompts(&self) -> Result<Vec<Prompt>, McpError>;
    async fn read_resource(
        &self,
        params: ReadResourceRequestParams,
    ) -> Result<ReadResourceResult, McpError>;
    async fn get_prompt(&self, params: GetPromptRequestParams)
        -> Result<GetPromptResult, McpError>;
    async fn call_tool(
        &self,
        params: CallToolRequestParams,
        cancel: CancellationToken,
    ) -> Result<CallToolResult, McpError>;
}
