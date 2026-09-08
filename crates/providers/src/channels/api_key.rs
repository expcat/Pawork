//! 首发 API-key 渠道：preset 驱动的配置 / Provider，薄封装 OpenAI-compatible 传输。
//!
//! API-key 渠道共用 Bearer 认证与 OpenAI-compatible transport；默认走 Chat
//! Completions；混合渠道仅接受官方逐模型声明或显式配置的 transport。构造期 fail-closed：必须提供且
//! 仅接受 CredentialKind::ApiKey；preset 必须声明 api_key 认证方法且对应
//! feature 已启用（SET-4 起按 auth_methods 数据字段判定，xAI 双认证通道复用）。

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use crate::channels::registry::{is_enabled, ChannelPreset};
use crate::net::http::HttpClientConfig;
use crate::ReasoningProtector;
use async_trait::async_trait;
use pawork_domain::{CancellationToken, ModelId, ProviderId};
use pawork_domain::{
    CanonicalModelRequest, CredentialKind, ModelDefinition, ModelProvider, ModelResponseSummary,
    ModelTransport, ProviderError, ProviderErrorKind, ProviderEventSink, ResolvedCredential,
};

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
    /// 逐模型 transport 声明；混合协议渠道由官方端点表初始化，未登记模型拒绝运行。
    pub model_transports: BTreeMap<ModelId, ModelTransport>,
}

impl ApiKeyChannelConfig {
    /// 构造渠道配置。preset 必须声明 api_key 认证方法且对应 feature 已启用
    /// （fail-closed；is_enabled 是注册表唯一的 cfg 求值点）。
    pub fn new(preset: &'static ChannelPreset) -> Result<Self, ProviderError> {
        if !preset.auth_methods.contains(&"api_key") {
            return Err(ProviderError::new(
                ProviderErrorKind::InvalidRequest,
                format!("channel {} does not declare api_key auth", preset.id),
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
            model_transports: documented_model_transports(preset.id),
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

    /// 与目录筛选和实际请求共用的协议解析；未声明的混合渠道模型返回 None。
    pub fn transport_for(&self, model: &ModelId) -> Option<ModelTransport> {
        resolve_model_transport(self.preset.id, &self.model_transports, model)
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
        let mut chat = OpenAiCompatibleProvider::new(
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
        let mut responses = ResponsesTransport::new(responses, credential)?;
        if config.preset.id == "opencode-go" {
            chat = chat.with_opencode_session();
            responses = responses.with_opencode_session();
        }
        Ok(Self {
            preset: config.preset,
            chat,
            responses,
            model_transports: config.model_transports,
        })
    }

    pub fn with_reasoning_protector(mut self, protector: Arc<dyn ReasoningProtector>) -> Self {
        self.responses = self.responses.with_reasoning_protector(protector);
        self
    }

    fn transport_for(&self, model: &ModelId) -> Option<ModelTransport> {
        resolve_model_transport(self.preset.id, &self.model_transports, model)
    }
}

fn resolve_model_transport(
    channel: &str,
    transports: &BTreeMap<ModelId, ModelTransport>,
    model: &ModelId,
) -> Option<ModelTransport> {
    transports.get(model).copied().or_else(|| {
        // 混合目录中的 ID 本身不能证明可用 transport。
        (!matches!(channel, "opencode-go" | "qwen-token-plan"))
            .then_some(ModelTransport::ChatCompletions)
    })
}

/// 官方逐模型端点快照（2026-09-08），不是按 ID 前缀猜测的路由规则。
/// Go: https://opencode.ai/docs/go/#endpoints
/// Qwen: https://help.aliyun.com/en/model-studio/qwen-code
///       https://help.aliyun.com/en/model-studio/token-plan-personal-overview
fn documented_model_transports(channel: &str) -> BTreeMap<ModelId, ModelTransport> {
    use ModelTransport::{ChatCompletions, Messages, Responses};
    let groups: &[(&[&str], ModelTransport)] = match channel {
        "opencode-go" => &[
            (
                &[
                    "grok-4.6",
                    "gpt-5.6-luna",
                    "muse-spark-1.3-contributor",
                    "muse-spark-1.2-contributor",
                ],
                Responses,
            ),
            (
                &[
                    "glm-5.3-flash",
                    "glm-5.3",
                    "glm-5.2",
                    "glm-5.1",
                    "kimi-k3",
                    "kimi-k2.7-code",
                    "kimi-k2.6",
                    "longcat-2.0",
                    "deepseek-v4-pro",
                    "deepseek-v4-flash",
                    "deepseek-v4-flash-vision-exp",
                    "mimo-v2.5",
                    "mimo-v2.5-pro",
                    "hy4-preview",
                    "hy3",
                    "omen-alpha",
                ],
                ChatCompletions,
            ),
            (
                &[
                    "minimax-m3",
                    "minimax-m2.7",
                    "minimax-m2.5",
                    "qwen3.8-max",
                    "qwen3.8-flash",
                    "qwen3.7-max",
                    "qwen3.7-plus",
                    "qwen3.6-plus",
                ],
                Messages,
            ),
        ],
        // 官方 Qwen Code 配置仅声明这些文本生成模型的 OpenAI-compatible 路径。
        // 图片、音频等模型没有 Chat/Responses 声明，因此不会进入此表。
        "qwen-token-plan" => &[(
            &[
                "qwen3.8-max",
                "qwen3.8-max-preview",
                "qwen3.8-flash",
                "qwen3.7-max",
                "qwen3.7-plus",
                "qwen3.6-flash",
                "glm-5.2",
                "deepseek-v4-pro",
                "deepseek-v4-pro-0813",
                "deepseek-v4-flash-0731",
            ],
            ChatCompletions,
        )],
        _ => &[],
    };
    groups
        .iter()
        .flat_map(|(models, transport)| {
            models.iter().map(move |id| (ModelId::new(*id), *transport))
        })
        .collect()
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
        models.retain_mut(|model| match self.transport_for(&model.id) {
            Some(transport @ (ModelTransport::ChatCompletions | ModelTransport::Responses)) => {
                model.capabilities.transport = transport;
                true
            }
            Some(ModelTransport::Messages) | None => false,
        });
        Ok(models)
    }

    async fn stream(
        &self,
        request: CanonicalModelRequest,
        sink: &dyn ProviderEventSink,
        cancel: CancellationToken,
    ) -> Result<ModelResponseSummary, ProviderError> {
        let transport = self.transport_for(&request.model).ok_or_else(|| {
            ProviderError::new(
                ProviderErrorKind::InvalidRequest,
                "model has no declared runnable transport for this channel",
            )
        })?;
        match transport {
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

/// SET-2（ADR-046 D2）Settings verify-then-replace 的验证入口：
/// 用候选 key 发已认证 GET；Go 的公开 /models 不验证凭证，改用 /usage。
/// 明文只存在于本次请求的 Authorization 头与栈上，
/// 不持久化、不记录；调用方仅在 Ok 时才写入 SecretBackend。
pub async fn verify_api_key(
    config: ApiKeyChannelConfig,
    candidate_key: &str,
) -> Result<(), ProviderError> {
    let credential = ResolvedCredential::new(CredentialKind::ApiKey, candidate_key);
    // 先经过 adapter 的 kind / 空 key / 凭证头校验，专用验证不能绕过它。
    let provider = ApiKeyChannelProvider::new(config.clone(), Some(credential))?;
    if config.preset.id == "opencode-go" {
        // 官方契约：https://github.com/anomalyco/opencode/blob/d4704347465c1ee63d0c213ed00e648e7f0231c5/packages/console/app/src/routes/zen/go/v1/usage.ts
        // 只验证 key 与 Go 订阅；rate-limited 同样是有效凭证，不发推理请求。
        let mut http = config.http;
        if let Some(timeout) = config.request_timeout {
            http.timeout = Some(timeout);
        }
        let value = crate::net::http::HttpClient::new(http)?
            .get_json_with_headers(
                &format!("{}/usage", config.base_url.trim_end_matches('/')),
                None,
                &[("Authorization".into(), format!("Bearer {candidate_key}"))],
                CancellationToken::new(),
            )
            .await
            .map_err(|error| normalize_vendor_error(config.preset.id, error))?;
        let valid = ["rolling", "weekly", "monthly"].iter().all(|window| {
            let usage = &value["usage"][*window];
            matches!(usage["status"].as_str(), Some("ok" | "rate-limited"))
                && usage["percent"]
                    .as_f64()
                    .is_some_and(|percent| percent >= 0.0)
                && usage["resetsAt"]
                    .as_str()
                    .is_some_and(|time| !time.is_empty())
        });
        return if valid {
            Ok(())
        } else {
            Err(ProviderError::new(
                ProviderErrorKind::InvalidRequest,
                "Go credential verification returned an invalid usage response",
            ))
        };
    }
    provider.list_models(None).await.map(|_| ())
}
