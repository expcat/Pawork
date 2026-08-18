//! Anthropic Messages [`ModelProvider`] 实现（S2 基线路径）。
//!
//! 认证头 `x-api-key` + `anthropic-version`；明文 secret 只在构造 header 时短暂
//! 出现，不持久化、不记录。`base_url` 必填，端点为 `{base_url}/v1/messages`。
//! [`ModelProvider::stream`] 只走 Messages 基线（等价 V1 `drive_legacy_stream`）。

use std::time::Duration;

use async_trait::async_trait;
use pawork_domain::{
    CanonicalModelRequest, ModelCapabilities, ModelDefinition, ModelProvider, ModelResponseSummary,
    ProviderError, ProviderErrorKind, ProviderEventSink, ProviderStreamEvent, ResolvedCredential,
};
use pawork_domain::{CancellationToken, ModelId, ProviderId, StopReason, TokenUsage};
use pawork_net::http::{HttpClient, HttpClientConfig};
use pawork_net::sse::SseParser;
use serde_json::Value;

use super::stream::{event_to_events, AnthropicStreamState};
use super::ANTHROPIC_VERSION;

/// Anthropic 适配器配置。`base_url` 必填，不内置官方端点。
#[derive(Clone, Debug)]
pub struct AnthropicConfig {
    /// 基础 URL，如兼容网关的 `https://example.com/api/anthropic`。
    pub base_url: String,
    /// Provider 标识（默认 `anthropic`）。
    pub provider_id: ProviderId,
    /// HTTP 客户端配置。
    pub http: HttpClientConfig,
    /// 建连及流式读取无数据超时（覆盖 `http.timeout` 时的便捷字段）。
    pub request_timeout: Option<Duration>,
}

impl AnthropicConfig {
    /// 构造配置。`base_url` 为 Messages 根，不含 `/v1/messages`。
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            provider_id: ProviderId::new("anthropic"),
            http: HttpClientConfig::default(),
            request_timeout: None,
        }
    }

    pub fn with_provider_id(mut self, id: impl Into<String>) -> Self {
        self.provider_id = ProviderId::new(id.into());
        self
    }

    fn messages_url(&self) -> String {
        format!("{}/v1/messages", self.base_url.trim_end_matches('/'))
    }
}

/// Anthropic Provider。
pub struct AnthropicProvider {
    config: AnthropicConfig,
    client: HttpClient,
    credential: Option<ResolvedCredential>,
}

impl AnthropicProvider {
    /// 构造适配器。`credential` 为 None 时不带 `x-api-key`（仍发送协议版本头）。
    pub fn new(
        config: AnthropicConfig,
        credential: Option<ResolvedCredential>,
    ) -> Result<Self, ProviderError> {
        if credential.is_some()
            && config
                .http
                .extra_headers
                .iter()
                .any(|(name, _)| crate::is_credential_header(name))
        {
            return Err(ProviderError::new(
                ProviderErrorKind::InvalidRequest,
                "authenticated Anthropic transport cannot override credential headers",
            ));
        }
        let http_config = match config.request_timeout {
            Some(timeout) => {
                let mut cloned = config.http.clone();
                cloned.timeout = Some(timeout);
                cloned
            }
            None => config.http.clone(),
        };
        let client = HttpClient::new(http_config)?;
        Ok(Self {
            config,
            client,
            credential,
        })
    }

    fn auth_headers(&self) -> Vec<(String, String)> {
        let mut headers = Vec::new();
        if let Some(cred) = &self.credential {
            headers.push(("x-api-key".to_string(), cred.expose_secret().to_string()));
        }
        headers.push((
            "anthropic-version".to_string(),
            ANTHROPIC_VERSION.to_string(),
        ));
        headers
    }

    async fn drive_legacy_stream(
        &self,
        request: &CanonicalModelRequest,
        sink: &dyn ProviderEventSink,
        cancel: CancellationToken,
    ) -> Result<ModelResponseSummary, ProviderError> {
        let body = super::request::to_messages_body(request);
        let mut state = AnthropicStreamState::default();
        self.pump_messages(body, request.trace_id.as_deref(), &mut state, sink, cancel)
            .await
    }

    async fn pump_messages(
        &self,
        body: Value,
        trace_id: Option<&str>,
        state: &mut AnthropicStreamState,
        sink: &dyn ProviderEventSink,
        cancel: CancellationToken,
    ) -> Result<ModelResponseSummary, ProviderError> {
        let headers = self.auth_headers();

        let mut byte_stream = self
            .client
            .post_stream_with_headers(
                &self.config.messages_url(),
                body,
                trace_id,
                &headers,
                cancel.clone(),
            )
            .await?;

        let mut sse = SseParser::new();
        let mut summary = ModelResponseSummary {
            stop_reason: StopReason::Error,
            usage: TokenUsage::default(),
            response_id: None,
            provider_metadata: Value::Null,
        };
        let mut saw_completion = false;

        use futures::StreamExt;
        while let Some(item) = byte_stream.next().await {
            if cancel.is_cancelled() {
                return Err(ProviderError::cancelled("stream cancelled"));
            }
            let bytes = item?;
            for event in sse.feed(&bytes) {
                if cancel.is_cancelled() {
                    return Err(ProviderError::cancelled("stream cancelled"));
                }
                let event = event?;
                let data = event.data.trim();
                if data.is_empty() {
                    continue;
                }
                self.process_chunk(data, state, sink, &mut summary, &mut saw_completion)
                    .await?;
            }
        }

        if let Some(event) = sse.finish()? {
            let data = event.data.trim();
            if !data.is_empty() {
                self.process_chunk(data, state, sink, &mut summary, &mut saw_completion)
                    .await?;
            }
        }

        if !saw_completion {
            return Err(ProviderError::new(
                ProviderErrorKind::StreamInterrupted,
                "anthropic stream ended without message_stop",
            ));
        }

        Ok(summary)
    }

    async fn process_chunk(
        &self,
        data: &str,
        state: &mut AnthropicStreamState,
        sink: &dyn ProviderEventSink,
        summary: &mut ModelResponseSummary,
        saw_completion: &mut bool,
    ) -> Result<(), ProviderError> {
        for ev in event_to_events(data, state) {
            match &ev {
                ProviderStreamEvent::ResponseStarted { response_id } => {
                    summary.response_id.clone_from(response_id);
                }
                ProviderStreamEvent::UsageUpdated(usage) => summary.usage = usage.clone(),
                ProviderStreamEvent::ResponseCompleted(stop) => {
                    summary.stop_reason = stop.clone();
                    *saw_completion = true;
                }
                ProviderStreamEvent::Error(err) => {
                    let err = err.clone();
                    sink.emit(ev).await?;
                    return Err(err);
                }
                _ => {}
            }
            sink.emit(ev).await?;
        }
        Ok(())
    }
}

/// Anthropic Messages 协议的静态内置模型目录（S5 registry 合并源）。
///
/// 调用方（app 装配层）在选中 Messages 协议时把它并入 ModelRegistry；
/// 本函数不做 Provider 名称分支，id 由调用方提供。
pub fn builtin_models() -> Vec<ModelDefinition> {
    let capabilities = ModelCapabilities {
        text: true,
        image_input: true,
        tool_calls: true,
        parallel_tool_calls: true,
        structured_output: true,
        ..ModelCapabilities::default()
    };
    vec![
        ModelDefinition {
            id: ModelId::new("claude-3-5-sonnet"),
            display_name: "Claude 3.5 Sonnet".into(),
            context_window_tokens: 200_000,
            max_output_tokens: 8_192,
            capabilities: capabilities.clone(),
        },
        ModelDefinition {
            id: ModelId::new("claude-3-5-haiku"),
            display_name: "Claude 3.5 Haiku".into(),
            context_window_tokens: 200_000,
            max_output_tokens: 8_192,
            capabilities,
        },
    ]
}

#[async_trait]
impl ModelProvider for AnthropicProvider {
    fn id(&self) -> ProviderId {
        self.config.provider_id.clone()
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
        self.drive_legacy_stream(&request, sink, cancel).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pawork_domain::CredentialKind;

    #[test]
    fn messages_url_trims_trailing_slash() {
        let config = AnthropicConfig::new("https://gateway.example/api/anthropic/");
        assert_eq!(
            config.messages_url(),
            "https://gateway.example/api/anthropic/v1/messages"
        );
    }

    #[test]
    fn config_new_requires_base_url_and_does_not_default_official_host() {
        let config = AnthropicConfig::new("https://gateway.example");
        assert_eq!(config.base_url, "https://gateway.example");
        assert_eq!(config.provider_id, ProviderId::new("anthropic"));
        assert!(!config.base_url.contains("api.anthropic.com"));
        assert!(!config.messages_url().contains("api.anthropic.com"));
    }

    #[test]
    fn provider_id_is_configurable() {
        let config = AnthropicConfig::new("https://gateway.example").with_provider_id("test");
        let provider = AnthropicProvider::new(config, None).expect("构造 adapter");
        assert_eq!(provider.id().as_str(), "test");
    }

    #[test]
    fn auth_headers_include_api_key_and_version() {
        let provider = AnthropicProvider::new(
            AnthropicConfig::new("https://gateway.example"),
            Some(ResolvedCredential::new(CredentialKind::ApiKey, "sk-ant-test")),
        )
        .expect("构造 adapter");
        let headers = provider.auth_headers();
        assert_eq!(
            headers
                .iter()
                .find(|(key, _)| key == "x-api-key")
                .map(|(_, value)| value.as_str()),
            Some("sk-ant-test")
        );
        assert_eq!(
            headers
                .iter()
                .find(|(key, _)| key == "anthropic-version")
                .map(|(_, value)| value.as_str()),
            Some(ANTHROPIC_VERSION)
        );
    }

    #[test]
    fn auth_headers_without_credential_still_has_version() {
        let provider = AnthropicProvider::new(AnthropicConfig::new("https://gateway.example"), None)
            .expect("构造 adapter");
        let headers = provider.auth_headers();
        assert!(headers.iter().all(|(key, _)| key != "x-api-key"));
        assert_eq!(
            headers
                .iter()
                .find(|(key, _)| key == "anthropic-version")
                .map(|(_, value)| value.as_str()),
            Some(ANTHROPIC_VERSION)
        );
    }

    #[test]
    fn list_models_is_static() {
        let models = builtin_models();
        assert!(models
            .iter()
            .any(|model| model.id == ModelId::new("claude-3-5-sonnet")));
        assert!(models.iter().all(|model| model.capabilities.tool_calls));
    }

    #[test]
    fn fixed_credential_header_is_rejected() {
        let mut config = AnthropicConfig::new("https://gateway.example");
        config
            .http
            .extra_headers
            .push(("x-api-key".into(), "sk-attacker".into()));
        let error = AnthropicProvider::new(
            config,
            Some(ResolvedCredential::new(CredentialKind::ApiKey, "sk-ant-test")),
        )
        .err()
        .expect("duplicate credential header must fail");
        assert_eq!(error.kind, ProviderErrorKind::InvalidRequest);
    }
}
