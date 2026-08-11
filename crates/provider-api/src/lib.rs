//! Provider 无关的 canonical 请求、流式事件与错误协议。
//!
//! 具体 Provider 负责在本协议和远端 API 之间转换；Agent Engine 不得按
//! Provider 名称分支，也不得依赖 HTTP 实现细节。

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
};

use agent_domain::{
    CancellationToken, ErrorCategory, ErrorContext, Message, ModelId, ProviderId,
    ProviderTranscriptEnvelope, ReasoningItem, RequestId, ServerToolEvent, StopReason, TokenUsage,
    ToolCallId, ToolCapabilityTag,
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
    /// ClientFunction 工具定义（Core 本地执行，随请求带给 Provider）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<ToolDefinition>,
    /// ProviderHosted 工具声明（Provider 服务端执行；P15-1）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hosted_tools: Vec<HostedToolRequest>,
    /// ProviderExtension 工具声明（Provider 中介的外部工具；P15-1）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extensions: Vec<ExtensionToolRequest>,
    #[serde(default)]
    pub tool_choice: ToolChoice,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<ThinkingConfig>,
    /// 现代权威 reasoning 请求（P15-8）。显式 effort 优先于 `thinking.level`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<ReasoningConfig>,
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
    /// Provider-specific wire options. Adapters merge non-reserved keys after
    /// canonical translation, so a same-name non-critical wire field may override
    /// its translated value. Critical canonical and authentication keys (model,
    /// messages, stream, tools, tool choice, and auth headers) must be ignored.
    /// Adapters must also reserve fields whose override would violate a wire-level
    /// invariant (for example Anthropic `max_tokens` / `thinking`).
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

/// ProviderHosted 工具声明：Provider 服务端内置工具（如 `web_search`）。
///
/// 只携带 canonical 信息，不携带 Provider 名称；由 P15-2/3/4 适配器翻译为各
/// Provider 的内置工具参数。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HostedToolRequest {
    /// canonical 工具名（如 `web_search`）。
    pub name: String,
    /// 服务端工具类别（如 WebSearch）。
    pub kind: agent_domain::ToolCapabilityTag,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub capabilities: Vec<agent_domain::ToolCapabilityTag>,
    /// Provider 中立的附加配置（不得包含 Provider 名称 / Secret）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config: Option<Value>,
}

/// ProviderExtension 工具声明：Provider 中介的外部工具 / 连接器 / 远程 MCP。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ExtensionToolRequest {
    /// canonical 工具名。
    pub name: String,
    /// 外部工具引用（MCP server / connector / remote endpoint）。
    pub reference: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub capabilities: Vec<agent_domain::ToolCapabilityTag>,
    /// 是否要求显式审批（未信任工作区默认拒绝）。
    #[serde(default)]
    pub requires_approval: bool,
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
    /// Complete provider reasoning continuation item. Sensitive continuation
    /// bytes have already been replaced with a Protected Blob Store reference.
    ReasoningItem(ReasoningItem),
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
    /// Provider 归一后的 server tool 生命周期事件（P15-5）。
    ///
    /// 由适配器逐 item 发射（web_search / live_search / file_search /
    /// code_execution 等 Provider-owned 工具），Core 只归一为
    /// [`ServerToolEvent`] / transcript envelope，不触发本地执行、不生成
    /// `ToolResult`。
    ServerTool(ServerToolEvent),
    /// Provider transcript 续传信封（provider-neutral）。
    ///
    /// 仅供 `ContinuationMode::ProviderTranscript`（Hosted / Extension）使用，
    /// 携带归一化 output item / cursor / continuation reference，不携带 Provider
    /// 名称；具体协议翻译封装在 provider adapter。
    TranscriptEnvelope(ProviderTranscriptEnvelope),
    Error(ProviderError),
}

/// server tool wire 字段无法映射到 canonical 类型时的错误。
///
/// 三家字段映射对不上的口径必须返回 `Unsupported`，而不是猜测值。
#[derive(Clone, Debug, Error, PartialEq, Eq, Serialize, Deserialize)]
#[error("server tool mapping unsupported: {0}")]
pub struct ServerToolMappingError(pub String);

impl ServerToolMappingError {
    pub fn unsupported(detail: impl Into<String>) -> Self {
        Self(detail.into())
    }
}

/// Reasoning continuation wire 字段无法安全映射时的错误。
///
/// 错误详情只能描述缺失字段或不支持的结构，不得包含 encrypted content、
/// signature 或其它 protected blob 明文。
#[derive(Clone, Debug, Error, PartialEq, Eq, Serialize, Deserialize)]
#[error("reasoning mapping unsupported: {0}")]
pub struct ReasoningMappingError(pub String);

impl ReasoningMappingError {
    pub fn unsupported(detail: impl Into<String>) -> Self {
        Self(detail.into())
    }
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
/// 模型/Provider 能力声明（P15-8 v2）。
///
/// v1 布尔字段（text/image_input/.../prompt_cache）保留为兼容基线（P6），
/// v2 新增字段逐项 `#[serde(default)]`，旧目录 / 旧序列化数据缺字段时按
/// fail-closed 默认值（不支持）反序列化。`ReasoningConfig` 是现代权威
/// reasoning 入口，v1 `thinking: bool` 保留为派生源。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelCapabilities {
    pub text: bool,
    pub image_input: bool,
    pub tool_calls: bool,
    pub parallel_tool_calls: bool,
    pub thinking: bool,
    pub structured_output: bool,
    pub prompt_cache: bool,
    // ---------- P15-8 v2 ----------
    /// 模型声明的传输路径（仅声明驱动，禁止按 Provider 名推断）。
    #[serde(default)]
    pub transport: ModelTransport,
    /// Provider 服务端内置工具能力标签（WebSearch / CodeExecution / ...）。
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub hosted_tool_tags: BTreeSet<ToolCapabilityTag>,
    /// 是否支持 Citation / Source 归一（P15-5）。
    #[serde(default)]
    pub citations: bool,
    /// reasoning continuation 维度能力（encrypted / signature / interleaved）。
    #[serde(default)]
    pub reasoning: ReasoningStateCapability,
}

/// Canonical 传输路径（P15-8）。transport 选择只能由逐模型声明驱动，
/// `CapabilityNegotiator` 据此选择，不按 Provider 名推断。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelTransport {
    /// OpenAI Responses（现代路径）。
    Responses,
    /// Anthropic Modern Messages。
    Messages,
    /// OpenAI Chat Completions / P6 基线（降级 fallback）。
    #[default]
    ChatCompletions,
}
impl ModelTransport {
    /// 是否属于「现代」传输（Responses / Messages）。
    pub fn is_modern(self) -> bool {
        matches!(self, Self::Responses | Self::Messages)
    }
}

/// Canonical reasoning effort（P15-8）。
///
/// 显式 `ReasoningConfig` 优先；旧 `ThinkingConfig.level` 仅在缺省时派生；
/// `XHigh / Max` 进入旧 P6 adapter 时显式 clamp 为 `High`，不形成双轨。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReasoningEffort {
    None,
    Low,
    #[default]
    Medium,
    High,
    XHigh,
    Max,
}

impl ReasoningEffort {
    /// 是否要求模型声明 reasoning 能力（任何非 None effort）。
    pub fn requires_reasoning_support(self) -> bool {
        !matches!(self, Self::None)
    }

    /// clamp 到旧 `ThinkingLevel`（XHigh / Max → High），供 P6 adapter 复用。
    pub fn clamp_to_thinking_level(self) -> ThinkingLevel {
        match self {
            Self::None => ThinkingLevel::Off,
            Self::Low => ThinkingLevel::Low,
            Self::Medium => ThinkingLevel::Medium,
            Self::High | Self::XHigh | Self::Max => ThinkingLevel::High,
        }
    }
}

/// reasoning continuation state 最小结构（P15-8 / P15-7）。
///
/// 只表达「是否需要」signature / encrypted / interleaved，绝不存明文 token、
/// encrypted_content、signature 等 protected blob 内容（见 ADR-032）。这是
/// 模型 *能力* 维度（声明本模型接受/产出这些 wire 字段），与
/// [`ReasoningConfig`] 的运行时请求不同。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReasoningStateDescriptor {
    /// 是否需要 / 产出签名（Anthropic signature / Responses encrypted continuation）。
    #[serde(default)]
    pub requires_signature: bool,
    /// 是否需要 / 产出加密 continuation（Responses encrypted_content / redacted_thinking）。
    #[serde(default)]
    pub requires_encrypted: bool,
    /// 是否支持 interleaved thinking + tool call（Anthropic interleaved thinking）。
    #[serde(default)]
    pub supports_interleaved: bool,
}

/// 模型声明的 reasoning 维度能力（v2）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReasoningStateCapability {
    #[serde(default)]
    pub state: ReasoningStateDescriptor,
    /// 是否支持 effort 调节（XHigh / Max 等需要单独声明）。
    #[serde(default)]
    pub supports_granular_effort: bool,
}

/// 现代权威 reasoning 请求字段（P15-8）。
///
/// 显式 `effort` 优先于旧 `ThinkingConfig.level`；`state` 只表达是否需要
/// signature / encrypted / interleaved 维度，不携带明文 / 签名 / 密文。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReasoningConfig {
    #[serde(default)]
    pub effort: ReasoningEffort,
    #[serde(default)]
    pub state: ReasoningStateDescriptor,
}

impl ReasoningConfig {
    pub fn new(effort: ReasoningEffort) -> Self {
        Self {
            effort,
            state: ReasoningStateDescriptor::default(),
        }
    }

    /// 是否要求模型声明 reasoning 能力。
    pub fn requires_reasoning_support(&self) -> bool {
        self.effort.requires_reasoning_support()
    }
}

/// 请求侧能力要求（P15-8 CapabilityNegotiator 输入）。
///
/// `transport_pref` 表示请求方偏好的传输集合（按优先级），未声明则由协商
/// 根据 evidence 选最大支持。`required_tools` 是请求要求的服务端工具标签；
/// `reasoning` 是运行时 reasoning 请求（None = 不要求 reasoning）。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityRequirements {
    /// 偏好的传输路径，按顺序优先；空表示「不约束，由 evidence 决定」。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub transport_pref: Vec<ModelTransport>,
    /// 要求的服务端工具能力标签（交集判 unsupported）。
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub required_tools: BTreeSet<ToolCapabilityTag>,
    /// 运行时 reasoning 请求（None = 不要求 reasoning）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<ReasoningConfig>,
    /// 是否要求 citation / source 归一。
    #[serde(default)]
    pub citations: bool,
}

/// 协商降级动作（P15-8）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityFallback {
    /// 退回 Core 本地 ClientFunction 等价承接。
    ClientTool,
    /// 退回 P6 基线传输（ChatCompletions）。
    LegacyTransport,
    /// effort 被 clamp（XHigh / Max → High）。
    ClampedEffort,
    /// 不支持，请求前必须失败并给出可读原因。
    Reject(String),
}

/// 协商结果（P15-8 CapabilityNegotiator 输出）。
///
/// `requested == supported ∪ unsupported`：每项请求必须显式落到 supported
/// 或 unsupported，禁止静默丢弃或伪造。`chosen_transport` 是协商后最终传输
/// 路径；`fallback` 记录逐项降级原因，可解释「为何降级」。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedCapabilities {
    /// 请求的全部能力标签（reasoning / citations / hosted tools）。
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub requested: BTreeSet<String>,
    /// 证据层支持的能力（交集）。
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub supported: BTreeSet<String>,
    /// 请求但未声明支持的能力（fail-closed）。
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub unsupported: BTreeSet<String>,
    /// 协商后选定的传输路径。
    #[serde(default)]
    pub chosen_transport: ModelTransport,
    /// 逐项降级原因（capability -> reason），可解释「为何降级」。
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub fallback: BTreeMap<String, CapabilityFallback>,
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
            hosted_tools: Vec::new(),
            extensions: Vec::new(),
            tool_choice: ToolChoice::Auto,
            thinking: None,
            reasoning: None,
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
        value.hosted_tools.push(HostedToolRequest {
            name: "web_search".into(),
            kind: agent_domain::ToolCapabilityTag::WebSearch,
            description: "search the web".into(),
            capabilities: vec![agent_domain::ToolCapabilityTag::WebSearch],
            config: Some(serde_json::json!({"max_results": 5})),
        });
        value.extensions.push(ExtensionToolRequest {
            name: "remote_mcp".into(),
            reference: "mcp://connector/search".into(),
            description: "remote mcp search".into(),
            capabilities: Vec::new(),
            requires_approval: true,
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
    fn canonical_request_declares_all_three_tool_classes_without_provider_names() {
        let mut value = request();
        value.tools.push(ToolDefinition {
            name: "read_file".into(),
            description: "read".into(),
            input_schema: serde_json::json!({"type": "object"}),
        });
        value.hosted_tools.push(HostedToolRequest {
            name: "web_search".into(),
            kind: agent_domain::ToolCapabilityTag::WebSearch,
            description: String::new(),
            capabilities: Vec::new(),
            config: None,
        });
        value.extensions.push(ExtensionToolRequest {
            name: "connector_x".into(),
            reference: "connector:remote-mcp".into(),
            description: String::new(),
            capabilities: Vec::new(),
            requires_approval: true,
        });

        let json = serde_json::to_value(&value).expect("serialize request");
        assert_eq!(json["tools"][0]["name"], "read_file");
        assert_eq!(json["hosted_tools"][0]["name"], "web_search");
        assert_eq!(json["hosted_tools"][0]["kind"], "web_search");
        assert_eq!(json["extensions"][0]["reference"], "connector:remote-mcp");
        assert_eq!(json["extensions"][0]["requires_approval"], true);

        // no_provider_branch 风格断言：三类声明均不携带 Provider 名称字段。
        let serialized = serde_json::to_string(&json).expect("stringify request");
        for forbidden in ["provider_id", "provider_name", "api_key", "secret"] {
            assert!(
                !serialized.contains(forbidden),
                "canonical request must not carry `{forbidden}`"
            );
        }
    }

    #[test]
    fn provider_stream_event_round_trips_server_tool_and_transcript_envelope() {
        use agent_domain::{
            ArtifactId, Citation, CitationSourceKind, ProgramStream, ProviderTranscriptEnvelope,
            ServerToolEvent, TranscriptItem,
        };

        let events = vec![
            ProviderStreamEvent::ServerTool(ServerToolEvent::Started {
                tool_call_id: ToolCallId::from("server-tool-1"),
                name: "web_search".into(),
                arguments: Some(serde_json::json!({"query": "pawork"})),
            }),
            ProviderStreamEvent::ServerTool(ServerToolEvent::CitationAdded {
                tool_call_id: ToolCallId::from("server-tool-1"),
                citation: Citation {
                    url: Some("https://example.com".into()),
                    title: Some("Example".into()),
                    source_kind: CitationSourceKind::WebSearch,
                    ..Citation::empty()
                },
            }),
            ProviderStreamEvent::ServerTool(ServerToolEvent::ProgramOutput {
                tool_call_id: ToolCallId::from("server-tool-1"),
                stream: ProgramStream::Stdout,
                delta: None,
                artifact: Some(ArtifactId::from("artifact-log-1")),
            }),
            ProviderStreamEvent::TranscriptEnvelope(ProviderTranscriptEnvelope {
                items: vec![
                    TranscriptItem::ServerTool(ServerToolEvent::Completed {
                        tool_call_id: ToolCallId::from("server-tool-1"),
                        summary: Some("done".into()),
                        artifacts: vec![ArtifactId::from("artifact-1")],
                    }),
                    TranscriptItem::Text("final".into()),
                ],
                cursor: Some("cursor-1".into()),
                continuation_reference: Some("ref-1".into()),
            }),
        ];

        for event in &events {
            let value = serde_json::to_value(event).expect("serialize stream event");
            let decoded: ProviderStreamEvent =
                serde_json::from_value(value).expect("deserialize stream event");
            assert_eq!(&decoded, event);
        }

        // transcript envelope 不携带 Provider 名称（no_provider_branch 断言）。
        let envelope = serde_json::to_string(&events[3]).expect("serialize envelope");
        for forbidden in [
            "provider",
            "openai",
            "anthropic",
            "xai",
            "api_key",
            "secret",
        ] {
            assert!(
                !envelope.contains(forbidden),
                "transcript envelope must not carry `{forbidden}`"
            );
        }
    }

    #[test]
    fn provider_stream_reasoning_item_round_trip_uses_safe_reference() {
        let item = agent_domain::ReasoningItem {
            id: agent_domain::ReasoningItemId::from("reasoning-1"),
            summary: Some("safe summary".into()),
            protected_blob_ref: agent_domain::ProtectedBlobRef::from("protected-1"),
            opaque_metadata: BTreeMap::new(),
            continuation_metadata: BTreeMap::new(),
        };
        let event = ProviderStreamEvent::ReasoningItem(item.clone());

        let encoded = serde_json::to_string(&event).expect("serialize reasoning event");
        let decoded: ProviderStreamEvent =
            serde_json::from_str(&encoded).expect("deserialize reasoning event");

        assert_eq!(decoded, event);
        assert!(encoded.contains(item.protected_blob_ref.as_str()));
        for forbidden in ["encrypted_content", "signature", "reasoning_content"] {
            assert!(!encoded.contains(forbidden));
        }
    }

    #[test]
    fn server_tool_mapping_error_round_trips_and_is_unsupported() {
        let error = ServerToolMappingError::unsupported("unknown citation type `page_location`");
        assert!(error.to_string().contains("unsupported"));
        let value = serde_json::to_value(&error).expect("serialize mapping error");
        let decoded: ServerToolMappingError =
            serde_json::from_value(value).expect("deserialize mapping error");
        assert_eq!(decoded, error);
    }

    #[test]
    fn reasoning_mapping_error_round_trips_without_credential_material() {
        let error = ReasoningMappingError::unsupported("missing encrypted continuation field");
        let encoded = serde_json::to_string(&error).expect("serialize mapping error");
        let decoded: ReasoningMappingError =
            serde_json::from_str(&encoded).expect("deserialize mapping error");

        assert_eq!(decoded, error);
        for forbidden in ["encrypted_content=", "signature=", "reasoning_content="] {
            assert!(!encoded.contains(forbidden));
        }
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
