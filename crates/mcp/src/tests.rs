use std::fs;
use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use pawork_domain::{
    AgentTool, ToolError, ToolEventSink, ToolExecutionContext, ToolRequest, ToolResult,
};
use pawork_domain::{CancellationToken, ToolCapability, ToolDescriptor, ToolHosting, ToolKind};
use pawork_tools::ToolRegistry;
use serde_json::json;

use crate::capabilities::{register_server_tools, McpToolAdapter};
use crate::config::McpPermissions;
use crate::{McpError, McpPeer, McpServerCapabilities, McpToolCall, McpToolInfo};

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
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut scanned = 0usize;
    for entry in fs::read_dir(&src).expect("src dir") {
        let entry = entry.expect("entry");
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("rs") {
            continue;
        }
        if matches!(
            path.file_name().and_then(|name| name.to_str()),
            Some("codec.rs" | "tests.rs")
        ) {
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

    assert_eq!(descriptors[0].name, "github_search");
    assert!(registry.get("builtin_echo").is_some());
    assert!(registry.get("github_search").is_some());
    let _ = McpToolAdapter::namespaced_name;
}
