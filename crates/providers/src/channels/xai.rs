//! xAI Grok OAuth adapter with model-declared Chat/Responses transport selection.
//!
//! OAuth acquisition/refresh is owned by `pawork-auth`; this adapter only consumes a resolved
//! bearer credential. SET-4 A3 起同时接受 OAuth bearer 与 API key（Bearer 用法相同，
//! 切换语义由宿主保证互斥替换）。SET-5 起 `list_models` 走远端
//! `GET {base}/language-models`（只保留 output_modalities 含 "text" 的模型）。

use std::sync::Arc;
use std::time::Duration;

use crate::net::http::{HttpClient, HttpClientConfig};
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
use serde_json::Value;

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
    models_http: HttpClient,
    models_url: String,
    credential: ResolvedCredential,
}

impl XaiProvider {
    pub fn new(
        config: XaiConfig,
        credential: Option<ResolvedCredential>,
    ) -> Result<Self, ProviderError> {
        let credential = require_bearer_credential(credential)?;
        let chat = OpenAiCompatibleProvider::new(
            OpenAiCompatibleConfig {
                base_url: config.base_url.clone(),
                provider_id: ProviderId::new(PROVIDER_ID),
                http: config.http.clone(),
                request_timeout: config.request_timeout,
            },
            Some(credential.clone()),
        )?;
        // SET-5：远端目录客户端（GET {base}/language-models），超时语义与 chat 对齐。
        let mut models_http_config = config.http.clone();
        if let Some(timeout) = config.request_timeout {
            models_http_config.timeout = Some(timeout);
        }
        let models_http = HttpClient::new(models_http_config)?;
        let models_url = format!("{}/language-models", config.base_url.trim_end_matches('/'));
        let mut responses = ResponsesTransportConfig::new(config.base_url, PROVIDER_ID);
        responses.http = config.http;
        responses.request_timeout = config.request_timeout;
        responses.wire = ResponsesWireOptions {
            store: None,
            include_encrypted_reasoning: true,
        };
        Ok(Self {
            chat,
            responses: ResponsesTransport::new(responses, credential.clone())?,
            models_http,
            models_url,
            credential,
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
        // 远端目录：GET {base}/language-models，OAuth bearer 与 API key 同为 Bearer。
        let auth_header = (
            "Authorization".to_string(),
            format!("Bearer {}", self.credential.expose_secret()),
        );
        let value = self
            .models_http
            .get_json_with_headers(
                &self.models_url,
                None,
                &[auth_header],
                CancellationToken::new(),
            )
            .await?;
        let entries = value
            .get("models")
            .and_then(Value::as_array)
            .ok_or_else(|| {
                ProviderError::new(
                    ProviderErrorKind::InvalidRequest,
                    "xAI language-models response must contain a models array",
                )
            })?;
        let mut definitions = Vec::new();
        for entry in entries {
            let Some(id) = entry.get("id").and_then(Value::as_str) else {
                continue;
            };
            // 只保留可输出文本的模型；modalities 缺失视为未证明，不入目录。
            let text_output = entry
                .get("output_modalities")
                .and_then(Value::as_array)
                .is_some_and(|modalities| modalities.iter().any(|m| m.as_str() == Some("text")));
            if !text_output {
                continue;
            }
            match builtin_models()
                .into_iter()
                .find(|definition| definition.id.as_str() == id)
            {
                Some(definition) => definitions.push(definition),
                None => definitions.push(unknown_text_model(id)),
            }
        }
        Ok(definitions)
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

fn require_bearer_credential(
    credential: Option<ResolvedCredential>,
) -> Result<ResolvedCredential, ProviderError> {
    let credential = credential.ok_or_else(|| {
        ProviderError::new(
            ProviderErrorKind::Authentication,
            "xAI Grok requires an OAuth bearer or API key credential",
        )
    })?;
    let accepted = matches!(
        credential.kind(),
        CredentialKind::OAuthBearer | CredentialKind::ApiKey
    ) && !credential.expose_secret().trim().is_empty();
    if !accepted {
        return Err(ProviderError::new(
            ProviderErrorKind::Authentication,
            "xAI Grok accepts only a non-empty OAuth bearer or API key credential",
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

/// 远端未知 ID 的保守默认：只声明端点已证明的文本输出，窗口/上限未知（0），
/// transport 退 Chat Completions 基线（与 `transport_for` 兜底一致）。
fn unknown_text_model(id: &str) -> ModelDefinition {
    ModelDefinition {
        id: ModelId::new(id),
        display_name: id.to_string(),
        context_window_tokens: 0,
        max_output_tokens: 0,
        capabilities: ModelCapabilities {
            text: true,
            transport: ModelTransport::ChatCompletions,
            ..ModelCapabilities::default()
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[test]
    fn bearer_credential_is_required_and_api_key_is_accepted() {
        for credential in [
            None,
            Some(ResolvedCredential::new(
                CredentialKind::SessionToken,
                "session",
            )),
            Some(ResolvedCredential::new(CredentialKind::ApiKey, "  ")),
        ] {
            assert_eq!(
                XaiProvider::new(XaiConfig::default(), credential)
                    .err()
                    .unwrap()
                    .kind,
                ProviderErrorKind::Authentication
            );
        }
        let provider = XaiProvider::new(
            XaiConfig::default(),
            Some(ResolvedCredential::new(CredentialKind::ApiKey, "sk-test")),
        )
        .expect("API key credential must construct");
        assert_eq!(provider.id().as_str(), "xai");
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

    #[tokio::test]
    async fn remote_language_models_parse_filter_and_merge_builtin() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/language-models"))
            .and(header("authorization", "Bearer sk-test"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "models": [
                    {
                        "id": "grok-4",
                        "input_modalities": ["text", "image"],
                        "output_modalities": ["text"]
                    },
                    {
                        "id": "grok-image-only",
                        "input_modalities": ["text"],
                        "output_modalities": ["image"]
                    },
                    {
                        "id": "grok-future",
                        "input_modalities": ["text"],
                        "output_modalities": ["text"]
                    }
                ]
            })))
            .expect(1)
            .mount(&server)
            .await;

        let mut config = XaiConfig::new(server.uri());
        config.http = crate::net::http::HttpClientConfig::builder()
            .disable_system_proxy()
            .build();
        let provider = XaiProvider::new(
            config,
            Some(ResolvedCredential::new(CredentialKind::ApiKey, "sk-test")),
        )
        .expect("construct");
        let models = provider.list_models(None).await.expect("remote models");

        let ids: Vec<&str> = models.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["grok-4", "grok-future"]);
        let grok4 = &models[0];
        assert_eq!(grok4.display_name, "Grok 4");
        assert_eq!(grok4.context_window_tokens, 256_000);
        assert_eq!(grok4.capabilities.transport, ModelTransport::Responses);
        let future = &models[1];
        assert_eq!(future.display_name, "grok-future");
        assert_eq!(future.context_window_tokens, 0);
        assert_eq!(future.max_output_tokens, 0);
        assert_eq!(
            future.capabilities.transport,
            ModelTransport::ChatCompletions
        );
        server.verify().await;
    }

    #[tokio::test]
    async fn remote_language_models_failure_returns_err() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/language-models"))
            .respond_with(ResponseTemplate::new(500))
            .expect(1)
            .mount(&server)
            .await;

        let mut config = XaiConfig::new(server.uri());
        config.http = crate::net::http::HttpClientConfig::builder()
            .disable_system_proxy()
            .build();
        let provider = XaiProvider::new(
            config,
            Some(ResolvedCredential::new(
                CredentialKind::OAuthBearer,
                "oauth-token",
            )),
        )
        .expect("construct");
        let error = provider
            .list_models(None)
            .await
            .err()
            .expect("remote failure must error");
        assert_eq!(error.kind, ProviderErrorKind::ProviderUnavailable);
    }
}
