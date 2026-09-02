//! Kimi Code OAuth adapter（SET-4 A2）：只消费 OAuth bearer，只走
//! OpenAI-compatible Chat Completions（api.kimi.com/coding/v1）。
//!
//! OAuth 获取 / 刷新归 pawork-auth（Device Flow，端点在 registry 预设）；
//! 本 adapter 不接受 API key 形态凭证。SET-5 起 `list_models` 走远端
//! `GET {base}/models`（与官方 kimi-cli 同端点，OpenAI 风格 data[] 解析；
//! 已知 id 沿用 builtin 元数据，未知 id 只给保守默认；形状不符即 Err）。

use std::time::Duration;

use crate::net::http::{HttpClient, HttpClientConfig};
use async_trait::async_trait;
use pawork_domain::{CancellationToken, ModelId, ProviderId};
use pawork_domain::{
    CanonicalModelRequest, CredentialKind, ModelCapabilities, ModelDefinition, ModelProvider,
    ModelResponseSummary, ModelTransport, ProviderError, ProviderErrorKind, ProviderEventSink,
    ResolvedCredential,
};

use crate::normalize_vendor_error;
use crate::provider::{OpenAiCompatibleConfig, OpenAiCompatibleProvider};
use serde_json::Value;

pub const DEFAULT_BASE_URL: &str = "https://api.kimi.com/coding/v1";
pub const PROVIDER_ID: &str = "kimi-code";

#[derive(Clone, Debug)]
pub struct KimiCodeConfig {
    pub base_url: String,
    pub http: HttpClientConfig,
    pub request_timeout: Option<Duration>,
}

impl Default for KimiCodeConfig {
    fn default() -> Self {
        Self {
            base_url: DEFAULT_BASE_URL.into(),
            http: HttpClientConfig::default(),
            request_timeout: None,
        }
    }
}

impl KimiCodeConfig {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            ..Self::default()
        }
    }
}

pub struct KimiCodeProvider {
    chat: OpenAiCompatibleProvider,
    models_http: HttpClient,
    models_url: String,
    credential: ResolvedCredential,
}

impl KimiCodeProvider {
    pub fn new(
        config: KimiCodeConfig,
        credential: Option<ResolvedCredential>,
    ) -> Result<Self, ProviderError> {
        let credential = require_oauth(credential)?;
        // SET-5：远端目录客户端（GET {base}/models），超时语义与 chat 对齐。
        let mut models_http_config = config.http.clone();
        if let Some(timeout) = config.request_timeout {
            models_http_config.timeout = Some(timeout);
        }
        let models_http = HttpClient::new(models_http_config)?;
        let models_url = format!("{}/models", config.base_url.trim_end_matches('/'));
        let chat = OpenAiCompatibleProvider::new(
            OpenAiCompatibleConfig {
                base_url: config.base_url,
                provider_id: ProviderId::new(PROVIDER_ID),
                http: config.http,
                request_timeout: config.request_timeout,
            },
            Some(credential.clone()),
        )?;
        Ok(Self {
            chat,
            models_http,
            models_url,
            credential,
        })
    }
}

#[async_trait]
impl ModelProvider for KimiCodeProvider {
    fn id(&self) -> ProviderId {
        ProviderId::new(PROVIDER_ID)
    }

    async fn list_models(
        &self,
        _credential: Option<&ResolvedCredential>,
    ) -> Result<Vec<ModelDefinition>, ProviderError> {
        // 远端目录：OAuth bearer 请求官方 kimi-cli 同款 /models 端点。
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
        // OpenAI 风格 data[]；形状不符按 Err 处理，由 app 层落 fixed_fallback。
        let entries = value.get("data").and_then(Value::as_array).ok_or_else(|| {
            ProviderError::new(
                ProviderErrorKind::InvalidRequest,
                "Kimi Code models response must contain a data array",
            )
        })?;
        let mut definitions = Vec::new();
        for entry in entries {
            let Some(id) = entry.get("id").and_then(Value::as_str) else {
                continue;
            };
            match builtin_models()
                .into_iter()
                .find(|definition| definition.id.as_str() == id)
            {
                Some(definition) => definitions.push(definition),
                None => definitions.push(unknown_model(id)),
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
        self.chat
            .stream(request, sink, cancel)
            .await
            .map_err(|error| normalize_vendor_error(PROVIDER_ID, error))
    }
}

fn require_oauth(
    credential: Option<ResolvedCredential>,
) -> Result<ResolvedCredential, ProviderError> {
    let credential = credential.ok_or_else(|| {
        ProviderError::new(
            ProviderErrorKind::Authentication,
            "Kimi Code requires an OAuth bearer credential",
        )
    })?;
    if credential.kind() != CredentialKind::OAuthBearer
        || credential.expose_secret().trim().is_empty()
    {
        return Err(ProviderError::new(
            ProviderErrorKind::Authentication,
            "Kimi Code accepts only a non-empty OAuth bearer credential",
        ));
    }
    Ok(credential)
}

/// 版本固定 builtin 目录（id 取自官方 kimi-cli / Models.dev；能力未知，
/// 不推断——context/max_output 为 0 表示未知，运行期探测与 config 覆盖可收紧）。
pub fn builtin_models() -> Vec<ModelDefinition> {
    fn model(id: &str, display_name: &str) -> ModelDefinition {
        ModelDefinition {
            id: ModelId::new(id),
            display_name: display_name.into(),
            context_window_tokens: 0,
            max_output_tokens: 0,
            capabilities: ModelCapabilities {
                text: true,
                transport: ModelTransport::ChatCompletions,
                ..ModelCapabilities::default()
            },
        }
    }

    vec![
        model("kimi-for-coding", "Kimi K2.7 Code"),
        model("kimi-for-coding-highspeed", "Kimi K2.7 Code HighSpeed"),
        model("k3", "Kimi K3"),
        model("k3-256k", "Kimi K3 256K"),
    ]
}

/// 远端未知 id 的保守默认：只声明文本输出与 Chat Completions 基线，
/// 窗口/上限未知（0），不推断未证实能力。
fn unknown_model(id: &str) -> ModelDefinition {
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
    fn oauth_is_required_and_api_key_is_rejected() {
        for credential in [
            None,
            Some(ResolvedCredential::new(CredentialKind::ApiKey, "sk-test")),
            Some(ResolvedCredential::new(
                CredentialKind::SessionToken,
                "session",
            )),
        ] {
            assert_eq!(
                KimiCodeProvider::new(KimiCodeConfig::default(), credential)
                    .err()
                    .unwrap()
                    .kind,
                ProviderErrorKind::Authentication
            );
        }
    }

    #[tokio::test]
    async fn remote_models_merge_builtin_metadata() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/models"))
            .and(header("authorization", "Bearer oauth-token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": [{"id": "kimi-for-coding"}, {"id": "kimi-new"}]
            })))
            .expect(1)
            .mount(&server)
            .await;

        let mut config = KimiCodeConfig::new(server.uri());
        config.http = HttpClientConfig::builder().disable_system_proxy().build();
        let provider = KimiCodeProvider::new(
            config,
            Some(ResolvedCredential::new(
                CredentialKind::OAuthBearer,
                "oauth-token",
            )),
        )
        .expect("construct");
        assert_eq!(provider.id().as_str(), "kimi-code");
        let models = provider.list_models(None).await.expect("remote models");
        let ids: Vec<&str> = models.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["kimi-for-coding", "kimi-new"]);
        let known = &models[0];
        assert_eq!(known.display_name, "Kimi K2.7 Code");
        assert_eq!(
            known.capabilities.transport,
            ModelTransport::ChatCompletions
        );
        let unknown = &models[1];
        assert_eq!(unknown.display_name, "kimi-new");
        assert_eq!(unknown.context_window_tokens, 0);
        assert_eq!(unknown.max_output_tokens, 0);
        assert_eq!(
            unknown.capabilities.transport,
            ModelTransport::ChatCompletions
        );
        server.verify().await;
    }

    #[tokio::test]
    async fn remote_models_shape_mismatch_returns_err() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/models"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"object": "list", "items": []})),
            )
            .expect(1)
            .mount(&server)
            .await;

        let mut config = KimiCodeConfig::new(server.uri());
        config.http = HttpClientConfig::builder().disable_system_proxy().build();
        let provider = KimiCodeProvider::new(
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
            .expect("shape mismatch must error");
        assert_eq!(error.kind, ProviderErrorKind::InvalidRequest);
        server.verify().await;
    }

    #[test]
    fn fixed_credential_header_is_rejected() {
        let mut config = KimiCodeConfig::default();
        config
            .http
            .extra_headers
            .push(("Authorization".into(), "Bearer attacker".into()));
        let credential = ResolvedCredential::new(CredentialKind::OAuthBearer, "oauth-token");
        let error = KimiCodeProvider::new(config, Some(credential))
            .err()
            .expect("duplicate credential header must fail");
        assert_eq!(error.kind, ProviderErrorKind::InvalidRequest);
    }
}
