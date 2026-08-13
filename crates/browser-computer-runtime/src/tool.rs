use std::sync::Arc;

use agent_domain::{ContentPart, TextContent};
use async_trait::async_trait;
use serde_json::json;
use tool_api::{
    AgentTool, CancellationToken, ToolCapability, ToolCapabilityTag, ToolDescriptor, ToolError,
    ToolErrorKind, ToolEventSink, ToolExecutionContext, ToolHosting, ToolKind, ToolRequest,
    ToolResult, ToolStreamEvent,
};

use crate::action::BrowserComputerAction;
use crate::capability::BrowserComputerCapability;
use crate::error::BrowserComputerError;

/// Browser / Computer ClientFunction 工具（P17-10）。
///
/// 这是进入 Core Tool Scheduler 的本地执行入口，`kind = ClientFunction`。它只走
/// [`BrowserComputerCapability::act_local`]；ProviderHosted 后端从不得经此路径执行
/// （由 selector 与运行期硬门保证）。
#[derive(Clone)]
pub struct BrowserComputerTool {
    capability: Arc<BrowserComputerCapability>,
    descriptor: ToolDescriptor,
}

impl BrowserComputerTool {
    pub const TOOL_NAME: &'static str = "browser_computer";

    pub fn new(capability: Arc<BrowserComputerCapability>) -> Self {
        let descriptor = ToolDescriptor {
            name: Self::TOOL_NAME.into(),
            description: "Drive a browser or computer via the BrowserComputerCapability facade (Local/Playwright/MCP). Provider-hosted computer use is routed via ServerToolEvent and never enters this tool.".into(),
            input_schema: json!({
                "type": "object",
                "oneOf": [
                    {
                        "properties": {
                            "action": { "const": "navigate" },
                            "url": { "type": "string" }
                        },
                        "required": ["action", "url"]
                    },
                    {
                        "properties": {
                            "action": { "const": "click" },
                            "selector": { "type": "string" },
                            "coordinate": {
                                "type": "array",
                                "items": { "type": "integer" },
                                "minItems": 2,
                                "maxItems": 2
                            }
                        },
                        "required": ["action"],
                        "anyOf": [
                            { "required": ["selector"] },
                            { "required": ["coordinate"] }
                        ]
                    },
                    {
                        "properties": {
                            "action": { "const": "type" },
                            "text": { "type": "string" },
                            "selector": { "type": "string" }
                        },
                        "required": ["action", "text"]
                    },
                    {
                        "properties": {
                            "action": { "const": "key" },
                            "keys": { "type": "string" }
                        },
                        "required": ["action", "keys"]
                    },
                    {
                        "properties": {
                            "action": { "const": "scroll" },
                            "dx": { "type": "integer" },
                            "dy": { "type": "integer" }
                        },
                        "required": ["action"]
                    },
                    {
                        "properties": { "action": { "const": "screenshot" } },
                        "required": ["action"]
                    },
                    {
                        "properties": {
                            "action": { "const": "snapshot_dom" },
                            "selector": { "type": "string" }
                        },
                        "required": ["action"]
                    },
                    {
                        "properties": { "action": { "const": "title" } },
                        "required": ["action"]
                    }
                ]
            }),
            capability: ToolCapability::Network,
            kind: ToolKind::ClientFunction,
            hosting: ToolHosting::Local,
            capabilities: vec![ToolCapabilityTag::ComputerUse],
            requires_approval: true,
            read_only: false,
            supports_concurrency: false,
            default_timeout_ms: Some(30_000),
            max_output_bytes: 64 * 1024,
            allowed_in_untrusted_workspace: false,
        };
        Self {
            capability,
            descriptor,
        }
    }

    pub fn capability(&self) -> &BrowserComputerCapability {
        &self.capability
    }
}

#[async_trait]
impl AgentTool for BrowserComputerTool {
    fn descriptor(&self) -> ToolDescriptor {
        self.descriptor.clone()
    }

    async fn execute(
        &self,
        request: ToolRequest,
        context: ToolExecutionContext,
        sink: &dyn ToolEventSink,
        cancel: CancellationToken,
    ) -> Result<ToolResult, ToolError> {
        let action = BrowserComputerAction::from_input(&request.input).map_err(map_error)?;
        let snapshot = self
            .capability
            .act_local(action, &context.workspace_id, cancel)
            .await
            .map_err(map_error)?;

        let mut content: Vec<ContentPart> = Vec::new();
        if !snapshot.summary.is_empty() {
            content.push(ContentPart::Text(TextContent {
                text: snapshot.summary,
            }));
        }
        if let Some(url) = snapshot.url {
            content.push(ContentPart::Text(TextContent {
                text: format!("url: {url}"),
            }));
        }
        if let Some(title) = snapshot.title {
            content.push(ContentPart::Text(TextContent {
                text: format!("title: {title}"),
            }));
        }
        for artifact in &snapshot.artifacts {
            content.push(ContentPart::ArtifactRef(artifact.clone()));
            let _ = sink
                .emit(ToolStreamEvent::ArtifactAvailable(artifact.clone()))
                .await;
        }

        let mut result = ToolResult::success(content);
        result.artifacts = snapshot.artifacts;
        if !snapshot.metadata.is_null() {
            result.metadata = snapshot.metadata;
        }
        Ok(result)
    }
}

/// 把 capability 错误归一为 `ToolError`。
fn map_error(err: BrowserComputerError) -> ToolError {
    let (kind, retryable) = match &err {
        BrowserComputerError::InvalidInput(_) => (ToolErrorKind::InvalidInput, false),
        BrowserComputerError::Cancelled => (ToolErrorKind::Cancelled, false),
        BrowserComputerError::PolicyDenied(_)
        | BrowserComputerError::CrossTrustFallbackDenied { .. }
        | BrowserComputerError::SandboxDenied { .. } => (ToolErrorKind::PermissionDenied, false),
        BrowserComputerError::PolicyAskUser(_) => (ToolErrorKind::PermissionDenied, false),
        BrowserComputerError::NotLocallyExecutable { .. } => {
            (ToolErrorKind::NotLocallyExecutable, false)
        }
        BrowserComputerError::HostedFallbackRequired { .. } => {
            (ToolErrorKind::ExecutionFailed, true)
        }
        BrowserComputerError::NoLocalBackend => (ToolErrorKind::NotFound, false),
        BrowserComputerError::Backend { .. } => (ToolErrorKind::ExecutionFailed, true),
        BrowserComputerError::Artifact(_) | BrowserComputerError::AuditSink(_) => {
            (ToolErrorKind::Internal, true)
        }
    };
    ToolError {
        kind,
        message: err.to_string(),
        retryable,
        retry_after_ms: None,
    }
}
