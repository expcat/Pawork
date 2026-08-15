//! ChatGPT subscription OAuth adapter.
//!
//! OAuth login/refresh belongs to `pawork-auth` (S6 later wave). This adapter consumes the
//! resolved access token plus ChatGPT account id and talks to the Responses backend; it never
//! embeds an OAuth client id/secret or persists token material.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use pawork_api::{
    CanonicalModelRequest, CredentialKind, ModelCapabilities, ModelDefinition, ModelProvider,
    ModelResponseSummary, ModelTransport, ProviderError, ProviderErrorKind, ProviderEventSink,
    ResolvedCredential,
};
use pawork_domain::{CancellationToken, ModelId, ProviderId};
use pawork_net::http::HttpClientConfig;
use pawork_provider_core::ReasoningProtector;
use serde_json::Value;

use crate::responses::{ResponsesTransport, ResponsesTransportConfig, ResponsesWireOptions};

pub const DEFAULT_BASE_URL: &str = "https://chatgpt.com/backend-api/codex";
pub const PROVIDER_ID: &str = "chatgpt";

#[derive(Clone, Debug)]
pub struct ChatGptConfig {
    pub base_url: String,
    /// OAuth token 所属 ChatGPT account；只作为请求路由头，不写入 canonical 事件。
    pub account_id: Option<String>,
    pub client_version: String,
    pub http: HttpClientConfig,
    pub request_timeout: Option<Duration>,
}

impl Default for ChatGptConfig {
    fn default() -> Self {
        Self {
            base_url: DEFAULT_BASE_URL.into(),
            account_id: None,
            client_version: env!("CARGO_PKG_VERSION").into(),
            http: HttpClientConfig::default(),
            request_timeout: None,
        }
    }
}

impl ChatGptConfig {
    pub fn new(account_id: impl Into<String>) -> Self {
        Self {
            account_id: Some(account_id.into()),
            ..Self::default()
        }
    }

    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }
}

pub struct ChatGptProvider {
    transport: ResponsesTransport,
    models_url: String,
}

impl ChatGptProvider {
    pub fn new(
        config: ChatGptConfig,
        credential: Option<ResolvedCredential>,
    ) -> Result<Self, ProviderError> {
        let credential = require_oauth(credential)?;
        let account_id = config
            .account_id
            .as_deref()
            .map(str::trim)
            .filter(|account_id| !account_id.is_empty())
            .ok_or_else(|| {
                ProviderError::new(
                    ProviderErrorKind::Authentication,
                    "ChatGPT OAuth requires a ChatGPT account id",
                )
            })?;
        if !config
            .client_version
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || ".-_".contains(character))
        {
            return Err(ProviderError::new(
                ProviderErrorKind::InvalidRequest,
                "ChatGPT client_version contains unsupported characters",
            ));
        }

        let models_url = format!(
            "{}/models?client_version={}",
            config.base_url.trim_end_matches('/'),
            config.client_version
        );
        let mut transport = ResponsesTransportConfig::new(&config.base_url, PROVIDER_ID);
        transport.http = config.http;
        transport.request_timeout = config.request_timeout;
        transport.request_headers = vec![
            ("ChatGPT-Account-Id".into(), account_id.to_string()),
            ("originator".into(), "pawork".into()),
        ];
        transport.wire = ResponsesWireOptions {
            store: Some(false),
            include_encrypted_reasoning: true,
        };
        Ok(Self {
            transport: ResponsesTransport::new(transport, credential)?,
            models_url,
        })
    }

    pub fn with_reasoning_protector(mut self, protector: Arc<dyn ReasoningProtector>) -> Self {
        self.transport = self.transport.with_reasoning_protector(protector);
        self
    }
}

#[async_trait]
impl ModelProvider for ChatGptProvider {
    fn id(&self) -> ProviderId {
        ProviderId::new(PROVIDER_ID)
    }

    async fn list_models(
        &self,
        _credential: Option<&ResolvedCredential>,
    ) -> Result<Vec<ModelDefinition>, ProviderError> {
        let value = self.transport.get_json(&self.models_url).await?;
        Ok(chatgpt_models(&value))
    }

    async fn stream(
        &self,
        request: CanonicalModelRequest,
        sink: &dyn ProviderEventSink,
        cancel: CancellationToken,
    ) -> Result<ModelResponseSummary, ProviderError> {
        self.transport.stream(request, sink, cancel).await
    }
}

fn require_oauth(
    credential: Option<ResolvedCredential>,
) -> Result<ResolvedCredential, ProviderError> {
    let credential = credential.ok_or_else(|| {
        ProviderError::new(
            ProviderErrorKind::Authentication,
            "ChatGPT requires an OAuth bearer credential",
        )
    })?;
    if credential.kind() != CredentialKind::OAuthBearer
        || credential.expose_secret().trim().is_empty()
    {
        return Err(ProviderError::new(
            ProviderErrorKind::Authentication,
            "ChatGPT accepts only a non-empty OAuth bearer credential",
        ));
    }
    Ok(credential)
}

fn chatgpt_models(value: &Value) -> Vec<ModelDefinition> {
    value
        .get("models")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|model| {
            let id = model
                .get("slug")
                .or_else(|| model.get("id"))
                .and_then(Value::as_str)?;
            let input_modalities = model
                .get("input_modalities")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            Some(ModelDefinition {
                id: ModelId::new(id),
                display_name: model
                    .get("display_name")
                    .and_then(Value::as_str)
                    .unwrap_or(id)
                    .to_string(),
                context_window_tokens: model
                    .get("context_window")
                    .or_else(|| model.get("max_context_window"))
                    .and_then(Value::as_u64)
                    .unwrap_or(0),
                max_output_tokens: model
                    .get("max_output_tokens")
                    .and_then(Value::as_u64)
                    .unwrap_or(0),
                capabilities: ModelCapabilities {
                    text: true,
                    image_input: input_modalities
                        .iter()
                        .any(|modality| modality.as_str() == Some("image")),
                    tool_calls: true,
                    parallel_tool_calls: model
                        .get("supports_parallel_tool_calls")
                        .and_then(Value::as_bool)
                        .unwrap_or(true),
                    thinking: model.get("supported_reasoning_levels").is_some(),
                    transport: ModelTransport::Responses,
                    ..ModelCapabilities::default()
                },
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrong_credentials_and_missing_account_fail_closed() {
        let api_key = ResolvedCredential::new(CredentialKind::ApiKey, "sk-test");
        assert_eq!(
            ChatGptProvider::new(ChatGptConfig::new("acct"), Some(api_key))
                .err()
                .unwrap()
                .kind,
            ProviderErrorKind::Authentication
        );
        let oauth = ResolvedCredential::new(CredentialKind::OAuthBearer, "token");
        assert_eq!(
            ChatGptProvider::new(ChatGptConfig::default(), Some(oauth))
                .err()
                .unwrap()
                .kind,
            ProviderErrorKind::Authentication
        );
    }

    #[test]
    fn model_payload_is_responses_capable() {
        let models = chatgpt_models(&serde_json::json!({"models": [{
            "slug": "codex-test",
            "display_name": "Codex Test",
            "context_window": 200000,
            "supports_parallel_tool_calls": true,
            "supported_reasoning_levels": ["medium"],
            "input_modalities": ["text", "image"]
        }]}));
        assert_eq!(models[0].capabilities.transport, ModelTransport::Responses);
        assert!(models[0].capabilities.image_input);
        assert!(models[0].capabilities.thinking);
    }
}
