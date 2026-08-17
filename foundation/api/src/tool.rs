//! 内置工具、MCP 工具与 WASM 工具共享的执行契约。
//!
//! `ToolDescriptor` / `ToolKind` / `ToolHosting` 等描述符类型定义在
//! `pawork-domain`；本模块只承载 `AgentTool` 执行面，不再做一层兼容
//! re-export 薄壳。

use pawork_domain::{
    ArtifactReference, CancellationToken, ContentPart, ErrorCategory, ErrorContext, RunId,
    ToolCallId, ToolDescriptor, WorkspaceId,
};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

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
    /// `ToolResult` 仅表示 Core 执行的 ClientFunction 结果。ProviderHosted /
    /// ProviderExtension 只能经 Provider transcript 续接，不能构造此类型。
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
    /// 该工具位点不允许 Core 本地执行（ProviderHosted / ProviderExtension）。
    NotLocallyExecutable,
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

    /// 构造「不允许本地执行」错误：hosted / extension 工具不得由 Core 执行，
    /// 结果经 Provider transcript / 中介通道回填（P15-5）。
    pub fn not_locally_executable(name: &str, site: &str) -> Self {
        Self {
            kind: ToolErrorKind::NotLocallyExecutable,
            message: format!(
                "tool `{name}` is {site} and must not be executed locally by core; \
                 its result is provided via the provider transcript"
            ),
            retryable: false,
            retry_after_ms: None,
        }
    }

    pub fn category(&self) -> ErrorCategory {
        match self.kind {
            ToolErrorKind::Cancelled => ErrorCategory::Cancelled,
            ToolErrorKind::Timeout => ErrorCategory::Timeout,
            ToolErrorKind::InvalidInput => ErrorCategory::InvalidRequest,
            ToolErrorKind::NotLocallyExecutable => ErrorCategory::Tool,
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
    use pawork_domain::{ContinuationMode, ToolCapability, ToolHosting, ToolKind};

    use super::*;

    #[test]
    fn descriptor_contains_scheduler_and_policy_fields() {
        let descriptor = ToolDescriptor {
            name: "read_file".into(),
            description: "Read a workspace-relative file".into(),
            input_schema: serde_json::json!({"type": "object"}),
            capability: ToolCapability::ReadOnly,
            kind: ToolKind::ClientFunction,
            hosting: ToolHosting::Local,
            capabilities: Vec::new(),
            requires_approval: false,
            read_only: true,
            supports_concurrency: true,
            default_timeout_ms: Some(5_000),
            max_output_bytes: 64 * 1024,
            allowed_in_untrusted_workspace: true,
        };
        let value = serde_json::to_value(&descriptor).expect("serialize descriptor");

        assert_eq!(value["kind"], "client_function");
        assert_eq!(value["hosting"]["type"], "local");
        assert_eq!(value["read_only"], true);
        assert_eq!(value["supports_concurrency"], true);
        assert_eq!(value["default_timeout_ms"], 5_000);
        assert_eq!(value["max_output_bytes"], 65_536);
        assert_eq!(value["capability"], "read_only");
    }

    #[test]
    fn tool_result_wire_has_no_writable_continuation_mode() {
        let result = ToolResult::success(Vec::new());
        let value = serde_json::to_value(&result).expect("serialize result");
        assert!(value.get("continuation").is_none());
        assert_eq!(
            ToolKind::ClientFunction.continuation_mode(),
            ContinuationMode::CoreSuppliedResult
        );
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
}
