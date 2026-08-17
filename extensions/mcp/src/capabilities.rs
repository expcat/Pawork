//! MCP capability bridge.
//!
//! Discovers tools from a connected [`McpPeer`], adapts each MCP tool into a
//! canonical [`AgentTool`] registered under a server namespace (`{server}.{tool}`),
//! and gates invocation with workspace/tool allowlists, non-object input
//! rejection, and output / structuredContent budgets. Runtime approval is left
//! to ToolScheduler.

use std::sync::Arc;

use pawork_api::{
    AgentTool, ToolError, ToolErrorKind, ToolEventSink, ToolExecutionContext, ToolRequest,
    ToolResult,
};
use pawork_domain::{
    CancellationToken, ErrorCategory, ErrorContext, ToolCapability, ToolDescriptor, ToolHosting,
    ToolKind,
};
use async_trait::async_trait;
use serde_json::Value;
use pawork_tools::ToolRegistry;

use crate::codec::apply_tool_result_budget;
use crate::config::McpPermissions;
use crate::{McpError, McpPeer, McpToolCall, McpToolInfo};

/// Build a namespaced tool name: `{server}.{tool}`.
pub fn namespaced_name(server: &str, tool: &str) -> String {
    format!("{server}.{tool}")
}

/// Snapshot of tools advertised by a peer.
#[derive(Clone, Debug, Default)]
pub struct McpCapabilities {
    pub tools: Vec<McpToolInfo>,
}

impl McpCapabilities {
    /// Discover tools from `peer`. Resources / prompts stay internal to the codec.
    pub async fn discover(peer: &dyn McpPeer) -> Result<Self, McpError> {
        let advertised = peer.server_capabilities().await?;
        let tools = if advertised.tools {
            peer.list_tools().await?
        } else {
            Vec::new()
        };
        Ok(Self { tools })
    }
}

/// An [`AgentTool`] backed by a single MCP tool on a single server.
pub struct McpToolAdapter {
    server: String,
    tool: String,
    namespaced: String,
    description: String,
    input_schema: Value,
    capability: ToolCapability,
    read_only: bool,
    peer: Arc<dyn McpPeer>,
    permissions: McpPermissions,
    trusted: bool,
}

impl McpToolAdapter {
    /// Construct an adapter for one discovered tool.
    pub fn new(
        server: impl Into<String>,
        tool: &McpToolInfo,
        peer: Arc<dyn McpPeer>,
        permissions: McpPermissions,
        trusted: bool,
    ) -> Self {
        let server = server.into();
        let namespaced = namespaced_name(&server, &tool.name);
        let (capability, read_only) = classify_tool(tool);
        Self {
            server,
            tool: tool.name.clone(),
            namespaced,
            description: tool.description.clone(),
            input_schema: tool.input_schema.clone(),
            capability,
            read_only,
            peer,
            permissions,
            trusted,
        }
    }

    pub fn namespaced_name(&self) -> &str {
        &self.namespaced
    }
}

fn classify_tool(tool: &McpToolInfo) -> (ToolCapability, bool) {
    if tool.read_only {
        (ToolCapability::ReadOnly, true)
    } else {
        (ToolCapability::ExternalPlugin, false)
    }
}

#[async_trait]
impl AgentTool for McpToolAdapter {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: self.namespaced.clone(),
            description: if self.description.is_empty() {
                format!("MCP tool {}", self.namespaced)
            } else {
                self.description.clone()
            },
            input_schema: self.input_schema.clone(),
            capability: self.capability.clone(),
            kind: ToolKind::ClientFunction,
            hosting: ToolHosting::Local,
            capabilities: Vec::new(),
            requires_approval: !self.read_only,
            read_only: self.read_only,
            supports_concurrency: self.capability.permits_concurrent_execution(),
            default_timeout_ms: None,
            max_output_bytes: self.permissions.max_output_bytes,
            allowed_in_untrusted_workspace: self.read_only || self.trusted,
        }
    }

    async fn execute(
        &self,
        request: ToolRequest,
        context: ToolExecutionContext,
        _sink: &dyn ToolEventSink,
        cancel: CancellationToken,
    ) -> Result<ToolResult, ToolError> {
        if !self.permissions.allowed_workspaces.is_empty()
            && !self
                .permissions
                .allowed_workspaces
                .contains(context.workspace_id.as_str())
        {
            return Ok(denied_result(format!(
                "workspace '{}' is not permitted for MCP server '{}'",
                context.workspace_id, self.server
            )));
        }

        if !self.permissions.allowed_tools.is_empty()
            && !self.permissions.allowed_tools.contains(self.tool.as_str())
        {
            return Ok(denied_result(format!(
                "tool '{}' is not on the allowlist for MCP server '{}'",
                self.tool, self.server
            )));
        }

        if cancel.is_cancelled() {
            return Err(ToolError::cancelled("MCP tool cancelled before invocation"));
        }

        let arguments = arguments_from_input(&request.input)?;

        if cancel.is_cancelled() {
            return Err(ToolError::cancelled(
                "MCP tool cancelled before remote call",
            ));
        }

        let result = match self
            .peer
            .call_tool(
                McpToolCall {
                    name: self.tool.clone(),
                    arguments,
                },
                cancel.clone(),
            )
            .await
        {
            Ok(result) => result,
            Err(McpError::Cancelled) => {
                return Err(ToolError::cancelled("MCP tool call cancelled"));
            }
            Err(McpError::Timeout(duration)) => {
                return Err(ToolError {
                    kind: ToolErrorKind::Timeout,
                    message: format!("MCP tool timed out after {duration:?}"),
                    retryable: true,
                    retry_after_ms: None,
                });
            }
            Err(error) => return Ok(mcp_error_result(&error)),
        };

        Ok(apply_tool_result_budget(
            result,
            self.permissions.max_output_bytes,
        ))
    }
}

/// Register all eligible tools advertised by `peer` into `registry`.
pub async fn register_server_tools(
    registry: &mut ToolRegistry,
    server: &str,
    peer: Arc<dyn McpPeer>,
    permissions: McpPermissions,
    trusted: bool,
) -> Result<Vec<ToolDescriptor>, McpError> {
    let capabilities = McpCapabilities::discover(peer.as_ref()).await?;
    register_discovered_tools(registry, server, &capabilities, peer, permissions, trusted)
}

/// Register discovered tools (synchronous variant).
pub fn register_discovered_tools(
    registry: &mut ToolRegistry,
    server: &str,
    capabilities: &McpCapabilities,
    peer: Arc<dyn McpPeer>,
    permissions: McpPermissions,
    trusted: bool,
) -> Result<Vec<ToolDescriptor>, McpError> {
    let mut descriptors = Vec::new();
    for tool in &capabilities.tools {
        if !permissions.allowed_tools.is_empty() && !permissions.allowed_tools.contains(&tool.name)
        {
            continue;
        }
        let adapter = Arc::new(McpToolAdapter::new(
            server,
            tool,
            peer.clone(),
            permissions.clone(),
            trusted,
        ));
        descriptors.push(adapter.descriptor());
        registry.register(adapter)?;
    }
    Ok(descriptors)
}

fn arguments_from_input(input: &Value) -> Result<serde_json::Map<String, Value>, ToolError> {
    match input {
        Value::Object(map) => Ok(map.clone()),
        _ => Err(ToolError {
            kind: ToolErrorKind::InvalidInput,
            message: "MCP tool input must be a JSON object".into(),
            retryable: false,
            retry_after_ms: None,
        }),
    }
}

fn denied_result(reason: impl Into<String>) -> ToolResult {
    ToolResult::failure(ErrorContext {
        category: ErrorCategory::Authorization,
        message: reason.into(),
        retryable: false,
        retry_after_ms: None,
        diagnostics: Default::default(),
    })
}

fn mcp_error_result(error: &McpError) -> ToolResult {
    ToolResult::failure(ErrorContext {
        category: ErrorCategory::Tool,
        message: error.to_string(),
        retryable: false,
        retry_after_ms: None,
        diagnostics: Default::default(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use pawork_domain::{ContentPart, RunId, WorkspaceId};
    use serde_json::json;
    use std::collections::BTreeSet;
    use pawork_tools::NoopToolEventSink;

    #[derive(Clone)]
    struct MockPeer {
        tool: McpToolInfo,
        response: ToolResult,
        advertised: crate::McpServerCapabilities,
        honor_cancel: bool,
    }

    #[async_trait]
    impl McpPeer for MockPeer {
        async fn server_capabilities(&self) -> Result<crate::McpServerCapabilities, McpError> {
            Ok(self.advertised)
        }

        async fn list_tools(&self) -> Result<Vec<McpToolInfo>, McpError> {
            if !self.advertised.tools {
                return Err(McpError::Protocol("tools are unsupported".into()));
            }
            Ok(vec![self.tool.clone()])
        }

        async fn call_tool(
            &self,
            _call: McpToolCall,
            cancel: CancellationToken,
        ) -> Result<ToolResult, McpError> {
            if self.honor_cancel && cancel.is_cancelled() {
                return Err(McpError::Cancelled);
            }
            Ok(self.response.clone())
        }
    }

    fn make_tool(name: &str, read_only: bool) -> McpToolInfo {
        McpToolInfo {
            name: name.to_owned(),
            description: "mock tool".into(),
            input_schema: json!({"type":"object"}),
            read_only,
        }
    }

    fn text_result(text: &str) -> ToolResult {
        ToolResult::success(vec![ContentPart::Text(pawork_domain::TextContent {
            text: text.to_string(),
        })])
    }

    fn make_peer(read_only: bool, response_text: &str) -> MockPeer {
        MockPeer {
            tool: make_tool("search", read_only),
            response: text_result(response_text),
            advertised: crate::McpServerCapabilities::default(),
            honor_cancel: true,
        }
    }

    fn execution_context(workspace: &str) -> ToolExecutionContext {
        ToolExecutionContext {
            workspace_id: WorkspaceId::from(workspace),
            run_id: RunId::from("run-1"),
            working_directory: None,
        }
    }

    fn request(input: Value) -> ToolRequest {
        ToolRequest {
            tool_call_id: pawork_domain::ToolCallId::from("call-1"),
            input,
        }
    }

    async fn run_adapter(
        adapter: &McpToolAdapter,
        input: Value,
        workspace: &str,
        cancel: CancellationToken,
    ) -> ToolResult {
        adapter
            .execute(
                request(input),
                execution_context(workspace),
                &NoopToolEventSink,
                cancel,
            )
            .await
            .expect("adapter execute")
    }

    #[tokio::test]
    async fn discovery_and_registration_namespace_tools() {
        let peer = Arc::new(make_peer(true, "ok"));
        let mut registry = ToolRegistry::new();
        let descriptors = register_server_tools(
            &mut registry,
            "github",
            peer,
            McpPermissions::default(),
            false,
        )
        .await
        .expect("register");

        assert_eq!(descriptors.len(), 1);
        assert_eq!(descriptors[0].name, "github.search");
        assert_eq!(registry.descriptors()[0].name, "github.search");
        assert_eq!(registry.len(), 1);
    }

    #[tokio::test]
    async fn read_only_tool_passes_and_returns_content() {
        let peer = Arc::new(make_peer(true, "hello world"));
        let adapter = McpToolAdapter::new(
            "github",
            &make_tool("search", true),
            peer,
            McpPermissions::default(),
            false,
        );
        let result = run_adapter(
            &adapter,
            json!({"q":"rust"}),
            "ws",
            CancellationToken::new(),
        )
        .await;
        assert!(result.success);
        assert!(!result.truncated);
        let text = match &result.content[0] {
            ContentPart::Text(t) => t.text.clone(),
            _ => panic!("expected text"),
        };
        assert_eq!(text, "hello world");
    }

    #[tokio::test]
    async fn write_tool_descriptor_marks_approval_and_untrusted_floor() {
        let peer = Arc::new(make_peer(false, "should not matter"));
        let adapter = McpToolAdapter::new(
            "github",
            &make_tool("create_issue", false),
            peer,
            McpPermissions::default(),
            false,
        );
        let descriptor = adapter.descriptor();
        assert!(descriptor.requires_approval);
        assert!(!descriptor.allowed_in_untrusted_workspace);
        assert_eq!(descriptor.capability, ToolCapability::ExternalPlugin);
        assert!(!descriptor.read_only);
    }

    #[tokio::test]
    async fn trusted_write_tool_descriptor_allows_untrusted_workspace_flag() {
        let peer = Arc::new(make_peer(false, "created"));
        let adapter = McpToolAdapter::new(
            "github",
            &make_tool("create_issue", false),
            peer,
            McpPermissions::default(),
            true,
        );
        let descriptor = adapter.descriptor();
        assert!(descriptor.requires_approval);
        assert!(descriptor.allowed_in_untrusted_workspace);
        assert_eq!(descriptor.capability, ToolCapability::ExternalPlugin);
    }

    #[tokio::test]
    async fn cancellation_aborts_before_remote_call() {
        let peer = Arc::new(make_peer(true, "late"));
        let adapter = McpToolAdapter::new(
            "github",
            &make_tool("search", true),
            peer,
            McpPermissions::default(),
            false,
        );
        let cancel = CancellationToken::new();
        cancel.cancel();
        let result = adapter
            .execute(
                request(json!({})),
                execution_context("ws"),
                &NoopToolEventSink,
                cancel,
            )
            .await;
        assert!(
            matches!(result, Err(ref e) if e.kind == ToolErrorKind::Cancelled),
            "expected cancelled, got {result:?}"
        );
    }

    #[tokio::test]
    async fn output_cap_truncates_and_flags() {
        let big = "x".repeat(1024);
        let peer = Arc::new(make_peer(true, &big));
        let permissions = McpPermissions {
            max_output_bytes: 16,
            ..McpPermissions::default()
        };
        let adapter = McpToolAdapter::new(
            "github",
            &make_tool("search", true),
            peer,
            permissions,
            false,
        );
        let result = run_adapter(&adapter, json!({}), "ws", CancellationToken::new()).await;
        assert!(result.truncated, "output must be flagged truncated");
        let text = match &result.content[0] {
            ContentPart::Text(t) => t.text.clone(),
            _ => panic!("expected text"),
        };
        assert_eq!(text.len(), 16);
    }

    #[tokio::test]
    async fn non_object_input_is_rejected_before_remote_call() {
        let peer = Arc::new(make_peer(true, "must not run"));
        let adapter = McpToolAdapter::new(
            "github",
            &make_tool("search", true),
            peer,
            McpPermissions::default(),
            false,
        );
        let error = adapter
            .execute(
                request(json!(["not", "an", "object"])),
                execution_context("ws"),
                &NoopToolEventSink,
                CancellationToken::new(),
            )
            .await
            .expect_err("non-object input must fail");
        assert_eq!(error.kind, ToolErrorKind::InvalidInput);
    }

    #[tokio::test]
    async fn structured_content_is_preserved_within_output_cap() {
        let mut peer = make_peer(true, "unused");
        peer.response = ToolResult {
            content: Vec::new(),
            artifacts: Vec::new(),
            metadata: json!({"mcp": {"structured_content": {"answer": 42}}}),
            truncated: false,
            success: true,
            error: None,
        };
        let adapter = McpToolAdapter::new(
            "github",
            &make_tool("search", true),
            Arc::new(peer),
            McpPermissions {
                max_output_bytes: 256,
                ..McpPermissions::default()
            },
            false,
        );
        let result = run_adapter(&adapter, json!({}), "ws", CancellationToken::new()).await;
        assert_eq!(result.metadata["mcp"]["structured_content"]["answer"], 42);
        assert!(!result.truncated);
    }

    #[tokio::test]
    async fn workspace_allowlist_denies_foreign_workspace() {
        let peer = Arc::new(make_peer(true, "ok"));
        let mut allowed = BTreeSet::new();
        allowed.insert("trusted-ws".to_string());
        let permissions = McpPermissions {
            allowed_workspaces: allowed,
            ..McpPermissions::default()
        };
        let adapter = McpToolAdapter::new(
            "github",
            &make_tool("search", true),
            peer,
            permissions,
            false,
        );
        let denied = run_adapter(&adapter, json!({}), "other-ws", CancellationToken::new()).await;
        assert!(denied.is_error());
        let allowed_result =
            run_adapter(&adapter, json!({}), "trusted-ws", CancellationToken::new()).await;
        assert!(allowed_result.success);
    }

    #[tokio::test]
    async fn tool_allowlist_filters_at_registration_and_execution() {
        let peer = Arc::new(make_peer(true, "ok"));
        let mut allowed = BTreeSet::new();
        allowed.insert("other_tool".to_string());
        let permissions = McpPermissions {
            allowed_tools: allowed,
            ..McpPermissions::default()
        };

        let mut registry = ToolRegistry::new();
        let descriptors = register_discovered_tools(
            &mut registry,
            "github",
            &McpCapabilities {
                tools: vec![make_tool("search", true)],
            },
            peer.clone(),
            permissions.clone(),
            false,
        )
        .expect("registration");
        assert!(descriptors.is_empty());
        assert!(registry.is_empty());

        let adapter = McpToolAdapter::new(
            "github",
            &make_tool("search", true),
            peer,
            permissions,
            false,
        );
        let result = run_adapter(&adapter, json!({}), "ws", CancellationToken::new()).await;
        assert!(result.is_error());
        assert!(result.error.as_ref().unwrap().message.contains("allowlist"));
    }

    #[tokio::test]
    async fn converts_call_tool_error_flag_to_error_result() {
        let error_response = ToolResult {
            content: vec![ContentPart::Text(pawork_domain::TextContent {
                text: "boom".into(),
            })],
            artifacts: Vec::new(),
            metadata: Value::Null,
            truncated: false,
            success: false,
            error: Some(ErrorContext {
                category: ErrorCategory::Tool,
                message: "boom".into(),
                retryable: false,
                retry_after_ms: None,
                diagnostics: Default::default(),
            }),
        };
        let peer = Arc::new(MockPeer {
            tool: make_tool("search", true),
            response: error_response,
            advertised: crate::McpServerCapabilities::default(),
            honor_cancel: false,
        });
        let adapter = McpToolAdapter::new(
            "github",
            &make_tool("search", true),
            peer,
            McpPermissions::default(),
            false,
        );
        let result = run_adapter(&adapter, json!({}), "ws", CancellationToken::new()).await;
        assert!(!result.success);
        assert_eq!(result.error.as_ref().unwrap().message, "boom");
        assert!(!result.content.is_empty());
    }

    #[tokio::test]
    async fn discovery_skips_unadvertised_capability_methods() {
        let mut peer = make_peer(true, "ok");
        peer.advertised = crate::McpServerCapabilities {
            tools: true,
            resources: false,
            prompts: false,
        };
        let caps = McpCapabilities::discover(&peer)
            .await
            .expect("tools-only discovery");
        assert_eq!(caps.tools.len(), 1);
    }
}
