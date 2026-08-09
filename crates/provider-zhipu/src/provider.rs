//! Zhipu GLM [`ModelProvider`](provider_api::ModelProvider) implementation.

use std::time::Duration;

use agent_domain::{CancellationToken, ProviderId};
use async_trait::async_trait;
use provider_api::{
    CanonicalModelRequest, CredentialKind, ModelDefinition, ModelProvider, ModelResponseSummary,
    ProviderError, ProviderErrorKind, ProviderEventSink, ResolvedCredential,
};
use provider_openai_compatible::{OpenAiCompatibleConfig, OpenAiCompatibleProvider};
use provider_runtime::http::HttpClientConfig;

use crate::DEFAULT_BASE_URL;

/// Zhipu BigModel v4 adapter configuration.
#[derive(Clone, Debug)]
pub struct ZhipuConfig {
    /// Base URL, defaulting to the BigModel OpenAI-compatible v4 endpoint.
    pub base_url: String,
    /// HTTP client configuration.
    pub http: HttpClientConfig,
    /// Connection and idle-read timeout override.
    pub request_timeout: Option<Duration>,
}

impl Default for ZhipuConfig {
    fn default() -> Self {
        Self {
            base_url: DEFAULT_BASE_URL.into(),
            http: HttpClientConfig::default(),
            request_timeout: None,
        }
    }
}

impl ZhipuConfig {
    /// Uses a custom BigModel-compatible base URL while retaining provider id `zhipu`.
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            ..Self::default()
        }
    }
}

/// Zhipu GLM provider using the OpenAI-compatible Chat Completions transport.
pub struct ZhipuProvider {
    inner: OpenAiCompatibleProvider,
}

impl ZhipuProvider {
    /// Builds the adapter. BigModel direct access accepts API keys as Bearer tokens.
    pub fn new(
        config: ZhipuConfig,
        credential: Option<ResolvedCredential>,
    ) -> Result<Self, ProviderError> {
        if let Some(credential) = &credential {
            if credential.kind() != CredentialKind::ApiKey {
                return Err(ProviderError::new(
                    ProviderErrorKind::Authentication,
                    "Zhipu requires an API key credential",
                ));
            }
        }

        let compatible = OpenAiCompatibleConfig {
            base_url: config.base_url,
            provider_id: ProviderId::new("zhipu"),
            http: config.http,
            request_timeout: config.request_timeout,
        };
        Ok(Self {
            inner: OpenAiCompatibleProvider::new(compatible, credential)?,
        })
    }
}

#[async_trait]
impl ModelProvider for ZhipuProvider {
    fn id(&self) -> ProviderId {
        ProviderId::new("zhipu")
    }

    async fn list_models(
        &self,
        credential: Option<&ResolvedCredential>,
    ) -> Result<Vec<ModelDefinition>, ProviderError> {
        self.inner
            .list_models(credential)
            .await
            .map_err(normalize_zhipu_error)
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
            .map_err(normalize_zhipu_error)
    }
}

fn normalize_zhipu_error(mut error: ProviderError) -> ProviderError {
    if error.kind != ProviderErrorKind::InvalidRequest {
        return error;
    }

    let message = error.message.to_ascii_lowercase();
    if message.contains("1113")
        || message.contains("insufficient balance")
        || message.contains("余额不足")
    {
        error.kind = ProviderErrorKind::QuotaExceeded;
        error.retryable = false;
    } else if message.contains("1301")
        || message.contains("content filtered")
        || message.contains("content moderation")
        || message.contains("敏感")
        || message.contains("审核")
    {
        error.kind = ProviderErrorKind::ContentFiltered;
        error.retryable = false;
    }
    error
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_zhipu_specific() {
        let config = ZhipuConfig::default();
        assert_eq!(config.base_url, DEFAULT_BASE_URL);
    }

    #[test]
    fn rejects_non_api_key_credentials() {
        let credential = ResolvedCredential::new(CredentialKind::OAuthBearer, "oauth");
        let error = ZhipuProvider::new(ZhipuConfig::default(), Some(credential))
            .err()
            .expect("OAuth credential must be rejected");
        assert_eq!(error.kind, ProviderErrorKind::Authentication);
    }
}
