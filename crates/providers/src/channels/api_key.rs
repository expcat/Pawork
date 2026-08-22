//! 首发 API-key 渠道：preset 驱动的配置 / Provider，薄封装 OpenAI-compatible 传输。
//!
//! 四条渠道共用 Bearer 认证与 OpenAI-compatible transport；默认走 Chat
//! Completions，只有逐模型显式声明时才走 Responses。构造期 fail-closed：必须提供且
//! 仅接受 CredentialKind::ApiKey；preset 必须来自 CHANNEL_REGISTRY 且对应
//! feature 已启用（R5 波 A 轨 b：枚举删除，数据行单点登记）。

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use pawork_domain::{
    CanonicalModelRequest, CredentialKind, ModelDefinition, ModelProvider, ModelResponseSummary,
    ModelTransport, ProviderError, ProviderErrorKind, ProviderEventSink, ResolvedCredential,
};
use pawork_domain::{CancellationToken, ModelId, ProviderId};
use crate::net::http::HttpClientConfig;
use crate::channels::registry::{is_enabled, ChannelKind, ChannelPreset};
use crate::ReasoningProtector;

use crate::normalize_vendor_error;
use crate::provider::{OpenAiCompatibleConfig, OpenAiCompatibleProvider};
use crate::responses::{ResponsesTransport, ResponsesTransportConfig, ResponsesWireOptions};

/// API-key 渠道配置。默认带上渠道 id / URL，允许覆盖 base_url / HTTP / timeout。
#[derive(Clone, Debug)]
pub struct ApiKeyChannelConfig {
    pub preset: &'static ChannelPreset,
    pub base_url: String,
    pub http: HttpClientConfig,
    pub request_timeout: Option<Duration>,
    /// 逐模型 transport 声明。未登记模型保守退回 Chat Completions；不得按渠道名猜。
    pub model_transports: BTreeMap<ModelId, ModelTransport>,
}

impl ApiKeyChannelConfig {
    /// 构造渠道配置。preset 的 kind 必须 ApiKey 且对应 feature 已启用
    /// （fail-closed；is_enabled 是注册表唯一的 cfg 求值点）。
    pub fn new(preset: &'static ChannelPreset) -> Result<Self, ProviderError> {
        if preset.kind != ChannelKind::ApiKey {
            return Err(ProviderError::new(
                ProviderErrorKind::InvalidRequest,
                format!("channel {} is not an API-key channel", preset.id),
            ));
        }
        if !is_enabled(preset) {
            return Err(ProviderError::new(
                ProviderErrorKind::InvalidRequest,
                format!(
                    "channel {} requires feature {} which is not enabled",
                    preset.id, preset.feature
                ),
            ));
        }
        Ok(Self {
            preset,
            base_url: preset.default_base_url.into(),
            http: HttpClientConfig::default(),
            request_timeout: None,
            model_transports: BTreeMap::new(),
        })
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
    preset: &'static ChannelPreset,
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
        let provider_id = ProviderId::new(config.preset.id);
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
            preset: config.preset,
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
            .map_err(|error| normalize_vendor_error(self.preset.id, error))?;
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
                    .map_err(|error| normalize_vendor_error(self.preset.id, error))
            }
            ModelTransport::Responses => self
                .responses
                .stream(request, sink, cancel)
                .await
                .map_err(|error| normalize_vendor_error(self.preset.id, error)),
            ModelTransport::Messages => Err(ProviderError::new(
                ProviderErrorKind::InvalidRequest,
                "this API-key adapter cannot route a Messages-only model",
            )),
        }
    }
}
