use std::time::Duration;

use agent_domain::{CancellationToken, ModelId, ProviderId};
use async_trait::async_trait;
use provider_api::{
    CanonicalModelRequest, CredentialKind, ModelCapabilities, ModelDefinition, ModelProvider,
    ModelResponseSummary, ProviderError, ProviderErrorKind, ProviderEventSink, ResolvedCredential,
    ThinkingLevel,
};
use provider_openai_compatible::{OpenAiCompatibleConfig, OpenAiCompatibleProvider};
use provider_runtime::http::HttpClientConfig;

use crate::DEFAULT_BASE_URL;

/// Qwen adapter configuration.
#[derive(Clone, Debug)]
pub struct QwenConfig {
    /// DashScope-compatible base URL.
    pub base_url: String,
    /// Shared HTTP client configuration.
    pub http: HttpClientConfig,
    /// Connection and idle-read timeout override.
    pub request_timeout: Option<Duration>,
}

impl Default for QwenConfig {
    fn default() -> Self {
        Self {
            base_url: DEFAULT_BASE_URL.into(),
            http: HttpClientConfig::default(),
            request_timeout: None,
        }
    }
}

impl QwenConfig {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            ..Self::default()
        }
    }
}

/// Qwen provider using DashScope API-key Bearer authentication.
pub struct QwenProvider {
    inner: OpenAiCompatibleProvider,
}

impl QwenProvider {
    pub fn new(
        config: QwenConfig,
        credential: Option<ResolvedCredential>,
    ) -> Result<Self, ProviderError> {
        if let Some(credential) = &credential {
            if credential.kind() != CredentialKind::ApiKey {
                return Err(ProviderError::new(
                    ProviderErrorKind::Authentication,
                    "Qwen requires a DashScope API key credential",
                ));
            }
        }
        let mut http = config.http;
        if let Some(timeout) = config.request_timeout {
            http.timeout = Some(timeout);
        }
        let inner = OpenAiCompatibleProvider::new(
            OpenAiCompatibleConfig {
                base_url: config.base_url,
                provider_id: ProviderId::new("qwen"),
                http,
                request_timeout: None,
            },
            credential,
        )?;
        Ok(Self { inner })
    }
}

#[async_trait]
impl ModelProvider for QwenProvider {
    fn id(&self) -> ProviderId {
        ProviderId::new("qwen")
    }

    async fn list_models(
        &self,
        _credential: Option<&ResolvedCredential>,
    ) -> Result<Vec<ModelDefinition>, ProviderError> {
        Ok(builtin_models())
    }

    async fn stream(
        &self,
        mut request: CanonicalModelRequest,
        sink: &dyn ProviderEventSink,
        cancel: CancellationToken,
    ) -> Result<ModelResponseSummary, ProviderError> {
        // DashScope compatible-mode 使用 enable_thinking，而不是 OpenAI 的
        // reasoning_effort。仅对目录声明支持 thinking 的模型自动注入；
        // 显式 provider option 保持最高优先级。
        if let Some(thinking) = request.thinking.take() {
            if model_supports_thinking(&request.model) {
                request
                    .provider_options
                    .entry("enable_thinking".into())
                    .or_insert_with(|| (thinking.level != ThinkingLevel::Off).into());
            }
        }
        self.inner
            .stream(request, sink, cancel)
            .await
            .map_err(normalize_qwen_error)
    }
}

fn model_supports_thinking(model: &ModelId) -> bool {
    builtin_models()
        .into_iter()
        .any(|definition| definition.id == *model && definition.capabilities.thinking)
}

/// Built-in Qwen model catalog with conservative capability declarations.
/// Data snapshot: 2026-08-09; refresh deliberately remains a tracked manual task.
pub fn builtin_models() -> Vec<ModelDefinition> {
    fn model(id: &str, display_name: &str, thinking: bool) -> ModelDefinition {
        ModelDefinition {
            id: ModelId::new(id),
            display_name: display_name.into(),
            context_window_tokens: 131_072,
            max_output_tokens: 16_384,
            capabilities: ModelCapabilities {
                text: true,
                tool_calls: true,
                parallel_tool_calls: true,
                thinking,
                structured_output: true,
                prompt_cache: true,
                ..ModelCapabilities::default()
            },
        }
    }

    vec![
        model("qwen3-max", "Qwen3 Max", true),
        model("qwen-plus", "Qwen Plus", true),
        model("qwen-turbo", "Qwen Turbo", false),
    ]
}

fn normalize_qwen_error(mut error: ProviderError) -> ProviderError {
    let message = error.message.to_ascii_lowercase();
    let kind = if message.contains("datainspectionfailed")
        || message.contains("data_inspection_failed")
        || message.contains("content_filter")
    {
        Some(ProviderErrorKind::ContentFiltered)
    } else if message.contains("throttling") || message.contains("rate limit") {
        Some(ProviderErrorKind::RateLimited)
    } else if message.contains("arrearage")
        || message.contains("quotaexhausted")
        || message.contains("quota_exhausted")
    {
        Some(ProviderErrorKind::QuotaExceeded)
    } else {
        None
    };

    if let Some(kind) = kind {
        error.retryable = matches!(kind, ProviderErrorKind::RateLimited);
        error.kind = kind;
    }
    error
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_and_provider_id_are_fixed() {
        let config = QwenConfig::default();
        assert_eq!(config.base_url, DEFAULT_BASE_URL);
        let provider = QwenProvider::new(config, None).expect("provider");
        assert_eq!(provider.id(), ProviderId::new("qwen"));
    }

    #[test]
    fn catalog_marks_qwen3_as_thinking() {
        let models = builtin_models();
        assert!(models
            .iter()
            .any(|model| model.id == ModelId::new("qwen3-max") && model.capabilities.thinking));
    }

    #[test]
    fn provider_error_codes_are_normalized() {
        let filtered = normalize_qwen_error(ProviderError::new(
            ProviderErrorKind::InvalidRequest,
            "HTTP 400: DataInspectionFailed",
        ));
        assert_eq!(filtered.kind, ProviderErrorKind::ContentFiltered);

        let throttled = normalize_qwen_error(ProviderError::new(
            ProviderErrorKind::InvalidRequest,
            "HTTP 400: Throttling.RateQuota",
        ));
        assert_eq!(throttled.kind, ProviderErrorKind::RateLimited);
        assert!(throttled.retryable);
    }

    #[test]
    fn rejects_non_api_key_credentials() {
        let credential = ResolvedCredential::new(CredentialKind::OAuthBearer, "oauth");
        let error = QwenProvider::new(QwenConfig::default(), Some(credential))
            .err()
            .expect("OAuth credential must be rejected");
        assert_eq!(error.kind, ProviderErrorKind::Authentication);
    }

    #[test]
    fn model_catalog_drives_thinking_injection() {
        assert!(model_supports_thinking(&ModelId::new("qwen3-max")));
        assert!(!model_supports_thinking(&ModelId::new("qwen-turbo")));
        assert!(!model_supports_thinking(&ModelId::new("unknown")));
    }
}
