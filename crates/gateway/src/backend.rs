//! HTTP 网关边界类型与宿主端口。
use async_trait::async_trait;
use pawork_domain::{
    CancellationToken, ModelPurpose, ModelResponseSummary, ProviderError, ProviderErrorKind,
    ProviderEventSink,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct GatewayError {
    pub status: u16,
    pub code: &'static str,
    pub message: &'static str,
}
impl GatewayError {
    pub fn new(status: u16, code: &'static str, message: &'static str) -> Self {
        Self {
            status,
            code,
            message,
        }
    }
    pub fn invalid() -> Self {
        Self::new(
            400,
            "invalid_request",
            "Unsupported or invalid completion request.",
        )
    }
    pub fn cancelled() -> Self {
        Self::new(499, "cancelled", "Request cancelled.")
    }
    pub fn timeout() -> Self {
        Self::new(504, "timeout", "Model request timed out.")
    }
}
impl From<ProviderError> for GatewayError {
    fn from(error: ProviderError) -> Self {
        use ProviderErrorKind::*;
        match error.kind {
            Cancelled => Self::cancelled(),
            Timeout => Self::timeout(),
            InvalidRequest | ContextTooLarge => Self::invalid(),
            ModelNotFound => Self::new(404, "model_not_found", "Model unavailable."),
            RateLimited | QuotaExceeded => Self::new(
                429,
                "rate_limit_exceeded",
                "Provider quota or rate limit exceeded.",
            ),
            Authentication | Authorization => Self::new(
                502,
                "upstream_authentication",
                "Update provider credentials in Pawork.",
            ),
            _ => Self::new(502, "upstream_error", "Provider request failed."),
        }
    }
}
#[derive(Serialize)]
pub struct GatewayModel {
    pub id: String,
    pub object: &'static str,
    pub owned_by: String,
    pub display_name: String,
    pub context_window: u64,
    /// ADR-064（Gateway v1.1）：目录能力位，字段名与 canonical
    /// ModelPurpose wire 名一致；未知位缺省 false（fail-closed）。
    pub capabilities: GatewayModelCapabilities,
}

/// /v1/models 条目的能力摘要（canonical 用途维度的布尔投影）。
#[derive(Serialize)]
pub struct GatewayModelCapabilities {
    pub text: bool,
    pub image_input: bool,
    pub image_output: bool,
    pub video_input: bool,
    pub web_search: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GatewayChatRequest {
    pub model: String,
    pub messages: Vec<GatewayMessage>,
    #[serde(default)]
    pub stream: bool,
    pub stream_options: Option<GatewayStreamOptions>,
    pub max_tokens: Option<u64>,
    pub max_completion_tokens: Option<u64>,
    pub temperature: Option<f64>,
    pub response_format: Option<Value>,
    /// ADR-064（Gateway v1.1）：请求级 hosted web search；模型未声明
    /// WebSearch 能力时由宿主能力闸门 fail-closed（400）。
    #[serde(default)]
    pub web_search: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GatewayMessage {
    pub role: String,
    /// ADR-064（Gateway v1.1）：字符串保持 v1 文本形状；数组携带
    /// text / image_url / video_url part（识图 / 识视频输入）。
    pub content: GatewayContent,
}

/// 消息正文：v1 纯文本字符串或 v1.1 多模态 part 数组。
#[derive(Deserialize)]
#[serde(untagged)]
pub enum GatewayContent {
    Text(String),
    Parts(Vec<GatewayContentPart>),
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum GatewayContentPart {
    Text {
        text: String,
    },
    /// OpenAI 兼容形状：image_url.url 为 https:// 或 data:image/*;base64,。
    ImageUrl {
        image_url: GatewayContentUrl,
    },
    /// 远程视频引用（Host 不下载不解码，URL 原样透传给 Provider）。
    VideoUrl {
        video_url: GatewayContentUrl,
    },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GatewayContentUrl {
    pub url: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GatewayStreamOptions {
    #[serde(default)]
    pub include_usage: bool,
}

/// 网关只负责 HTTP 与客户端认证；模型路由、账户、租约和用量由宿主实现。
#[async_trait]
pub trait GatewayBackend: Send + Sync + 'static {
    /// 宿主冻结的请求与凭证快照；HTTP 层不读取其内容。
    type Completion: Send;

    /// ADR-064：按用途过滤目录；空用途集 = v1 兼容口径（仅 text 模型）。
    async fn gateway_models(
        &self,
        purposes: &[ModelPurpose],
    ) -> Result<Vec<GatewayModel>, GatewayError>;
    async fn prepare_gateway_completion(
        &self,
        input: GatewayChatRequest,
    ) -> Result<Self::Completion, GatewayError>;
    async fn run_gateway_completion(
        &self,
        client: &str,
        completion: Self::Completion,
        sink: Arc<dyn ProviderEventSink>,
        cancel: CancellationToken,
    ) -> Result<ModelResponseSummary, GatewayError>;
}
