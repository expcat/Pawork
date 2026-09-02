//! Kimi Code OAuth adapter（SET-4 A2）：只消费 OAuth bearer，只走
//! OpenAI-compatible Chat Completions（api.kimi.com/coding/v1）。
//!
//! OAuth 获取 / 刷新归 pawork-auth（Device Flow，端点在 registry 预设）；
//! 本 adapter 不接受 API key 形态凭证。模型目录为版本固定 builtin
//!（id 取自官方 kimi-cli / Models.dev；能力未知，不推断）。

use std::time::Duration;

use crate::net::http::HttpClientConfig;
use async_trait::async_trait;
use pawork_domain::{CancellationToken, ModelId, ProviderId};
use pawork_domain::{
    CanonicalModelRequest, CredentialKind, ModelCapabilities, ModelDefinition, ModelProvider,
    ModelResponseSummary, ModelTransport, ProviderError, ProviderErrorKind, ProviderEventSink,
    ResolvedCredential,
};

use crate::normalize_vendor_error;
use crate::provider::{OpenAiCompatibleConfig, OpenAiCompatibleProvider};

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
}

impl KimiCodeProvider {
    pub fn new(
        config: KimiCodeConfig,
        credential: Option<ResolvedCredential>,
    ) -> Result<Self, ProviderError> {
        let credential = require_oauth(credential)?;
        let chat = OpenAiCompatibleProvider::new(
            OpenAiCompatibleConfig {
                base_url: config.base_url,
                provider_id: ProviderId::new(PROVIDER_ID),
                http: config.http,
                request_timeout: config.request_timeout,
            },
            Some(credential),
        )?;
        Ok(Self { chat })
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
        Ok(builtin_models())
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

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn oauth_bearer_constructs_and_lists_builtin_models() {
        let provider = KimiCodeProvider::new(
            KimiCodeConfig::default(),
            Some(ResolvedCredential::new(
                CredentialKind::OAuthBearer,
                "oauth-token",
            )),
        )
        .expect("construct");
        assert_eq!(provider.id().as_str(), "kimi-code");
        let models =
            futures::executor::block_on(provider.list_models(None)).expect("builtin models");
        let ids: Vec<&str> = models.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "kimi-for-coding",
                "kimi-for-coding-highspeed",
                "k3",
                "k3-256k",
            ]
        );
        assert!(models
            .iter()
            .all(|m| m.capabilities.transport == ModelTransport::ChatCompletions));
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
