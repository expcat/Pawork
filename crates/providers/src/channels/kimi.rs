//! Kimi Code / Coding Plan adapter（SET-4 A2）：只走 OpenAI-compatible
//! Chat Completions（api.kimi.com/coding/v1）。
//!
//! OAuth 获取 / 刷新归 pawork-auth（Device Flow，端点在 registry 预设）。
//! Coding Plan API key 与 OAuth bearer 用法相同（`Authorization: Bearer`）；
//! 本 adapter 同时接受两种形态。SET-5 起 `list_models` 走远端
//! `GET {base}/models`（与官方 kimi-cli 同端点，OpenAI 风格 data[] 解析；
//! 已知 id 沿用 builtin 元数据，未知 id 只给保守默认；形状不符即 Err）。

use std::time::Duration;

use crate::net::http::{HttpClient, HttpClientConfig};
use async_trait::async_trait;
use pawork_domain::{CancellationToken, ModelId, ProviderId};
use pawork_domain::{
    CanonicalModelRequest, ModelCapabilities, ModelDefinition, ModelProvider, ModelResponseSummary,
    ModelTransport, ProviderError, ProviderEventSink, ResolvedCredential,
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
        let credential = super::require_bearer_credential("Kimi Code", credential)?;
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
        // 远端目录：Bearer（OAuth 或 Coding Plan API key）请求官方 /models。
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
        let entries = crate::provider::catalog_entries(&value, "data")?;
        let builtin = builtin_models();
        let mut definitions = Vec::new();
        for entry in entries {
            let id = crate::provider::catalog_model_id(entry, "id")?;
            let mut definition = builtin
                .iter()
                .find(|definition| definition.id.as_str() == id)
                .cloned()
                .unwrap_or_else(|| unknown_model(id));
            // 官方 kimi-cli auth/platforms.py ModelInfo（2026-09-08）。
            if let Some(name) = entry.get("display_name").and_then(Value::as_str) {
                definition.display_name = name.to_owned();
            }
            if let Some(context) = entry.get("context_length").and_then(Value::as_u64) {
                definition.context_window_tokens = context;
            }
            if let Some(thinking) = entry.get("supports_reasoning").and_then(Value::as_bool) {
                definition.capabilities.thinking = thinking;
            }
            if let Some(image_input) = entry.get("supports_image_in").and_then(Value::as_bool) {
                definition.capabilities.image_input = image_input;
            } else {
                // VISION-2：远端未声明时按官方视觉指南默认表回填。
                crate::registry::apply_default_image_input(&mut definition);
            }
            // supports_video_in 不证明当前 endpoint 支持远程 HTTP(S) 视频；不能冒充 image_input。
            // ADR-063：远端未声明推理强度时按默认表回填（k3 系实测忽略
            // effort 字段，表内为未知不约束；K2.7 系为显式无档位）。
            crate::registry::apply_default_supported_efforts(&mut definition);
            definitions.push(definition);
        }
        Ok(definitions)
    }

    async fn stream(
        &self,
        request: &CanonicalModelRequest,
        sink: &dyn ProviderEventSink,
        cancel: CancellationToken,
    ) -> Result<ModelResponseSummary, ProviderError> {
        // 官方视觉指南只收 base64 与 ms:// 文件 ID；外部 URL 发 HTTP 前拒绝。
        crate::request::reject_kimi_external_image_urls(request)?;
        self.chat
            .stream(request, sink, cancel)
            .await
            .map_err(|error| normalize_vendor_error(PROVIDER_ID, error))
    }
}

/// 版本固定 builtin 目录（id 取自官方 kimi-cli / Models.dev；能力未知，
/// 不推断——context/max_output 为 0 表示未知，运行期探测与 config 覆盖可收紧）。
pub fn builtin_models() -> Vec<ModelDefinition> {
    /// VISION-1：K2.7 Code / K3 均声明原生图片输入
    ///（platform.kimi.ai/docs/models.md 与 kimi-k2-7-code quickstart，2026-09-15 调研）。
    /// web search（$web_search）需客户端回显 arguments 的两段流程，本通道未接线，
    /// 不声明 WebSearch（fail-closed）。
    fn model(id: &str, display_name: &str) -> ModelDefinition {
        ModelDefinition {
            id: ModelId::new(id),
            display_name: display_name.into(),
            context_window_tokens: 0,
            max_output_tokens: 0,
            capabilities: ModelCapabilities {
                text: true,
                image_input: true,
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
    use pawork_domain::{CredentialKind, ProviderErrorKind};
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[test]
    fn bearer_api_key_or_oauth_is_required() {
        for credential in [
            None,
            Some(ResolvedCredential::new(
                CredentialKind::SessionToken,
                "session",
            )),
            Some(ResolvedCredential::new(CredentialKind::ApiKey, "  ")),
        ] {
            assert_eq!(
                KimiCodeProvider::new(KimiCodeConfig::default(), credential)
                    .err()
                    .unwrap()
                    .kind,
                ProviderErrorKind::Authentication
            );
        }
        for credential in [
            ResolvedCredential::new(CredentialKind::ApiKey, "sk-kimi-test"),
            ResolvedCredential::new(CredentialKind::OAuthBearer, "oauth-token"),
        ] {
            KimiCodeProvider::new(KimiCodeConfig::default(), Some(credential))
                .expect("bearer credential must construct");
        }
    }

    #[tokio::test]
    async fn remote_models_merge_builtin_metadata() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/models"))
            .and(header("authorization", "Bearer oauth-token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": [
                    {"id": "kimi-for-coding", "display_name": "Current Code", "context_length": 262144,
                     "supports_reasoning": true, "supports_image_in": true},
                    {"id": "kimi-new", "supports_video_in": true}
                ]
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
        assert_eq!(known.display_name, "Current Code");
        assert_eq!(known.context_window_tokens, 262144);
        assert!(known.capabilities.thinking);
        assert!(known.capabilities.image_input);
        assert_eq!(
            known.capabilities.transport,
            ModelTransport::ChatCompletions
        );
        let unknown = &models[1];
        assert_eq!(unknown.display_name, "kimi-new");
        assert_eq!(unknown.context_window_tokens, 0);
        assert_eq!(unknown.max_output_tokens, 0);
        assert!(!unknown.capabilities.image_input);
        assert!(!unknown.capabilities.tool_calls);
        assert_eq!(
            unknown.capabilities.transport,
            ModelTransport::ChatCompletions
        );
        server.verify().await;
    }

    #[tokio::test]
    async fn remote_models_accept_coding_plan_api_key() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/models"))
            .and(header("authorization", "Bearer sk-kimi-plan"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": [{"id": "kimi-for-coding", "display_name": "K2.8 Preview"}]
            })))
            .expect(1)
            .mount(&server)
            .await;

        let mut config = KimiCodeConfig::new(server.uri());
        config.http = HttpClientConfig::builder().disable_system_proxy().build();
        let provider = KimiCodeProvider::new(
            config,
            Some(ResolvedCredential::new(
                CredentialKind::ApiKey,
                "sk-kimi-plan",
            )),
        )
        .expect("construct");
        let models = provider.list_models(None).await.expect("remote models");
        assert_eq!(models[0].id.as_str(), "kimi-for-coding");
        assert_eq!(models[0].display_name, "K2.8 Preview");
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

    #[tokio::test]
    #[ignore = "live Coding Plan key; set PAWORK_TEST_KIMI_CODE_KEY"]
    async fn live_coding_plan_api_key_lists_models() {
        let key = std::env::var("PAWORK_TEST_KIMI_CODE_KEY")
            .expect("PAWORK_TEST_KIMI_CODE_KEY must be set for this ignored test");
        let mut config = KimiCodeConfig::default();
        config.http = HttpClientConfig::builder().disable_system_proxy().build();
        let provider = KimiCodeProvider::new(
            config,
            Some(ResolvedCredential::new(CredentialKind::ApiKey, key.clone())),
        )
        .expect("construct");
        let models = provider
            .list_models(None)
            .await
            .expect("live Coding Plan /models");
        assert!(
            models
                .iter()
                .any(|model| model.id.as_str() == "kimi-for-coding"),
            "unexpected live catalog: {:?}",
            models
                .iter()
                .map(|model| model.id.as_str())
                .collect::<Vec<_>>()
        );
        let verify_config = crate::ApiKeyChannelConfig::new(
            crate::channel_preset(PROVIDER_ID).expect("kimi-code preset"),
        )
        .expect("verify config")
        .with_http(HttpClientConfig::builder().disable_system_proxy().build());
        crate::verify_api_key(verify_config, &key)
            .await
            .expect("Settings verify_api_key must accept a live Coding Plan key");
    }
}
