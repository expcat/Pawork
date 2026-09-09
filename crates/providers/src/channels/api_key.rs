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
use pawork_domain::{CancellationToken, ModelId, ProviderId, Timestamp};
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
    if config.preset.id == "opencode-go" {
        // 耗尽同样是有效凭证；任何畸形窗口都不能通过 verify-then-replace。
        let usage = fetch_go_usage(config, &credential, CancellationToken::new()).await?;
        usage.rolling?;
        usage.weekly?;
        usage.monthly?;
        return Ok(());
    }
    let provider = ApiKeyChannelProvider::new(config, Some(credential))?;
    provider.list_models(None).await.map(|_| ())
}

/// 一次 Go /usage 响应；单窗解析失败不丢弃其它窗口。
#[derive(Clone, Debug)]
pub struct GoUsage {
    pub rolling: Result<GoUsageWindow, ProviderError>,
    pub weekly: Result<GoUsageWindow, ProviderError>,
    pub monthly: Result<GoUsageWindow, ProviderError>,
}

/// 官方整数已用百分比与服务端重置时刻，不推算订阅边界。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GoUsageWindow {
    pub used_percent: u64,
    pub resets_at: Timestamp,
}

/// 仅接受启用的 Go preset 和 API key；一次认证 GET 读取全部三窗。
pub async fn fetch_go_usage(
    config: ApiKeyChannelConfig,
    credential: &ResolvedCredential,
    cancel: CancellationToken,
) -> Result<GoUsage, ProviderError> {
    if config.preset.id != "opencode-go" {
        return Err(ProviderError::new(
            ProviderErrorKind::InvalidRequest,
            "usage endpoint requires the Go preset",
        ));
    }
    ApiKeyChannelConfig::new(config.preset)?;
    let credential = require_api_key(Some(credential.clone()))?;
    if config
        .http
        .extra_headers
        .iter()
        .any(|(name, _)| crate::is_credential_header(name))
    {
        return Err(ProviderError::new(
            ProviderErrorKind::InvalidRequest,
            "fixed headers must not contain credentials",
        ));
    }
    let mut http = config.http;
    if let Some(timeout) = config.request_timeout {
        http.timeout = Some(timeout);
    }
    // 非流式额度查询必须有总期限，持续滴流不能无限续期。
    // 显式禁用底层空闲超时时仍保留此端点默认 60 秒的总期限。
    let total_timeout = http.timeout.unwrap_or(Duration::from_secs(60));
    let client = crate::net::http::HttpClient::new(http)?;
    let url = format!("{}/usage", config.base_url.trim_end_matches('/'));
    let headers = [(
        "Authorization".into(),
        format!("Bearer {}", credential.expose_secret()),
    )];
    // 外层取消也覆盖 get_json 内部拿到响应头后的正文读取。
    let value = tokio::select! {
        biased;
        _ = cancel.cancelled() => return Err(ProviderError::cancelled("Go usage request cancelled")),
        _ = tokio::time::sleep(total_timeout) => return Err(ProviderError::new(
            ProviderErrorKind::Timeout, "Go usage request exceeded total timeout",
        )),
        result = client.get_json_with_headers(&url, None, &headers, cancel.clone()) =>
            result.map_err(|error| normalize_vendor_error(config.preset.id, error))?,
    };
    let usage = value
        .get("usage")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(invalid_go_usage)?;
    Ok(GoUsage {
        rolling: parse_go_window(usage.get("rolling")),
        weekly: parse_go_window(usage.get("weekly")),
        monthly: parse_go_window(usage.get("monthly")),
    })
}

fn invalid_go_usage() -> ProviderError {
    // 不回显上游字段或响应体；其中可能包含凭证。
    ProviderError::new(
        ProviderErrorKind::InvalidRequest,
        "Go returned an invalid usage response",
    )
}

fn parse_go_window(value: Option<&serde_json::Value>) -> Result<GoUsageWindow, ProviderError> {
    let value = value.ok_or_else(invalid_go_usage)?;
    let used_percent = value["percent"]
        .as_u64()
        .filter(|percent| *percent <= 100)
        .ok_or_else(invalid_go_usage)?;
    match (value["status"].as_str(), used_percent) {
        (Some("ok"), 0..=99) | (Some("rate-limited"), 100) => {}
        _ => return Err(invalid_go_usage()),
    }
    let resets_at = value["resetsAt"]
        .as_str()
        .and_then(parse_go_reset)
        .ok_or_else(invalid_go_usage)?;
    Ok(GoUsageWindow {
        used_percent,
        resets_at,
    })
}

/// 仅支持官方 Date.toISOString 的 YYYY-MM-DDTHH:mm:ss.sssZ，且 Timestamp
/// 无法表示 epoch 前时间；拒绝偏移、闰秒、无毫秒和不合法日历日期。
fn parse_go_reset(value: &str) -> Option<Timestamp> {
    let b = value.as_bytes();
    if b.len() != 24
        || b[4] != b'-'
        || b[7] != b'-'
        || b[10] != b'T'
        || b[13] != b':'
        || b[16] != b':'
        || b[19] != b'.'
        || b[23] != b'Z'
    {
        return None;
    }
    let number = |range: std::ops::Range<usize>| -> Option<u64> {
        b[range].iter().try_fold(0, |n, digit| {
            digit
                .is_ascii_digit()
                .then(|| n * 10 + u64::from(digit - b'0'))
        })
    };
    let year = number(0..4)?;
    let month = number(5..7)?;
    let day = number(8..10)?;
    let hour = number(11..13)?;
    let minute = number(14..16)?;
    let second = number(17..19)?;
    let millis = number(20..23)?;
    if year < 1970 || !(1..=12).contains(&month) || hour > 23 || minute > 59 || second > 59 {
        return None;
    }
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let month_days = [
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    if day == 0 || day > month_days[month as usize - 1] {
        return None;
    }
    let leaps_before = |y: u64| (y - 1) / 4 - (y - 1) / 100 + (y - 1) / 400;
    let days = (year - 1970) * 365 + leaps_before(year) - leaps_before(1970)
        + month_days[..month as usize - 1].iter().sum::<u64>()
        + day
        - 1;
    Some(Timestamp::from_unix_millis(
        ((days * 24 + hour) * 60 + minute) * 60_000 + second * 1000 + millis,
    ))
}
