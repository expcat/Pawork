//! 首发 API-key 渠道：一个中立枚举 / 配置 / Provider，薄封装 OpenAI-compatible 传输。
//!
//! 四条渠道共用 Bearer 认证与 OpenAI-compatible transport；默认走 Chat
//! Completions，只有逐模型显式声明时才走 Responses。构造期 fail-closed：必须提供且
//! 仅接受 CredentialKind::ApiKey。

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use pawork_domain::{
    CanonicalModelRequest, CredentialKind, ModelDefinition, ModelProvider, ModelResponseSummary,
    ModelTransport, ProviderError, ProviderErrorKind, ProviderEventSink, ResolvedCredential,
};
use pawork_domain::{CancellationToken, ModelId, ProviderId};
use pawork_net::http::HttpClientConfig;
use pawork_provider_core::ReasoningProtector;

use crate::normalize_vendor_error;
use crate::provider::{OpenAiCompatibleConfig, OpenAiCompatibleProvider};
use crate::responses::{ResponsesTransport, ResponsesTransportConfig, ResponsesWireOptions};

/// 首发 API-key 渠道。渠道名只用于默认 id / URL，不进入传输特例。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ApiKeyChannel {
    #[cfg(feature = "glm-coding")]
    GlmCoding,
    #[cfg(feature = "opencode-go")]
    OpenCodeGo,
    #[cfg(feature = "qwen-token-plan")]
    QwenTokenPlan,
    #[cfg(feature = "deepseek")]
    DeepSeek,
}

impl ApiKeyChannel {
    pub const ALL: &'static [Self] = &[
        #[cfg(feature = "glm-coding")]
        Self::GlmCoding,
        #[cfg(feature = "opencode-go")]
        Self::OpenCodeGo,
        #[cfg(feature = "qwen-token-plan")]
        Self::QwenTokenPlan,
        #[cfg(feature = "deepseek")]
        Self::DeepSeek,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            #[cfg(feature = "glm-coding")]
            Self::GlmCoding => "glm-coding",
            #[cfg(feature = "opencode-go")]
            Self::OpenCodeGo => "opencode-go",
            #[cfg(feature = "qwen-token-plan")]
            Self::QwenTokenPlan => "qwen-token-plan",
            #[cfg(feature = "deepseek")]
            Self::DeepSeek => "deepseek",
        }
    }

    pub const fn default_base_url(self) -> &'static str {
        match self {
            #[cfg(feature = "glm-coding")]
            Self::GlmCoding => "https://api.z.ai/api/coding/paas/v4",
            #[cfg(feature = "opencode-go")]
            Self::OpenCodeGo => "https://opencode.ai/zen/go/v1",
            #[cfg(feature = "qwen-token-plan")]
            Self::QwenTokenPlan => {
                "https://token-plan.cn-beijing.maas.aliyuncs.com/compatible-mode/v1"
            }
            #[cfg(feature = "deepseek")]
            Self::DeepSeek => "https://api.deepseek.com",
        }
    }

    pub fn provider_id(self) -> ProviderId {
        ProviderId::new(self.as_str())
    }
}

/// API-key 渠道配置。默认带上渠道 id / URL，允许覆盖 base_url / HTTP / timeout。
#[derive(Clone, Debug)]
pub struct ApiKeyChannelConfig {
    pub channel: ApiKeyChannel,
    pub base_url: String,
    pub http: HttpClientConfig,
    pub request_timeout: Option<Duration>,
    /// 逐模型 transport 声明。未登记模型保守退回 Chat Completions；不得按渠道名猜。
    pub model_transports: BTreeMap<ModelId, ModelTransport>,
}

impl ApiKeyChannelConfig {
    pub fn new(channel: ApiKeyChannel) -> Self {
        Self {
            channel,
            base_url: channel.default_base_url().into(),
            http: HttpClientConfig::default(),
            request_timeout: None,
            model_transports: BTreeMap::new(),
        }
    }

    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }

    pub fn with_http(mut self, http: HttpClientConfig) -> Self {
        self.http = http;
        self
    }

    pub fn with_request_timeout(mut self, timeout: Duration) -> Self {
        self.request_timeout = Some(timeout);
        self
    }

    /// 为混合协议渠道（OpenCode Go / Qwen Token Plan）声明单个模型的 wire transport。
    pub fn with_model_transport(
        mut self,
        model: impl Into<String>,
        transport: ModelTransport,
    ) -> Self {
        self.model_transports
            .insert(ModelId::new(model.into()), transport);
        self
    }
}

/// 首发 API-key Provider：校验凭证后按模型声明委托 Chat 或 Responses transport。
pub struct ApiKeyChannelProvider {
    channel: ApiKeyChannel,
    chat: OpenAiCompatibleProvider,
    responses: ResponsesTransport,
    model_transports: BTreeMap<ModelId, ModelTransport>,
}

impl ApiKeyChannelProvider {
    /// 构造适配器。缺少凭证或 kind 不是 ApiKey 时返回 Authentication。
    pub fn new(
        config: ApiKeyChannelConfig,
        credential: Option<ResolvedCredential>,
    ) -> Result<Self, ProviderError> {
        let credential = require_api_key(credential)?;
        let provider_id = config.channel.provider_id();
        let chat = OpenAiCompatibleProvider::new(
            OpenAiCompatibleConfig {
                base_url: config.base_url.clone(),
                provider_id: provider_id.clone(),
                http: config.http.clone(),
                request_timeout: config.request_timeout,
            },
            Some(credential.clone()),
        )?;
        let mut responses = ResponsesTransportConfig::new(config.base_url, provider_id.to_string());
        responses.http = config.http;
        responses.request_timeout = config.request_timeout;
        responses.wire = ResponsesWireOptions {
            store: None,
            include_encrypted_reasoning: true,
        };
        Ok(Self {
            channel: config.channel,
            chat,
            responses: ResponsesTransport::new(responses, credential)?,
            model_transports: config.model_transports,
        })
    }

    pub fn with_reasoning_protector(mut self, protector: Arc<dyn ReasoningProtector>) -> Self {
        self.responses = self.responses.with_reasoning_protector(protector);
        self
    }

    fn transport_for(&self, model: &ModelId) -> ModelTransport {
        self.model_transports
            .get(model)
            .copied()
            .unwrap_or(ModelTransport::ChatCompletions)
    }
}

fn require_api_key(
    credential: Option<ResolvedCredential>,
) -> Result<ResolvedCredential, ProviderError> {
    let credential = credential.ok_or_else(|| {
        ProviderError::new(
            ProviderErrorKind::Authentication,
            "API-key channel requires an API key credential",
        )
    })?;
    if credential.kind() != CredentialKind::ApiKey || credential.expose_secret().trim().is_empty() {
        return Err(ProviderError::new(
            ProviderErrorKind::Authentication,
            "API-key channel accepts only CredentialKind::ApiKey",
        ));
    }
    Ok(credential)
}

#[async_trait]
impl ModelProvider for ApiKeyChannelProvider {
    fn id(&self) -> ProviderId {
        self.chat.id()
    }

    async fn list_models(
        &self,
        credential: Option<&ResolvedCredential>,
    ) -> Result<Vec<ModelDefinition>, ProviderError> {
        let mut models = self
            .chat
            .list_models(credential)
            .await
            .map_err(|error| normalize_vendor_error(self.channel.as_str(), error))?;
        for model in &mut models {
            if let Some(transport) = self.model_transports.get(&model.id) {
                model.capabilities.transport = *transport;
            }
        }
        Ok(models)
    }

    async fn stream(
        &self,
        request: CanonicalModelRequest,
        sink: &dyn ProviderEventSink,
        cancel: CancellationToken,
    ) -> Result<ModelResponseSummary, ProviderError> {
        match self.transport_for(&request.model) {
            ModelTransport::ChatCompletions => {
                if !request.hosted_tools.is_empty() || !request.extensions.is_empty() {
                    return Err(ProviderError::new(
                        ProviderErrorKind::InvalidRequest,
                        "API-key Chat Completions model does not declare provider-hosted tools",
                    ));
                }
                self.chat
                    .stream(request, sink, cancel)
                    .await
                    .map_err(|error| normalize_vendor_error(self.channel.as_str(), error))
            }
            ModelTransport::Responses => self
                .responses
                .stream(request, sink, cancel)
                .await
                .map_err(|error| normalize_vendor_error(self.channel.as_str(), error)),
            ModelTransport::Messages => Err(ProviderError::new(
                ProviderErrorKind::InvalidRequest,
                "this API-key adapter cannot route a Messages-only model",
            )),
        }
    }
}
