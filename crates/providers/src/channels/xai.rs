//! xAI Grok OAuth adapter with model-declared Chat/Responses transport selection.
//!
//! OAuth acquisition/refresh is owned by `pawork-auth`; this adapter only consumes a resolved
//! bearer credential. SET-4 A3 起同时接受 OAuth bearer 与 API key（Bearer 用法相同，
//! 切换语义由宿主保证互斥替换）。订阅走 CLI proxy 的 `/models`；API key
//! 走 API 的 `/language-models`（只保留 output_modalities 含 "text" 的模型）。

use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};
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
const SUBSCRIPTION_BASE_URL: &str = "https://cli-chat-proxy.grok.com/v1";
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
    discovered_transports: RwLock<BTreeMap<ModelId, ModelTransport>>,
}

impl XaiProvider {
    pub fn new(
        mut config: XaiConfig,
        credential: Option<ResolvedCredential>,
    ) -> Result<Self, ProviderError> {
        let credential = super::require_bearer_credential("xAI Grok", credential)?;
        let subscription = credential.kind() == CredentialKind::OAuthBearer;
        if subscription {
            // 宿主传入注册表默认值时，按凭证选择订阅端点；自定义端点保留。
            if config.base_url.trim_end_matches('/') == DEFAULT_BASE_URL {
                config.base_url = SUBSCRIPTION_BASE_URL.into();
            }
            config
                .http
                .extra_headers
                .push(("X-XAI-Token-Auth".into(), "xai-grok-cli".into()));
        }
        let mut chat = OpenAiCompatibleProvider::new(
            OpenAiCompatibleConfig {
                base_url: config.base_url.clone(),
                provider_id: ProviderId::new(PROVIDER_ID),
                http: config.http.clone(),
                request_timeout: config.request_timeout,
            },
            Some(credential.clone()),
        )?;
        // 目录与推理共用端点、认证头与超时配置。
        let mut models_http_config = config.http.clone();
        if let Some(timeout) = config.request_timeout {
            models_http_config.timeout = Some(timeout);
        }
        let models_http = HttpClient::new(models_http_config)?;
        let models_path = if subscription {
            "models"
        } else {
            "language-models"
        };
        let models_url = format!("{}/{models_path}", config.base_url.trim_end_matches('/'));
        let mut responses = ResponsesTransportConfig::new(config.base_url, PROVIDER_ID);
        responses.http = config.http;
        responses.request_timeout = config.request_timeout;
        responses.wire = ResponsesWireOptions {
            store: None,
            include_encrypted_reasoning: true,
            hosted_web_search: true,
        };
        let mut responses = ResponsesTransport::new(responses, credential.clone())?;
        if subscription {
            chat = chat.with_model_header("x-grok-model-override");
            responses = responses.with_model_header("x-grok-model-override");
        }
        Ok(Self {
            chat,
            responses,
            models_http,
            models_url,
            credential,
            discovered_transports: RwLock::new(BTreeMap::new()),
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
        let subscription = self.credential.kind() == CredentialKind::OAuthBearer;
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
        let entries =
            crate::provider::catalog_entries(&value, if subscription { "data" } else { "models" })?;
        let builtin = builtin_models();
        let mut definitions = Vec::new();
        for entry in entries {
            let id_key = if subscription && entry.get("model").is_some() {
                "model"
            } else if subscription && entry.get("modelId").is_some() {
                "modelId"
            } else {
                "id"
            };
            let id = crate::provider::catalog_model_id(entry, id_key)?;
            // 只保留可输出文本的模型；modalities 缺失视为未证明，不入目录。
            let text_output = entry
                .get("output_modalities")
                .and_then(Value::as_array)
                .is_some_and(|modalities| modalities.iter().any(|m| m.as_str() == Some("text")));
            if !subscription && !text_output {
                continue;
            }
            let mut definition = builtin
                .iter()
                .find(|definition| {
                    definition.id.as_str() == id
                        || entry
                            .get("aliases")
                            .and_then(Value::as_array)
                            .is_some_and(|aliases| {
                                aliases
                                    .iter()
                                    .any(|alias| alias.as_str() == Some(definition.id.as_str()))
                            })
                })
                .cloned()
                .unwrap_or_else(|| unknown_text_model(id));
            definition.id = ModelId::new(id);
            // canonical ID 与 stream 使用相同路由；别名只补能力，不改变实际请求路径。
            definition.capabilities.transport = Self::transport_for(&definition.id);
            if subscription {
                definition.capabilities.transport = match entry
                    .get("apiBackend")
                    .or_else(|| entry.get("api_backend"))
                    .and_then(Value::as_str)
                {
                    Some("responses") => ModelTransport::Responses,
                    Some("chat_completions") | None => ModelTransport::ChatCompletions,
                    Some(_) => continue,
                };
                if let Some(name) = entry.get("name").and_then(Value::as_str) {
                    definition.display_name = name.into();
                }
                if let Some(context) = entry
                    .get("contextWindow")
                    .or_else(|| entry.get("context_window"))
                    .or_else(|| entry.pointer("/_meta/contextWindow"))
                    .or_else(|| entry.pointer("/_meta/totalContextTokens"))
                    .and_then(Value::as_u64)
                {
                    definition.context_window_tokens = context;
                }
                if let Some(max) = entry
                    .get("maxCompletionTokens")
                    .or_else(|| entry.get("max_completion_tokens"))
                    .and_then(Value::as_u64)
                {
                    definition.max_output_tokens = max;
                }
            }
            // 搜索声明随 transport 走（2026-09-22 调研）：xAI Responses API
            // 官方提供 server 端 web_search 工具（docs.x.ai/developers/tools/
            // web-search），订阅 CLI proxy 亦经 /responses 调 web_search
            //（xai-org/grok-build xai-grok-tools）；Chat 路径未接线，清除标签。
            if definition.capabilities.transport == ModelTransport::Responses {
                definition
                    .capabilities
                    .hosted_tool_tags
                    .insert(pawork_domain::ToolCapabilityTag::WebSearch);
            } else {
                definition.capabilities.hosted_tool_tags.clear();
            }
            if let Some(modalities) = entry.get("input_modalities").and_then(Value::as_array) {
                definition.capabilities.image_input = modalities
                    .iter()
                    .any(|modality| modality.as_str() == Some("image"));
            } else {
                // VISION-2：远端未声明模态时按官方模型页默认表回填。
                crate::registry::apply_default_image_input(&mut definition);
            }
            if let Some(context) = entry.get("context_length").and_then(Value::as_u64) {
                definition.context_window_tokens = context;
            }
            // ADR-063：远端未声明推理强度时按默认表回填（订阅目录
            // reasoning_efforts 声明未来接入时优先于默认表）。
            crate::registry::apply_default_supported_efforts(&mut definition);
            definitions.push(definition);
        }
        *self
            .discovered_transports
            .write()
            .expect("xAI transports lock poisoned") = definitions
            .iter()
            .map(|model| (model.id.clone(), model.capabilities.transport))
            .collect();
        Ok(definitions)
    }

    async fn stream(
        &self,
        request: &CanonicalModelRequest,
        sink: &dyn ProviderEventSink,
        cancel: CancellationToken,
    ) -> Result<ModelResponseSummary, ProviderError> {
        let transport = self
            .discovered_transports
            .read()
            .expect("xAI transports lock poisoned")
            .get(&request.model)
            .copied()
            .unwrap_or_else(|| Self::transport_for(&request.model));
        match transport {
            ModelTransport::Responses => self
                .responses
                .stream(request, sink, cancel)
                .await
                .map_err(|error| normalize_vendor_error(PROVIDER_ID, error)),
            ModelTransport::ChatCompletions => self
                .chat
                .stream(request, sink, cancel)
                .await
                .map_err(|error| normalize_vendor_error(PROVIDER_ID, error)),
            ModelTransport::Messages => Err(ProviderError::new(
                ProviderErrorKind::InvalidRequest,
                "xAI adapter cannot route a Messages-only model",
            )),
        }
    }
}

/// 已知 id 的 transport / 能力提示，不是 GUI 选择目录。
/// 可选模型只来自 `list_models` 的远端目录。
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
                // 当前 hosted search 仅由 Responses API 承接。
                hosted_tool_tags: (transport == ModelTransport::Responses)
                    .then_some(pawork_domain::ToolCapabilityTag::WebSearch)
                    .into_iter()
                    .collect(),
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
    use pawork_domain::CredentialKind;
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
        assert_eq!(provider.models_url, "https://api.x.ai/v1/language-models");
        let subscription = XaiProvider::new(
            XaiConfig::new(format!("{DEFAULT_BASE_URL}/")),
            Some(ResolvedCredential::new(
                CredentialKind::OAuthBearer,
                "oauth-token",
            )),
        )
        .unwrap();
        assert_eq!(
            subscription.models_url,
            "https://cli-chat-proxy.grok.com/v1/models"
        );
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
                        "context_length": 262144,
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
                        "input_modalities": ["text", "image"],
                        "output_modalities": ["text"]
                    },
                    {
                        "id": "grok-3-current",
                        "aliases": ["grok-3"],
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
        assert_eq!(ids, ["grok-4", "grok-future", "grok-3-current"]);
        let grok4 = &models[0];
        assert_eq!(grok4.display_name, "Grok 4");
        assert_eq!(grok4.context_window_tokens, 262_144);
        assert_eq!(grok4.capabilities.transport, ModelTransport::Responses);
        let future = &models[1];
        assert_eq!(future.display_name, "grok-future");
        assert_eq!(future.context_window_tokens, 0);
        assert_eq!(future.max_output_tokens, 0);
        assert!(future.capabilities.image_input);
        assert!(!future.capabilities.tool_calls);
        assert_eq!(
            future.capabilities.transport,
            ModelTransport::ChatCompletions
        );
        let alias = &models[2];
        assert_eq!(alias.context_window_tokens, 131_072);
        assert!(alias.capabilities.tool_calls);
        assert!(!alias.capabilities.image_input);
        assert_eq!(
            alias.capabilities.transport,
            ModelTransport::ChatCompletions
        );
        assert!(!server.received_requests().await.unwrap()[0]
            .headers
            .contains_key("x-xai-token-auth"));
        server.verify().await;
    }

    #[tokio::test]
    async fn remote_subscription_models_forbidden_returns_err() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/models"))
            .respond_with(ResponseTemplate::new(403))
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
        assert_eq!(error.kind, ProviderErrorKind::Authorization);
        server.verify().await;
    }
}
