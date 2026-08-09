use std::time::Duration;

use agent_domain::{CancellationToken, ModelId, ProviderId};
use async_trait::async_trait;
use provider_api::{
    CanonicalModelRequest, CredentialKind, ModelCapabilities, ModelDefinition, ModelProvider,
    ModelResponseSummary, ProviderError, ProviderErrorKind, ProviderEventSink, ResolvedCredential,
};
use provider_openai_compatible::{OpenAiCompatibleConfig, OpenAiCompatibleProvider};
use provider_runtime::http::HttpClientConfig;

use crate::DEFAULT_BASE_URL;

/// Moonshot adapter configuration.
#[derive(Clone, Debug)]
pub struct MoonshotConfig {
    /// Moonshot-compatible base URL.
    pub base_url: String,
    /// Shared HTTP client configuration.
    pub http: HttpClientConfig,
    /// Connection and idle-read timeout override.
    pub request_timeout: Option<Duration>,
}

impl Default for MoonshotConfig {
    fn default() -> Self {
        Self {
            base_url: DEFAULT_BASE_URL.into(),
            http: HttpClientConfig::default(),
            request_timeout: None,
        }
    }
}

impl MoonshotConfig {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            ..Self::default()
        }
    }
}

/// Moonshot provider using API-key Bearer authentication.
pub struct MoonshotProvider {
    inner: OpenAiCompatibleProvider,
}

impl MoonshotProvider {
    pub fn new(
        config: MoonshotConfig,
        credential: Option<ResolvedCredential>,
    ) -> Result<Self, ProviderError> {
        if let Some(credential) = &credential {
            if credential.kind() != CredentialKind::ApiKey {
                return Err(ProviderError::new(
                    ProviderErrorKind::Authentication,
                    "Moonshot requires an API key credential",
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
                provider_id: ProviderId::new("moonshot"),
                http,
                request_timeout: None,
            },
            credential,
        )?;
        Ok(Self { inner })
    }
}

#[async_trait]
impl ModelProvider for MoonshotProvider {
    fn id(&self) -> ProviderId {
        ProviderId::new("moonshot")
    }

    async fn list_models(
        &self,
        _credential: Option<&ResolvedCredential>,
    ) -> Result<Vec<ModelDefinition>, ProviderError> {
        Ok(builtin_models())
    }

    async fn stream(
        &self,
        request: CanonicalModelRequest,
        sink: &dyn ProviderEventSink,
        cancel: CancellationToken,
    ) -> Result<ModelResponseSummary, ProviderError> {
        self.inner
            .stream(request, sink, cancel)
            .await
            .map_err(normalize_moonshot_error)
    }
}

/// Built-in Moonshot/Kimi model catalog with conservative capability declarations.
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
        model("kimi-k2-thinking", "Kimi K2 Thinking", true),
        model("kimi-k2", "Kimi K2", true),
        model("moonshot-v1-128k", "Moonshot v1 128K", false),
    ]
}

fn normalize_moonshot_error(mut error: ProviderError) -> ProviderError {
    let message = error.message.to_ascii_lowercase();
    let kind = if message.contains("content_filter")
        || message.contains("content safety")
        || message.contains("high risk")
    {
        Some(ProviderErrorKind::ContentFiltered)
    } else if message.contains("rate_limit") || message.contains("rate limit") {
        Some(ProviderErrorKind::RateLimited)
    } else if message.contains("insufficient_balance")
        || message.contains("insufficient balance")
        || message.contains("quota_exceeded")
    {
        Some(ProviderErrorKind::QuotaExceeded)
    } else if message.contains("invalid_authentication") || message.contains("invalid api key") {
        Some(ProviderErrorKind::Authentication)
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
        let config = MoonshotConfig::default();
        assert_eq!(config.base_url, DEFAULT_BASE_URL);
        let provider = MoonshotProvider::new(config, None).expect("provider");
        assert_eq!(provider.id(), ProviderId::new("moonshot"));
    }

    #[test]
    fn catalog_marks_kimi_reasoning_model() {
        let models = builtin_models();
        assert!(models.iter().any(|model| {
            model.id == ModelId::new("kimi-k2-thinking") && model.capabilities.thinking
        }));
    }

    #[test]
    fn provider_error_codes_are_normalized() {
        let quota = normalize_moonshot_error(ProviderError::new(
            ProviderErrorKind::InvalidRequest,
            "HTTP 400: insufficient_balance",
        ));
        assert_eq!(quota.kind, ProviderErrorKind::QuotaExceeded);

        let filtered = normalize_moonshot_error(ProviderError::new(
            ProviderErrorKind::InvalidRequest,
            "HTTP 400: content_filter",
        ));
        assert_eq!(filtered.kind, ProviderErrorKind::ContentFiltered);
    }

    #[test]
    fn rejects_non_api_key_credentials() {
        let credential = ResolvedCredential::new(CredentialKind::OAuthBearer, "oauth");
        let error = MoonshotProvider::new(MoonshotConfig::default(), Some(credential))
            .err()
            .expect("OAuth credential must be rejected");
        assert_eq!(error.kind, ProviderErrorKind::Authentication);
    }
}
