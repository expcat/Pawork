//! MCP capability bridge.
//!
//! Discovers Tools / Resources / Resource Templates / Prompts from a connected
//! [`McpPeer`], adapts each MCP tool into a canonical [`AgentTool`] registered
//! under a server namespace (`{server}.{tool}`), and gates every invocation
//! with:
//! - the shared [`CancellationToken`];
//! - a per-server [`PolicyEngine`] decision;
//! - an auditable [`McpApproval`] channel for `AskUser` outcomes;
//! - tool and workspace allowlists;
//! - a hard byte output cap that marks oversized output as truncated.

use std::collections::BTreeSet;
use std::sync::Arc;

use agent_domain::{
    CancellationToken, ContentPart, ErrorCategory, ErrorContext, ImageContent, ImageSource,
    TextContent,
};
use async_trait::async_trait;
use policy_engine::{ApprovalMode, PolicyDecision, PolicyEngine, PolicyInput};
use rmcp::model::{
    CallToolRequestParams, CallToolResult, ContentBlock, Prompt, Resource, ResourceTemplate, Tool,
};
use serde_json::{json, Value};
use tool_api::{
    AgentTool, ToolCapability, ToolDescriptor, ToolError, ToolErrorKind, ToolEventSink,
    ToolExecutionContext, ToolRequest, ToolResult,
};
use tool_runtime::ToolRegistry;

use crate::config::McpPermissions;
use crate::{McpError, McpPeer};

/// Build a namespaced tool name: `{server}.{tool}`.
///
/// Server names are validated (no `.`) by [`crate::config`] so the namespace
/// boundary is unambiguous.
pub fn namespaced_name(server: &str, tool: &str) -> String {
    format!("{server}.{tool}")
}

/// Snapshot of capabilities advertised by a peer.
#[derive(Clone, Debug, Default)]
pub struct McpCapabilities {
    pub tools: Vec<Tool>,
    pub resources: Vec<Resource>,
    pub resource_templates: Vec<ResourceTemplate>,
    pub prompts: Vec<Prompt>,
}

impl McpCapabilities {
    /// Discover all capability lists from `peer`.
    pub async fn discover(peer: &dyn McpPeer) -> Result<Self, McpError> {
        let advertised = peer.server_capabilities().await?;
        let tools = if advertised.tools {
            peer.list_tools().await?
        } else {
            Vec::new()
        };
        let (resources, resource_templates) = if advertised.resources {
            (
                peer.list_resources().await?,
                peer.list_resource_templates().await?,
            )
        } else {
            (Vec::new(), Vec::new())
        };
        let prompts = if advertised.prompts {
            peer.list_prompts().await?
        } else {
            Vec::new()
        };
        Ok(Self {
            tools,
            resources,
            resource_templates,
            prompts,
        })
    }
}

/// Per-server invocation policy carried by every adapted tool.
#[derive(Clone, Debug)]
pub struct McpInvocationPolicy {
    pub approval_mode: ApprovalMode,
    /// Trust floor for this server. When false the policy engine's untrusted
    /// floor denies every non-read-only capability regardless of approval.
    pub trusted: bool,
    pub allowed_workspaces: BTreeSet<String>,
    pub allowed_tools: BTreeSet<String>,
    pub max_output_bytes: u64,
}

impl Default for McpInvocationPolicy {
    fn default() -> Self {
        Self {
            approval_mode: ApprovalMode::ReadOnly,
            trusted: false,
            allowed_workspaces: BTreeSet::new(),
            allowed_tools: BTreeSet::new(),
            max_output_bytes: 1024 * 1024,
        }
    }
}

impl McpInvocationPolicy {
    /// Build the invocation policy from a server's parsed permissions + trust.
    pub fn from_permissions(permissions: &McpPermissions, trusted: bool) -> Self {
        Self {
            approval_mode: permissions.approval_mode,
            trusted,
            allowed_workspaces: permissions.allowed_workspaces.clone(),
            allowed_tools: permissions.allowed_tools.clone(),
            max_output_bytes: permissions.max_output_bytes,
        }
    }

    /// Build the invocation policy from a parsed server configuration.
    pub fn from_server_config(server: &crate::config::McpServerConfig) -> Self {
        Self::from_permissions(&server.permissions, server.trusted)
    }
}

/// A request for an approval decision, carrying everything an audit record needs.
#[derive(Clone, Debug)]
pub struct McpApprovalRequest {
    pub server: String,
    pub tool: String,
    pub namespaced_name: String,
    pub arguments: Value,
    pub risk: policy_engine::RiskLevel,
    pub workspace_id: String,
}

/// An auditable approval decision.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum McpApprovalDecision {
    Approved,
    Denied { reason: String },
}

/// Auditable per-server approval channel.
///
/// Implementations must persist every decision correlated with the
/// [`McpApprovalRequest`] so grants and denials are reviewable. The host
/// typically wires this to the same audit log used by the tool scheduler.
#[async_trait]
pub trait McpApproval: Send + Sync {
    async fn decide(&self, request: &McpApprovalRequest) -> McpApprovalDecision;
}

/// An [`AgentTool`] backed by a single MCP tool on a single server.
///
/// The descriptor is exposed under the `{server}.{tool}` namespace; the actual
/// remote call uses the server-local tool name.
pub struct McpToolAdapter {
    server: String,
    tool: String,
    namespaced: String,
    description: String,
    input_schema: Value,
    capability: ToolCapability,
    read_only: bool,
    peer: Arc<dyn McpPeer>,
    policy_engine: PolicyEngine,
    invocation: McpInvocationPolicy,
    approval: Arc<dyn McpApproval>,
}

impl McpToolAdapter {
    /// Construct an adapter for one discovered tool.
    pub fn new(
        server: impl Into<String>,
        tool: &Tool,
        peer: Arc<dyn McpPeer>,
        invocation: McpInvocationPolicy,
        approval: Arc<dyn McpApproval>,
    ) -> Self {
        let server = server.into();
        let tool_name = tool.name.to_string();
        let namespaced = namespaced_name(&server, &tool_name);
        let (capability, read_only) = classify_tool(tool);
        Self {
            server,
            tool: tool_name,
            namespaced,
            description: tool
                .description
                .as_deref()
                .map(str::to_string)
                .unwrap_or_default(),
            input_schema: tool.schema_as_json_value(),
            capability,
            read_only,
            peer,
            policy_engine: PolicyEngine::new(invocation.approval_mode),
            invocation,
            approval,
        }
    }

    /// The namespaced (`{server}.{tool}`) descriptor name.
    pub fn namespaced_name(&self) -> &str {
        &self.namespaced
    }
}

/// Map an MCP tool's annotations to a canonical capability + read-only flag.
///
/// `readOnlyHint` tools become [`ToolCapability::ReadOnly`]; everything else is
/// conservatively treated as [`ToolCapability::ExternalPlugin`] (MCP servers are
/// external processes and side-effecting by default).
fn classify_tool(tool: &Tool) -> (ToolCapability, bool) {
    let read_only = tool
        .annotations
        .as_ref()
        .and_then(|annotations| annotations.read_only_hint)
        .unwrap_or(false);
    if read_only {
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
            read_only: self.read_only,
            supports_concurrency: self.capability.permits_concurrent_execution(),
            default_timeout_ms: None,
            max_output_bytes: self.invocation.max_output_bytes,
            allowed_in_untrusted_workspace: self.read_only || self.invocation.trusted,
        }
    }

    async fn execute(
        &self,
        request: ToolRequest,
        context: ToolExecutionContext,
        _sink: &dyn ToolEventSink,
        cancel: CancellationToken,
    ) -> Result<ToolResult, ToolError> {
        // 1. Workspace allowlist.
        if !self.invocation.allowed_workspaces.is_empty()
            && !self
                .invocation
                .allowed_workspaces
                .contains(context.workspace_id.as_str())
        {
            return Ok(denied_result(format!(
                "workspace '{}' is not permitted for MCP server '{}'",
                context.workspace_id, self.server
            )));
        }

        // 2. Tool allowlist.
        if !self.invocation.allowed_tools.is_empty()
            && !self.invocation.allowed_tools.contains(self.tool.as_str())
        {
            return Ok(denied_result(format!(
                "tool '{}' is not on the allowlist for MCP server '{}'",
                self.tool, self.server
            )));
        }

        // 3. Cancellation pre-check.
        if cancel.is_cancelled() {
            return Err(ToolError::cancelled("MCP tool cancelled before invocation"));
        }

        let arguments = arguments_from_input(&request.input)?;

        // 4. Per-server policy decision.
        let decision = self.policy_engine.decide(&PolicyInput {
            capability: self.capability.clone(),
            input: request.input.clone(),
            trusted: self.invocation.trusted,
            allowed_in_untrusted_workspace: self.read_only || self.invocation.trusted,
            approval_mode: self.invocation.approval_mode,
        });
        match decision {
            PolicyDecision::Deny { reason } => return Ok(denied_result(reason)),
            PolicyDecision::AskUser { prompt } => {
                let approval_request = McpApprovalRequest {
                    server: self.server.clone(),
                    tool: self.tool.clone(),
                    namespaced_name: self.namespaced.clone(),
                    arguments: request.input.clone(),
                    risk: prompt.risk,
                    workspace_id: context.workspace_id.to_string(),
                };
                match self.approval.decide(&approval_request).await {
                    McpApprovalDecision::Approved => {}
                    McpApprovalDecision::Denied { reason } => return Ok(denied_result(reason)),
                }
            }
            PolicyDecision::AllowWithConstraints { .. } | PolicyDecision::Allow => {}
        }

        // 5. Cancellation right before the remote call.
        if cancel.is_cancelled() {
            return Err(ToolError::cancelled(
                "MCP tool cancelled before remote call",
            ));
        }

        // 6. Invoke the peer with the server-local tool name.
        let params = CallToolRequestParams::new(self.tool.clone()).with_arguments(arguments);
        let result = match self.peer.call_tool(params, cancel.clone()).await {
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

        // 7. Convert + enforce the byte output cap.
        Ok(convert_call_tool_result(
            &result,
            self.invocation.max_output_bytes,
        ))
    }
}

/// Register all eligible tools advertised by `peer` into `registry` under the
/// `server` namespace, returning their descriptors.
///
/// Discovery is async (lists all four capability kinds); only tools are
/// adapted. Tools outside `invocation.allowed_tools` are filtered out before
/// registration.
pub async fn register_server_tools(
    registry: &mut ToolRegistry,
    server: &str,
    peer: Arc<dyn McpPeer>,
    invocation: McpInvocationPolicy,
    approval: Arc<dyn McpApproval>,
) -> Result<Vec<ToolDescriptor>, McpError> {
    let capabilities = McpCapabilities::discover(peer.as_ref()).await?;
    Ok(register_discovered_tools(
        registry,
        server,
        &capabilities,
        peer,
        invocation,
        approval,
    ))
}

/// Register discovered tools (synchronous variant). Filters by the tool
/// allowlist, builds one [`McpToolAdapter`] per eligible tool, and registers
/// each into `registry`.
pub fn register_discovered_tools(
    registry: &mut ToolRegistry,
    server: &str,
    capabilities: &McpCapabilities,
    peer: Arc<dyn McpPeer>,
    invocation: McpInvocationPolicy,
    approval: Arc<dyn McpApproval>,
) -> Vec<ToolDescriptor> {
    let mut descriptors = Vec::new();
    for tool in &capabilities.tools {
        if !invocation.allowed_tools.is_empty()
            && !invocation.allowed_tools.contains(tool.name.as_ref())
        {
            continue;
        }
        let adapter = Arc::new(McpToolAdapter::new(
            server,
            tool,
            peer.clone(),
            invocation.clone(),
            approval.clone(),
        ));
        descriptors.push(adapter.descriptor());
        registry.register(adapter);
    }
    descriptors
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

/// Convert an MCP [`CallToolResult`] into a canonical [`ToolResult`], enforcing
/// the byte output cap and marking oversized output as truncated.
fn convert_call_tool_result(result: &CallToolResult, max_output_bytes: u64) -> ToolResult {
    let parts: Vec<ContentPart> = result
        .content
        .iter()
        .filter_map(content_block_to_part)
        .collect();
    let (content, mut truncated, remaining) = apply_output_cap(parts, max_output_bytes);
    let metadata = result
        .structured_content
        .as_ref()
        .and_then(|structured| {
            let metadata = json!({"mcp": {"structured_content": structured}});
            let encoded = serde_json::to_vec(&metadata).ok()?;
            if encoded.len() <= remaining {
                Some(metadata)
            } else {
                truncated = true;
                None
            }
        })
        .unwrap_or(Value::Null);
    let is_error = result.is_error.unwrap_or(false);
    let error = if is_error {
        let message = content
            .iter()
            .filter_map(|part| match part {
                ContentPart::Text(text) => Some(text.text.clone()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        Some(ErrorContext {
            category: ErrorCategory::Tool,
            message: if message.is_empty() {
                "MCP server reported a tool error".to_string()
            } else {
                message
            },
            retryable: false,
            retry_after_ms: None,
            diagnostics: Default::default(),
        })
    } else {
        None
    };

    ToolResult {
        content,
        artifacts: Vec::new(),
        metadata,
        truncated,
        success: !is_error,
        error,
    }
}

fn content_block_to_part(block: &ContentBlock) -> Option<ContentPart> {
    match block {
        ContentBlock::Text(text) => Some(ContentPart::Text(TextContent {
            text: text.text.clone(),
        })),
        ContentBlock::Image(image) => Some(ContentPart::Image(ImageContent {
            source: ImageSource::Base64(image.data.clone()),
            media_type: image.mime_type.clone(),
            alt_text: None,
        })),
        ContentBlock::Resource(embedded) => {
            let text = embedded.get_text();
            if text.is_empty() {
                None
            } else {
                Some(ContentPart::Text(TextContent { text }))
            }
        }
        // Audio / resource links / future ContentBlock variants have no
        // canonical ContentPart equivalent. `ContentBlock` is
        // `#[non_exhaustive]`, so a wildcard is required for forward compat.
        _ => None,
    }
}

/// Enforce a hard byte cap. Walks `parts` with a budget: text that overflows is
/// truncated on a UTF-8 boundary; an image that overflows is dropped. Anything
/// dropped or shortened sets `truncated = true`.
fn apply_output_cap(
    parts: Vec<ContentPart>,
    max_output_bytes: u64,
) -> (Vec<ContentPart>, bool, usize) {
    let cap = usize::try_from(max_output_bytes.max(1)).unwrap_or(usize::MAX);
    let mut budget = cap;
    let mut out = Vec::with_capacity(parts.len());
    let mut truncated = false;

    for part in parts {
        if budget == 0 {
            truncated = true;
            continue;
        }
        match part {
            ContentPart::Text(mut text_content) => {
                let text = std::mem::take(&mut text_content.text);
                let len = text.len();
                if len <= budget {
                    budget -= len;
                    out.push(ContentPart::Text(TextContent { text }));
                } else {
                    let cut = char_boundary_down(&text, budget);
                    out.push(ContentPart::Text(TextContent {
                        text: text[..cut].to_string(),
                    }));
                    truncated = true;
                    budget = 0;
                }
            }
            ContentPart::Image(image) => {
                let len = image_byte_size(&image);
                if len <= budget {
                    budget -= len;
                    out.push(ContentPart::Image(image));
                } else {
                    truncated = true;
                    budget = 0;
                }
            }
            // Other part kinds are not produced by MCP conversion; pass through
            // without consuming the budget (they are not output bytes from the server).
            other => out.push(other),
        }
    }

    (out, truncated, budget)
}

fn char_boundary_down(value: &str, mut index: usize) -> usize {
    if index >= value.len() {
        return value.len();
    }
    while index > 0 && !value.is_char_boundary(index) {
        index -= 1;
    }
    index
}

fn image_byte_size(image: &ImageContent) -> usize {
    match &image.source {
        ImageSource::Base64(data) => data.len(),
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_domain::{RunId, WorkspaceId};
    use rmcp::model::{ContentBlock, Prompt, Resource, ResourceTemplate};
    use serde_json::{json, Value};
    use std::sync::Mutex as StdMutex;
    use tool_api::ToolRequest;

    /// Mock peer: advertises one tool and returns a configurable text response.
    #[derive(Clone)]
    struct MockPeer {
        tool: Tool,
        response: CallToolResult,
        resources: Vec<Resource>,
        resource_templates: Vec<ResourceTemplate>,
        prompts: Vec<Prompt>,
        advertised: crate::McpServerCapabilities,
        honor_cancel: bool,
    }

    #[async_trait]
    impl McpPeer for MockPeer {
        async fn server_capabilities(&self) -> Result<crate::McpServerCapabilities, McpError> {
            Ok(self.advertised)
        }

        async fn list_tools(&self) -> Result<Vec<Tool>, McpError> {
            if !self.advertised.tools {
                return Err(McpError::Protocol("tools are unsupported".into()));
            }
            Ok(vec![self.tool.clone()])
        }
        async fn list_resources(&self) -> Result<Vec<Resource>, McpError> {
            if !self.advertised.resources {
                return Err(McpError::Protocol("resources are unsupported".into()));
            }
            Ok(self.resources.clone())
        }
        async fn list_resource_templates(&self) -> Result<Vec<ResourceTemplate>, McpError> {
            if !self.advertised.resources {
                return Err(McpError::Protocol(
                    "resource templates are unsupported".into(),
                ));
            }
            Ok(self.resource_templates.clone())
        }
        async fn list_prompts(&self) -> Result<Vec<Prompt>, McpError> {
            if !self.advertised.prompts {
                return Err(McpError::Protocol("prompts are unsupported".into()));
            }
            Ok(self.prompts.clone())
        }
        async fn read_resource(
            &self,
            _params: rmcp::model::ReadResourceRequestParams,
        ) -> Result<rmcp::model::ReadResourceResult, McpError> {
            Err(McpError::Protocol("not implemented in mock".into()))
        }
        async fn get_prompt(
            &self,
            _params: rmcp::model::GetPromptRequestParams,
        ) -> Result<rmcp::model::GetPromptResult, McpError> {
            Err(McpError::Protocol("not implemented in mock".into()))
        }
        async fn call_tool(
            &self,
            _params: CallToolRequestParams,
            cancel: CancellationToken,
        ) -> Result<CallToolResult, McpError> {
            if self.honor_cancel && cancel.is_cancelled() {
                return Err(McpError::Cancelled);
            }
            Ok(self.response.clone())
        }
    }

    fn make_tool(name: &str, read_only: bool) -> Tool {
        let mut tool = Tool::new(
            name.to_owned(),
            "mock tool",
            json!({"type":"object"}).as_object().unwrap().clone(),
        );
        if read_only {
            tool = tool.with_annotations(rmcp::model::ToolAnnotations::new().read_only(true));
        }
        tool
    }

    fn make_peer(read_only: bool, response_text: &str) -> MockPeer {
        MockPeer {
            tool: make_tool("search", read_only),
            response: CallToolResult::success(vec![ContentBlock::text(response_text.to_string())]),
            resources: Vec::new(),
            resource_templates: Vec::new(),
            prompts: Vec::new(),
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
            tool_call_id: agent_domain::ToolCallId::from("call-1"),
            input,
        }
    }

    /// Approve / deny channel that records every decision (auditable).
    #[derive(Clone)]
    struct RecordingApproval {
        approve: bool,
        log: Arc<StdMutex<Vec<(String, McpApprovalDecision)>>>,
    }

    impl RecordingApproval {
        fn approve() -> Self {
            Self {
                approve: true,
                log: Arc::new(StdMutex::new(Vec::new())),
            }
        }
        fn deny() -> Self {
            Self {
                approve: false,
                log: Arc::new(StdMutex::new(Vec::new())),
            }
        }
        fn recorded(&self) -> Vec<(String, McpApprovalDecision)> {
            self.log.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl McpApproval for RecordingApproval {
        async fn decide(&self, req: &McpApprovalRequest) -> McpApprovalDecision {
            let decision = if self.approve {
                McpApprovalDecision::Approved
            } else {
                McpApprovalDecision::Denied {
                    reason: "denied by test approval channel".into(),
                }
            };
            self.log
                .lock()
                .unwrap()
                .push((req.namespaced_name.clone(), decision.clone()));
            decision
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
                &tool_runtime::NoopToolEventSink,
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
            McpInvocationPolicy::default(),
            Arc::new(RecordingApproval::approve()),
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
            McpInvocationPolicy::default(),
            Arc::new(RecordingApproval::approve()),
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
    async fn untrusted_floor_denies_write_tool_without_approval() {
        let peer = Arc::new(make_peer(false, "should not run"));
        // default policy: trusted=false, ReadOnly mode → write tool hard-denied.
        let adapter = McpToolAdapter::new(
            "github",
            &make_tool("create_issue", false),
            peer,
            McpInvocationPolicy::default(),
            Arc::new(RecordingApproval::approve()),
        );
        let result = run_adapter(&adapter, json!({}), "ws", CancellationToken::new()).await;
        assert!(result.is_error());
        assert_eq!(
            result.error.as_ref().unwrap().category,
            ErrorCategory::Authorization
        );
    }

    #[tokio::test]
    async fn ask_for_writes_routes_through_approval_channel() {
        let peer = Arc::new(make_peer(false, "created"));
        let invocation = McpInvocationPolicy {
            approval_mode: ApprovalMode::AskForWrites,
            trusted: true,
            ..McpInvocationPolicy::default()
        };

        // Denied by approval channel → tool never invoked.
        let deny = Arc::new(RecordingApproval::deny());
        let adapter = McpToolAdapter::new(
            "github",
            &make_tool("create_issue", false),
            peer.clone(),
            invocation.clone(),
            deny.clone(),
        );
        let denied = run_adapter(
            &adapter,
            json!({"title":"x"}),
            "ws",
            CancellationToken::new(),
        )
        .await;
        assert!(denied.is_error());
        let record = deny.recorded();
        assert_eq!(record.len(), 1);
        assert_eq!(record[0].0, "github.create_issue");
        assert!(matches!(record[0].1, McpApprovalDecision::Denied { .. }));

        // Approved → peer invoked, content returned.
        let approve = Arc::new(RecordingApproval::approve());
        let adapter = McpToolAdapter::new(
            "github",
            &make_tool("create_issue", false),
            peer,
            invocation,
            approve,
        );
        let approved = run_adapter(
            &adapter,
            json!({"title":"x"}),
            "ws",
            CancellationToken::new(),
        )
        .await;
        assert!(approved.success);
    }

    #[tokio::test]
    async fn cancellation_aborts_before_remote_call() {
        let peer = Arc::new(make_peer(true, "late"));
        let adapter = McpToolAdapter::new(
            "github",
            &make_tool("search", true),
            peer,
            McpInvocationPolicy::default(),
            Arc::new(RecordingApproval::approve()),
        );
        let cancel = CancellationToken::new();
        cancel.cancel();
        let result = adapter
            .execute(
                request(json!({})),
                execution_context("ws"),
                &tool_runtime::NoopToolEventSink,
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
        let invocation = McpInvocationPolicy {
            max_output_bytes: 16,
            ..McpInvocationPolicy::default()
        };
        let adapter = McpToolAdapter::new(
            "github",
            &make_tool("search", true),
            peer,
            invocation,
            Arc::new(RecordingApproval::approve()),
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
            McpInvocationPolicy::default(),
            Arc::new(RecordingApproval::approve()),
        );
        let error = adapter
            .execute(
                request(json!(["not", "an", "object"])),
                execution_context("ws"),
                &tool_runtime::NoopToolEventSink,
                CancellationToken::new(),
            )
            .await
            .expect_err("non-object input must fail");
        assert_eq!(error.kind, ToolErrorKind::InvalidInput);
    }

    #[tokio::test]
    async fn structured_content_is_preserved_within_output_cap() {
        let mut peer = make_peer(true, "unused");
        peer.response = CallToolResult::structured(json!({"answer": 42}));
        let adapter = McpToolAdapter::new(
            "github",
            &make_tool("search", true),
            Arc::new(peer),
            McpInvocationPolicy {
                max_output_bytes: 256,
                ..McpInvocationPolicy::default()
            },
            Arc::new(RecordingApproval::approve()),
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
        let invocation = McpInvocationPolicy {
            allowed_workspaces: allowed,
            ..McpInvocationPolicy::default()
        };
        let adapter = McpToolAdapter::new(
            "github",
            &make_tool("search", true),
            peer,
            invocation,
            Arc::new(RecordingApproval::approve()),
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
        let invocation = McpInvocationPolicy {
            allowed_tools: allowed,
            ..McpInvocationPolicy::default()
        };

        // Registration filter drops the unallowed tool entirely.
        let mut registry = ToolRegistry::new();
        let descriptors = register_discovered_tools(
            &mut registry,
            "github",
            &McpCapabilities {
                tools: vec![make_tool("search", true)],
                ..McpCapabilities::default()
            },
            peer.clone(),
            invocation.clone(),
            Arc::new(RecordingApproval::approve()),
        );
        assert!(descriptors.is_empty());
        assert!(registry.is_empty());

        // Execution-time gate also denies if an unallowed adapter is constructed directly.
        let adapter = McpToolAdapter::new(
            "github",
            &make_tool("search", true),
            peer,
            invocation,
            Arc::new(RecordingApproval::approve()),
        );
        let result = run_adapter(&adapter, json!({}), "ws", CancellationToken::new()).await;
        assert!(result.is_error());
        assert!(result.error.as_ref().unwrap().message.contains("allowlist"));
    }

    #[tokio::test]
    async fn converts_call_tool_error_flag_to_error_result() {
        let error_response = CallToolResult::error(vec![ContentBlock::text("boom")]);
        let peer = Arc::new(MockPeer {
            tool: make_tool("search", true),
            response: error_response,
            resources: Vec::new(),
            resource_templates: Vec::new(),
            prompts: Vec::new(),
            advertised: crate::McpServerCapabilities::default(),
            honor_cancel: false,
        });
        let adapter = McpToolAdapter::new(
            "github",
            &make_tool("search", true),
            peer,
            McpInvocationPolicy::default(),
            Arc::new(RecordingApproval::approve()),
        );
        let result = run_adapter(&adapter, json!({}), "ws", CancellationToken::new()).await;
        assert!(!result.success);
        assert_eq!(result.error.as_ref().unwrap().message, "boom");
        // content preserved alongside the error context.
        assert!(!result.content.is_empty());
    }

    #[tokio::test]
    async fn discovery_reports_resources_templates_and_prompts() {
        let peer = Arc::new(MockPeer {
            tool: make_tool("search", true),
            response: CallToolResult::success(vec![ContentBlock::text("ok")]),
            resources: vec![Resource::new("file:///x", "x")],
            resource_templates: vec![ResourceTemplate::new("file:///{name}", "tpl")],
            prompts: vec![Prompt::new("greet", Some("hi"), None)],
            advertised: crate::McpServerCapabilities::default(),
            honor_cancel: false,
        });
        let caps = McpCapabilities::discover(peer.as_ref())
            .await
            .expect("discover");
        assert_eq!(caps.tools.len(), 1);
        assert_eq!(caps.resources.len(), 1);
        assert_eq!(caps.resource_templates.len(), 1);
        assert_eq!(caps.prompts.len(), 1);
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
        assert!(caps.resources.is_empty());
        assert!(caps.resource_templates.is_empty());
        assert!(caps.prompts.is_empty());
    }

    #[test]
    fn invocation_policy_bridges_from_config_permissions() {
        let permissions = McpPermissions {
            approval_mode: ApprovalMode::AskForWrites,
            max_output_bytes: 4096,
            allowed_tools: BTreeSet::from(["read".into()]),
            ..McpPermissions::default()
        };
        let policy = McpInvocationPolicy::from_permissions(&permissions, true);
        assert_eq!(policy.approval_mode, ApprovalMode::AskForWrites);
        assert!(policy.trusted);
        assert_eq!(policy.max_output_bytes, 4096);
        assert!(policy.allowed_tools.contains("read"));
    }
}
