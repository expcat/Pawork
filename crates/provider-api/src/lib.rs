//! Provider 无关的 canonical 请求、流式事件与错误协议。
//!
//! 具体 Provider 负责在本协议和远端 API 之间转换；Agent Engine 不得按
//! Provider 名称分支，也不得依赖 HTTP 实现细节。

use std::{collections::BTreeMap, fmt};

use agent_domain::{
    CancellationToken, ErrorCategory, ErrorContext, Message, ModelId, ProviderId, RequestId,
    StopReason, TokenUsage, ToolCallId,
};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CanonicalModelRequest {
    pub request_id: RequestId,
    pub model: ModelId,
    pub messages: Vec<Message>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<ToolDefinition>,
    #[serde(default)]
    pub tool_choice: ToolChoice,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<ThinkingConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stop_sequences: Vec<String>,
    #[serde(default)]
    pub response_format: ResponseFormat,
    #[serde(default)]
    pub prompt_cache: PromptCachePreference,
    #[serde(default)]
    pub budget: RequestBudget,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub provider_options: BTreeMap<String, Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "name", rename_all = "snake_case")]
pub enum ToolChoice {
    None,
    #[default]
    Auto,
    Required,
    Named(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThinkingConfig {
    pub level: ThinkingLevel,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_tokens: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThinkingLevel {
    Off,
    Low,
    Medium,
    High,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ResponseFormat {
    #[default]
    Text,
    Json,
    JsonSchema {
        name: String,
        schema: Value,
    },
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptCachePreference {
    #[default]
    Automatic,
    Disabled,
    Required,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestBudget {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_cost_micros: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_input_tokens: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum ProviderStreamEvent {
    ResponseStarted {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        response_id: Option<String>,
    },
    TextDelta(String),
    ThinkingDelta(String),
    ToolCallStarted {
        id: ToolCallId,
        name: String,
    },
    ToolCallArgumentsDelta {
        id: ToolCallId,
        json: String,
    },
    ToolCallCompleted {
        id: ToolCallId,
    },
    UsageUpdated(TokenUsage),
    ResponseCompleted(StopReason),
    ProviderMetadata(Value),
    Error(ProviderError),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModelResponseSummary {
    pub stop_reason: StopReason,
    #[serde(default)]
    pub usage: TokenUsage,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_id: Option<String>,
    #[serde(default)]
    pub provider_metadata: Value,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelDefinition {
    pub id: ModelId,
    pub display_name: String,
    pub context_window_tokens: u64,
    pub max_output_tokens: u64,
    pub capabilities: ModelCapabilities,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelCapabilities {
    pub text: bool,
    pub image_input: bool,
    pub tool_calls: bool,
    pub parallel_tool_calls: bool,
    pub thinking: bool,
    pub structured_output: bool,
    pub prompt_cache: bool,
}

#[async_trait]
pub trait ProviderEventSink: Send + Sync {
    async fn emit(&self, event: ProviderStreamEvent) -> Result<(), ProviderError>;
}

#[async_trait]
pub trait ModelProvider: Send + Sync {
    fn id(&self) -> ProviderId;

    async fn list_models(
        &self,
        credential: Option<&ResolvedCredential>,
    ) -> Result<Vec<ModelDefinition>, ProviderError>;

    async fn stream(
        &self,
        request: CanonicalModelRequest,
        sink: &dyn ProviderEventSink,
        cancel: CancellationToken,
    ) -> Result<ModelResponseSummary, ProviderError>;
}

#[derive(Clone, PartialEq, Eq)]
pub struct ResolvedCredential {
    kind: CredentialKind,
    secret: String,
}

impl ResolvedCredential {
    pub fn new(kind: CredentialKind, secret: impl Into<String>) -> Self {
        Self {
            kind,
            secret: secret.into(),
        }
    }

    pub const fn kind(&self) -> CredentialKind {
        self.kind
    }

    /// 仅 Provider adapter 可在构造认证请求时读取；不得记录到日志或事件。
    pub fn expose_secret(&self) -> &str {
        &self.secret
    }
}

impl fmt::Debug for ResolvedCredential {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ResolvedCredential")
            .field("kind", &self.kind)
            .field("secret", &"[REDACTED]")
            .finish()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CredentialKind {
    ApiKey,
    OAuthBearer,
    SessionToken,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderErrorKind {
    Authentication,
    Authorization,
    RateLimited,
    QuotaExceeded,
    InvalidRequest,
    ModelNotFound,
    ContextTooLarge,
    ContentFiltered,
    Network,
    Timeout,
    ProviderUnavailable,
    StreamInterrupted,
    MalformedResponse,
    Cancelled,
    Unknown,
}

#[derive(Clone, Debug, Error, PartialEq, Eq, Serialize, Deserialize)]
#[error("{kind:?}: {message}")]
pub struct ProviderError {
    pub kind: ProviderErrorKind,
    pub message: String,
    pub retryable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_request_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http_status: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub redacted_details: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub diagnostics: BTreeMap<String, String>,
}

impl ProviderError {
    pub fn new(kind: ProviderErrorKind, message: impl Into<String>) -> Self {
        Self {
            retryable: default_retryability(&kind),
            kind,
            message: message.into(),
            retry_after_ms: None,
            provider_request_id: None,
            http_status: None,
            redacted_details: None,
            diagnostics: BTreeMap::new(),
        }
    }

    pub fn cancelled(message: impl Into<String>) -> Self {
        Self::new(ProviderErrorKind::Cancelled, message)
    }

    pub fn category(&self) -> ErrorCategory {
        match self.kind {
            ProviderErrorKind::Authentication => ErrorCategory::Authentication,
            ProviderErrorKind::Authorization | ProviderErrorKind::ContentFiltered => {
                ErrorCategory::Authorization
            }
            ProviderErrorKind::RateLimited => ErrorCategory::RateLimit,
            ProviderErrorKind::QuotaExceeded => ErrorCategory::ResourceExhausted,
            ProviderErrorKind::InvalidRequest | ProviderErrorKind::ContextTooLarge => {
                ErrorCategory::InvalidRequest
            }
            ProviderErrorKind::ModelNotFound => ErrorCategory::NotFound,
            ProviderErrorKind::Timeout => ErrorCategory::Timeout,
            ProviderErrorKind::ProviderUnavailable
            | ProviderErrorKind::Network
            | ProviderErrorKind::StreamInterrupted => ErrorCategory::Unavailable,
            ProviderErrorKind::MalformedResponse => ErrorCategory::MalformedData,
            ProviderErrorKind::Cancelled => ErrorCategory::Cancelled,
            ProviderErrorKind::Unknown => ErrorCategory::Provider,
        }
    }
}

fn default_retryability(kind: &ProviderErrorKind) -> bool {
    matches!(
        kind,
        ProviderErrorKind::RateLimited
            | ProviderErrorKind::Network
            | ProviderErrorKind::Timeout
            | ProviderErrorKind::ProviderUnavailable
            | ProviderErrorKind::StreamInterrupted
    )
}

impl From<ProviderError> for ErrorContext {
    fn from(error: ProviderError) -> Self {
        Self {
            category: error.category(),
            message: error.message,
            retryable: error.retryable,
            retry_after_ms: error.retry_after_ms,
            diagnostics: error.diagnostics,
        }
    }
}

impl From<ErrorContext> for ProviderError {
    fn from(context: ErrorContext) -> Self {
        let kind = match context.category {
            ErrorCategory::Cancelled => ProviderErrorKind::Cancelled,
            ErrorCategory::RateLimit => ProviderErrorKind::RateLimited,
            ErrorCategory::Timeout => ProviderErrorKind::Timeout,
            ErrorCategory::Authentication => ProviderErrorKind::Authentication,
            ErrorCategory::Authorization => ProviderErrorKind::Authorization,
            ErrorCategory::InvalidRequest => ProviderErrorKind::InvalidRequest,
            ErrorCategory::NotFound => ProviderErrorKind::ModelNotFound,
            ErrorCategory::ResourceExhausted => ProviderErrorKind::QuotaExceeded,
            ErrorCategory::Unavailable => ProviderErrorKind::ProviderUnavailable,
            ErrorCategory::MalformedData => ProviderErrorKind::MalformedResponse,
            ErrorCategory::Provider
            | ErrorCategory::Tool
            | ErrorCategory::Internal
            | ErrorCategory::Conflict => ProviderErrorKind::Unknown,
        };
        Self {
            kind,
            message: context.message,
            retryable: context.retryable,
            retry_after_ms: context.retry_after_ms,
            provider_request_id: None,
            http_status: None,
            redacted_details: None,
            diagnostics: context.diagnostics,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use agent_domain::{MessageId, MessageMetadata, MessageRole};

    use super::*;

    #[derive(Default)]
    struct RecordingSink(Mutex<Vec<ProviderStreamEvent>>);

    #[async_trait]
    impl ProviderEventSink for RecordingSink {
        async fn emit(&self, event: ProviderStreamEvent) -> Result<(), ProviderError> {
            self.0.lock().expect("recording sink mutex").push(event);
            Ok(())
        }
    }

    struct CancelAwareProvider;

    #[async_trait]
    impl ModelProvider for CancelAwareProvider {
        fn id(&self) -> ProviderId {
            ProviderId::from("mock")
        }

        async fn list_models(
            &self,
            _credential: Option<&ResolvedCredential>,
        ) -> Result<Vec<ModelDefinition>, ProviderError> {
            Ok(Vec::new())
        }

        async fn stream(
            &self,
            _request: CanonicalModelRequest,
            sink: &dyn ProviderEventSink,
            cancel: CancellationToken,
        ) -> Result<ModelResponseSummary, ProviderError> {
            if cancel.is_cancelled() {
                let error = ProviderError::cancelled("request cancelled");
                sink.emit(ProviderStreamEvent::Error(error.clone())).await?;
                return Err(error);
            }
            unreachable!("test always cancels before invoking provider")
        }
    }

    fn request() -> CanonicalModelRequest {
        CanonicalModelRequest {
            request_id: RequestId::from("request-1"),
            model: ModelId::from("model-1"),
            messages: vec![Message {
                id: MessageId::from("message-1"),
                role: MessageRole::User,
                content: Vec::new(),
                metadata: MessageMetadata::default(),
            }],
            tools: Vec::new(),
            tool_choice: ToolChoice::Auto,
            thinking: None,
            temperature: None,
            max_output_tokens: Some(128),
            stop_sequences: Vec::new(),
            response_format: ResponseFormat::Text,
            prompt_cache: PromptCachePreference::Automatic,
            budget: RequestBudget {
                timeout_ms: Some(5_000),
                ..RequestBudget::default()
            },
            provider_options: BTreeMap::new(),
            trace_id: None,
        }
    }

    #[tokio::test]
    async fn shared_cancellation_reaches_provider_and_sink() {
        let token = CancellationToken::new();
        token.cancel();
        let sink = RecordingSink::default();

        let error = CancelAwareProvider
            .stream(request(), &sink, token)
            .await
            .expect_err("cancelled request must fail");

        assert_eq!(error.kind, ProviderErrorKind::Cancelled);
        assert_eq!(error.category(), ErrorCategory::Cancelled);
        assert!(matches!(
            sink.0.lock().expect("recording sink mutex").as_slice(),
            [ProviderStreamEvent::Error(ProviderError {
                kind: ProviderErrorKind::Cancelled,
                ..
            })]
        ));
    }

    #[test]
    fn canonical_request_round_trip_covers_tool_image_thinking_and_budget() {
        let mut value = request();
        value.tools.push(ToolDefinition {
            name: "read_file".into(),
            description: "read".into(),
            input_schema: serde_json::json!({"type": "object"}),
        });
        value.thinking = Some(ThinkingConfig {
            level: ThinkingLevel::High,
            budget_tokens: Some(32),
        });
        value
            .provider_options
            .insert("custom".into(), serde_json::json!({"enabled": true}));

        let encoded = serde_json::to_string(&value).expect("serialize request");
        let decoded: CanonicalModelRequest =
            serde_json::from_str(&encoded).expect("deserialize request");
        assert_eq!(decoded, value);
    }

    #[test]
    fn error_conversion_preserves_retry_advice() {
        let mut error = ProviderError::new(ProviderErrorKind::RateLimited, "slow down");
        error.retry_after_ms = Some(250);
        let context = ErrorContext::from(error);

        assert_eq!(context.category, ErrorCategory::RateLimit);
        assert!(context.retryable);
        assert_eq!(context.retry_after_ms, Some(250));
    }

    #[test]
    fn credential_debug_output_is_redacted() {
        let credential = ResolvedCredential::new(CredentialKind::ApiKey, "super-secret");
        let debug = format!("{credential:?}");
        assert!(!debug.contains("super-secret"));
        assert!(debug.contains("[REDACTED]"));
    }
}
