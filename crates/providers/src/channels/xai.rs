//! xAI Grok OAuth adapter with model-declared Chat/Responses transport selection.
//!
//! OAuth acquisition/refresh is owned by `pawork-auth`; this adapter only consumes a resolved
//! bearer token. API-key auth is intentionally outside the initial product scope requested for S6.

use std::sync::Arc;
use std::time::Duration;

use crate::net::http::HttpClientConfig;
use crate::ReasoningProtector;
use async_trait::async_trait;
use pawork_domain::{CancellationToken, ModelId, ProviderId};
use pawork_domain::{
    CanonicalModelRequest, CredentialKind, ModelCapabilities, ModelDefinition, ModelProvider,
    ModelResponseSummary, ModelTransport, ProviderError, ProviderErrorKind, ProviderEventSink,
    ResolvedCredential,
};

use crate::normalize_vendor_error;
use crate::provider::{OpenAiCompatibleConfig, OpenAiCompatibleProvider};
use crate::responses::{ResponsesTransport, ResponsesTransportConfig, ResponsesWireOptions};

pub const DEFAULT_BASE_URL: &str = "https://api.x.ai/v1";
pub const PROVIDER_ID: &str = "xai";

#[derive(Clone, Debug)]
pub struct XaiConfig {
    pub base_url: String,
    pub http: HttpClientConfig,
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
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            ..Self::default()
        }
    }
}

pub struct XaiProvider {
    chat: OpenAiCompatibleProvider,
    responses: ResponsesTransport,
}

impl XaiProvider {
    pub fn new(
        config: XaiConfig,
        credential: Option<ResolvedCredential>,
    ) -> Result<Self, ProviderError> {
        let credential = require_oauth(credential)?;
        let chat = OpenAiCompatibleProvider::new(
            OpenAiCompatibleConfig {
                base_url: config.base_url.clone(),
                provider_id: ProviderId::new(PROVIDER_ID),
                http: config.http.clone(),
                request_timeout: config.request_timeout,
            },
            Some(credential.clone()),
        )?;
        let mut responses = ResponsesTransportConfig::new(config.base_url, PROVIDER_ID);
        responses.http = config.http;
        responses.request_timeout = config.request_timeout;
        responses.wire = ResponsesWireOptions {
            store: None,
            include_encrypted_reasoning: true,
        };
        Ok(Self {
            chat,
            responses: ResponsesTransport::new(responses, credential)?,
        })
    }

    pub fn with_reasoning_protector(mut self, protector: Arc<dyn ReasoningProtector>) -> Self {
        self.responses = self.responses.with_reasoning_protector(protector);
        self
    }

    fn transport_for(model: &ModelId) -> ModelTransport {
        builtin_models()
            .into_iter()
            .find(|definition| definition.id == *model)
            .map(|definition| definition.capabilities.transport)
            .unwrap_or(ModelTransport::ChatCompletions)
    }
}

#[async_trait]
impl ModelProvider for XaiProvider {
    fn id(&self) -> ProviderId {
        ProviderId::new(PROVIDER_ID)
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
        match Self::transport_for(&request.model) {
            ModelTransport::Responses => self
                .responses
                .stream(request, sink, cancel)
                .await
                .map_err(|error| normalize_vendor_error(PROVIDER_ID, error)),
            ModelTransport::ChatCompletions => {
                if !request.hosted_tools.is_empty() || !request.extensions.is_empty() {
                    return Err(ProviderError::new(
                        ProviderErrorKind::InvalidRequest,
                        "xAI Chat Completions models do not declare provider-hosted tools",
                    ));
                }
                self.chat
                    .stream(request, sink, cancel)
                    .await
                    .map_err(|error| normalize_vendor_error(PROVIDER_ID, error))
            }
            ModelTransport::Messages => Err(ProviderError::new(
                ProviderErrorKind::InvalidRequest,
                "xAI adapter cannot route a Messages-only model",
            )),
        }
    }
}

fn require_oauth(
    credential: Option<ResolvedCredential>,
) -> Result<ResolvedCredential, ProviderError> {
    let credential = credential.ok_or_else(|| {
        ProviderError::new(
            ProviderErrorKind::Authentication,
            "xAI Grok requires an OAuth bearer credential",
        )
    })?;
    if credential.kind() != CredentialKind::OAuthBearer
        || credential.expose_secret().trim().is_empty()
    {
        return Err(ProviderError::new(
            ProviderErrorKind::Authentication,
            "xAI Grok initial adapter accepts only a non-empty OAuth bearer credential",
        ));
    }
    Ok(credential)
}

pub fn builtin_models() -> Vec<ModelDefinition> {
    fn model(
        id: &str,
        display_name: &str,
        context_window_tokens: u64,
        image_input: bool,
        thinking: bool,
        transport: ModelTransport,
    ) -> ModelDefinition {
        ModelDefinition {
            id: ModelId::new(id),
            display_name: display_name.into(),
            context_window_tokens,
            max_output_tokens: 32_768,
            capabilities: ModelCapabilities {
                text: true,
                image_input,
                tool_calls: true,
                parallel_tool_calls: true,
                thinking,
                structured_output: true,
                transport,
                ..ModelCapabilities::default()
            },
        }
    }

    vec![
        model(
            "grok-4",
            "Grok 4",
            256_000,
            true,
            true,
            ModelTransport::Responses,
        ),
        model(
            "grok-4-fast",
            "Grok 4 Fast",
            128_000,
            true,
            true,
            ModelTransport::Responses,
        ),
        model(
            "grok-3",
            "Grok 3",
            131_072,
            true,
            false,
            ModelTransport::ChatCompletions,
        ),
        model(
            "grok-2",
            "Grok 2",
            131_072,
            false,
            false,
            ModelTransport::ChatCompletions,
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oauth_is_required_and_api_key_is_deferred() {
        for credential in [
            None,
            Some(ResolvedCredential::new(CredentialKind::ApiKey, "sk-test")),
            Some(ResolvedCredential::new(
                CredentialKind::SessionToken,
                "session",
            )),
        ] {
            assert_eq!(
                XaiProvider::new(XaiConfig::default(), credential)
                    .err()
                    .unwrap()
                    .kind,
                ProviderErrorKind::Authentication
            );
        }
    }

    #[test]
    fn fixed_credential_header_is_rejected() {
        let mut config = XaiConfig::default();
        config
            .http
            .extra_headers
            .push(("Authorization".into(), "Bearer attacker".into()));
        let credential = ResolvedCredential::new(CredentialKind::OAuthBearer, "oauth-token");
        let error = XaiProvider::new(config, Some(credential))
            .err()
            .expect("duplicate credential header must fail");
        assert_eq!(error.kind, ProviderErrorKind::InvalidRequest);
    }

    #[test]
    fn model_data_drives_transport() {
        assert_eq!(
            XaiProvider::transport_for(&ModelId::new("grok-4")),
            ModelTransport::Responses
        );
        assert_eq!(
            XaiProvider::transport_for(&ModelId::new("grok-3")),
            ModelTransport::ChatCompletions
        );
        assert_eq!(
            XaiProvider::transport_for(&ModelId::new("future-model")),
            ModelTransport::ChatCompletions
        );
    }
}
