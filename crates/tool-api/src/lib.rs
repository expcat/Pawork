//! 内置工具、MCP 工具与 WASM 工具共享的 canonical 协议。

use agent_domain::{
    ArtifactReference, ContentPart, ErrorCategory, ErrorContext, RunId, ToolCallId, WorkspaceId,
};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

pub use agent_domain::CancellationToken;

#[async_trait]
pub trait AgentTool: Send + Sync {
    fn descriptor(&self) -> ToolDescriptor;

    async fn execute(
        &self,
        request: ToolRequest,
        context: ToolExecutionContext,
        sink: &dyn ToolEventSink,
        cancel: CancellationToken,
    ) -> Result<ToolResult, ToolError>;
}

#[async_trait]
pub trait ToolEventSink: Send + Sync {
    async fn emit(&self, event: ToolStreamEvent) -> Result<(), ToolError>;
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolDescriptor {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
    pub capability: ToolCapability,
    pub read_only: bool,
    pub supports_concurrency: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_timeout_ms: Option<u64>,
    pub max_output_bytes: u64,
    #[serde(default)]
    pub allowed_in_untrusted_workspace: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolCapability {
    ReadOnly,
    WorkspaceWrite,
    GitWrite,
    Process,
    Network,
    UserInteraction,
    ExternalPlugin,
}

impl ToolCapability {
    pub const fn permits_concurrent_execution(&self) -> bool {
        matches!(self, Self::ReadOnly)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolRequest {
    pub tool_call_id: ToolCallId,
    pub input: Value,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolExecutionContext {
    pub workspace_id: WorkspaceId,
    pub run_id: RunId,
    /// 工作区相对路径；绝对路径由可信的 Workspace 服务解析。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub working_directory: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolResult {
    #[serde(default)]
    pub content: Vec<ContentPart>,
    #[serde(default)]
    pub artifacts: Vec<ArtifactReference>,
    #[serde(default)]
    pub metadata: Value,
    #[serde(default)]
    pub truncated: bool,
    pub success: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ErrorContext>,
}

impl ToolResult {
    pub fn success(content: Vec<ContentPart>) -> Self {
        Self {
            content,
            artifacts: Vec::new(),
            metadata: Value::Null,
            truncated: false,
            success: true,
            error: None,
        }
    }

    pub fn failure(error: ErrorContext) -> Self {
        Self {
            content: Vec::new(),
            artifacts: Vec::new(),
            metadata: Value::Null,
            truncated: false,
            success: false,
            error: Some(error),
        }
    }

    pub const fn is_error(&self) -> bool {
        !self.success
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum ToolStreamEvent {
    OutputDelta {
        channel: ToolOutputChannel,
        delta: String,
    },
    Progress {
        completed: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        total: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message: Option<String>,
    },
    ArtifactAvailable(ArtifactReference),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolOutputChannel {
    Stdout,
    Stderr,
    Structured,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolErrorKind {
    Cancelled,
    Timeout,
    InvalidInput,
    PermissionDenied,
    NotFound,
    Conflict,
    ExecutionFailed,
    Internal,
}

#[derive(Clone, Debug, Error, PartialEq, Eq, Serialize, Deserialize)]
#[error("{kind:?}: {message}")]
pub struct ToolError {
    pub kind: ToolErrorKind,
    pub message: String,
    #[serde(default)]
    pub retryable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u64>,
}

impl ToolError {
    pub fn cancelled(message: impl Into<String>) -> Self {
        Self {
            kind: ToolErrorKind::Cancelled,
            message: message.into(),
            retryable: false,
            retry_after_ms: None,
        }
    }

    pub fn category(&self) -> ErrorCategory {
        match self.kind {
            ToolErrorKind::Cancelled => ErrorCategory::Cancelled,
            ToolErrorKind::Timeout => ErrorCategory::Timeout,
            ToolErrorKind::InvalidInput => ErrorCategory::InvalidRequest,
            ToolErrorKind::PermissionDenied => ErrorCategory::Authorization,
            ToolErrorKind::NotFound => ErrorCategory::NotFound,
            ToolErrorKind::Conflict => ErrorCategory::Conflict,
            ToolErrorKind::ExecutionFailed => ErrorCategory::Tool,
            ToolErrorKind::Internal => ErrorCategory::Internal,
        }
    }
}

impl From<ToolError> for ErrorContext {
    fn from(error: ToolError) -> Self {
        Self {
            category: error.category(),
            message: error.message,
            retryable: error.retryable,
            retry_after_ms: error.retry_after_ms,
            diagnostics: Default::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptor_contains_scheduler_and_policy_fields() {
        let descriptor = ToolDescriptor {
            name: "read_file".into(),
            description: "Read a workspace-relative file".into(),
            input_schema: serde_json::json!({"type": "object"}),
            capability: ToolCapability::ReadOnly,
            read_only: true,
            supports_concurrency: true,
            default_timeout_ms: Some(5_000),
            max_output_bytes: 64 * 1024,
            allowed_in_untrusted_workspace: true,
        };
        let value = serde_json::to_value(&descriptor).expect("serialize descriptor");

        assert_eq!(value["read_only"], true);
        assert_eq!(value["supports_concurrency"], true);
        assert_eq!(value["default_timeout_ms"], 5_000);
        assert_eq!(value["max_output_bytes"], 65_536);
        assert_eq!(value["capability"], "read_only");
    }

    #[test]
    fn tool_error_converts_to_shared_category_without_losing_retry_context() {
        let context = ErrorContext::from(ToolError {
            kind: ToolErrorKind::Timeout,
            message: "tool timed out".into(),
            retryable: true,
            retry_after_ms: Some(100),
        });

        assert_eq!(context.category, ErrorCategory::Timeout);
        assert!(context.retryable);
        assert_eq!(context.retry_after_ms, Some(100));
    }

    #[test]
    fn shared_cancellation_token_is_visible_to_tools() {
        let token = CancellationToken::new();
        let observer = token.clone();
        token.cancel();

        assert!(observer.is_cancelled());
    }
}
