//! xAI Grok [`ModelProvider`](provider_api::ModelProvider) implementation.

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

/// xAI Chat Completions adapter configuration.
#[derive(Clone, Debug)]
pub struct XaiConfig {
    /// Base URL, defaulting to `https://api.x.ai/v1`.
    pub base_url: String,
    /// HTTP client configuration.
    pub http: HttpClientConfig,
    /// Connection and idle-read timeout override.
    pub request_timeout: Option<Duration>,
}

impl Default for XaiConfig {
    fn default() -> Self {
        Self {
            base_url: DEFAULT_BASE_URL.into(),
            http: HttpClientConfig::default(),
            request_timeout: None,
        }
    }
}

impl XaiConfig {
    /// Uses a custom OpenAI-compatible base URL while retaining provider id `xai`.
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            ..Self::default()
        }
    }
}

/// xAI Grok provider using the Chat Completions transport.
pub struct XaiProvider {
    inner: OpenAiCompatibleProvider,
}

impl XaiProvider {
    /// Builds the adapter with either an API key or an OAuth bearer access token.
    pub fn new(
        config: XaiConfig,
        credential: Option<ResolvedCredential>,
    ) -> Result<Self, ProviderError> {
        if let Some(credential) = &credential {
            if !matches!(
                credential.kind(),
                CredentialKind::ApiKey | CredentialKind::OAuthBearer
            ) {
                return Err(ProviderError::new(
                    ProviderErrorKind::Authentication,
                    "xAI requires an API key or OAuth bearer credential",
                ));
            }
        }

        let compatible = OpenAiCompatibleConfig {
            base_url: config.base_url,
            provider_id: ProviderId::new("xai"),
            http: config.http,
            request_timeout: config.request_timeout,
        };
        Ok(Self {
            inner: OpenAiCompatibleProvider::new(compatible, credential)?,
        })
    }
}

#[async_trait]
impl ModelProvider for XaiProvider {
    fn id(&self) -> ProviderId {
        ProviderId::new("xai")
    }

    async fn list_models(
        &self,
        credential: Option<&ResolvedCredential>,
    ) -> Result<Vec<ModelDefinition>, ProviderError> {
        self.inner.list_models(credential).await
    }

    async fn stream(
        &self,
        request: CanonicalModelRequest,
        sink: &dyn ProviderEventSink,
        cancel: CancellationToken,
    ) -> Result<ModelResponseSummary, ProviderError> {
        self.inner.stream(request, sink, cancel).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_xai_specific() {
        let config = XaiConfig::default();
        assert_eq!(config.base_url, DEFAULT_BASE_URL);
    }

    #[test]
    fn rejects_session_credentials() {
        let credential = ResolvedCredential::new(CredentialKind::SessionToken, "session");
        let error = XaiProvider::new(XaiConfig::default(), Some(credential))
            .err()
            .expect("session credential must be rejected");
        assert_eq!(error.kind, ProviderErrorKind::Authentication);
    }
}
