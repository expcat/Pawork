//! Anthropic 原生 [`ModelProvider`](provider_api::ModelProvider) 实现。
//!
//! 认证头 `x-api-key: <secret>` + `anthropic-version: 2023-06-01`，明文 secret
//! 只在构造 header 时短暂出现，不持久化、不记录。流式响应由 [`SseParser`] 驱动，
//! 经 [`event_to_events`](crate::stream::event_to_events) 映射为 canonical 事件。

use std::time::Duration;

use agent_domain::{CancellationToken, ModelId, ProviderId, StopReason, TokenUsage};
use async_trait::async_trait;
use provider_api::{
    CanonicalModelRequest, ModelCapabilities, ModelDefinition, ModelProvider, ModelResponseSummary,
    ProviderError, ProviderEventSink, ProviderStreamEvent, ResolvedCredential,
};
use provider_runtime::http::{HttpClient, HttpClientConfig};
use provider_runtime::sse::SseParser;
use serde_json::Value;

use crate::stream::{event_to_events, AnthropicStreamState};
use crate::{ANTHROPIC_VERSION, DEFAULT_BASE_URL};

/// Anthropic 适配器配置。
#[derive(Clone, Debug)]
pub struct AnthropicConfig {
    /// 基础 URL（默认 `https://api.anthropic.com`）。
    pub base_url: String,
    /// Provider 标识（默认 `anthropic`）。
    pub provider_id: ProviderId,
    /// HTTP 客户端配置。
    pub http: HttpClientConfig,
    /// 请求超时（覆盖 http.timeout）。
    pub request_timeout: Option<Duration>,
}

impl Default for AnthropicConfig {
    fn default() -> Self {
        Self {
            base_url: DEFAULT_BASE_URL.into(),
            provider_id: ProviderId::new("anthropic"),
            http: HttpClientConfig::default(),
            request_timeout: None,
        }
    }
}

impl AnthropicConfig {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            ..Default::default()
        }
    }

    pub fn with_provider_id(mut self, id: impl Into<String>) -> Self {
        self.provider_id = ProviderId::new(id.into());
        self
    }

    fn messages_url(&self) -> String {
        format!("{}/v1/messages", self.base_url.trim_end_matches('/'))
    }

    #[allow(dead_code)]
    fn models_url(&self) -> String {
        format!("{}/v1/models", self.base_url.trim_end_matches('/'))
    }
}

/// Anthropic Provider。
pub struct AnthropicProvider {
    config: AnthropicConfig,
    client: HttpClient,
    credential: Option<ResolvedCredential>,
}

impl AnthropicProvider {
    /// 构造适配器。`credential` 为 None 时不带认证头。
    pub fn new(
        config: AnthropicConfig,
        credential: Option<ResolvedCredential>,
    ) -> Result<Self, ProviderError> {
        let http_config = match config.request_timeout {
            Some(timeout) => {
                let mut c = config.http.clone();
                c.timeout = Some(timeout);
                c
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

    async fn drive_stream(
        &self,
        request: &CanonicalModelRequest,
        sink: &dyn ProviderEventSink,
        cancel: CancellationToken,
    ) -> Result<ModelResponseSummary, ProviderError> {
        let body = crate::request::to_messages_body(request);
        let headers = self.auth_headers();

        let mut byte_stream = self
            .client
            .post_stream_with_headers(
                &self.config.messages_url(),
                body,
                request.trace_id.as_deref(),
                &headers,
                cancel.clone(),
            )
            .await?;

        let mut sse = SseParser::new();
        let mut state = AnthropicStreamState::default();
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
                for ev in event_to_events(data, &mut state) {
                    match &ev {
                        ProviderStreamEvent::ResponseStarted { response_id } => {
                            summary.response_id.clone_from(response_id);
                        }
                        ProviderStreamEvent::UsageUpdated(u) => summary.usage = u.clone(),
                        ProviderStreamEvent::ResponseCompleted(stop) => {
                            summary.stop_reason = stop.clone();
                            saw_completion = true;
                        }
                        _ => {}
                    }
                    sink.emit(ev).await?;
                }
            }
        }

        if let Some(event) = sse.finish()? {
            let data = event.data.trim();
            if !data.is_empty() {
                for ev in event_to_events(data, &mut state) {
                    match &ev {
                        ProviderStreamEvent::ResponseStarted { response_id } => {
                            summary.response_id.clone_from(response_id);
                        }
                        ProviderStreamEvent::UsageUpdated(u) => summary.usage = u.clone(),
                        ProviderStreamEvent::ResponseCompleted(stop) => {
                            summary.stop_reason = stop.clone();
                            saw_completion = true;
                        }
                        _ => {}
                    }
                    sink.emit(ev).await?;
                }
            }
        }

        if !saw_completion {
            return Err(ProviderError::new(
                provider_api::ProviderErrorKind::StreamInterrupted,
                "anthropic stream ended without message_stop",
            ));
        }

        Ok(summary)
    }
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
        self.drive_stream(&request, sink, cancel).await
    }
}

/// Anthropic 内置模型目录（含 thinking / image / tool / cache 能力）。
pub fn builtin_models() -> Vec<ModelDefinition> {
    fn caps(
        text: bool,
        image_input: bool,
        tool_calls: bool,
        parallel_tool_calls: bool,
        thinking: bool,
        structured_output: bool,
        prompt_cache: bool,
    ) -> ModelCapabilities {
        ModelCapabilities {
            text,
            image_input,
            tool_calls,
            parallel_tool_calls,
            thinking,
            structured_output,
            prompt_cache,
        }
    }

    fn def(
        id: &str,
        display: &str,
        context_window_tokens: u64,
        max_output_tokens: u64,
        capabilities: ModelCapabilities,
    ) -> ModelDefinition {
        ModelDefinition {
            id: ModelId::new(id),
            display_name: display.into(),
            context_window_tokens,
            max_output_tokens,
            capabilities,
        }
    }

    let full = caps(true, true, true, true, true, true, true);
    let haiku = caps(true, true, true, true, false, true, true);

    vec![
        def(
            "claude-3-5-sonnet",
            "Claude 3.5 Sonnet",
            200_000,
            8_192,
            full.clone(),
        ),
        def(
            "claude-3-5-sonnet-20241022",
            "Claude 3.5 Sonnet (2024-10-22)",
            200_000,
            8_192,
            full.clone(),
        ),
        def(
            "claude-3-5-haiku",
            "Claude 3.5 Haiku",
            200_000,
            8_192,
            haiku,
        ),
        def(
            "claude-3-opus",
            "Claude 3 Opus",
            200_000,
            4_096,
            caps(true, true, true, true, false, true, true),
        ),
        def(
            "claude-3-sonnet",
            "Claude 3 Sonnet",
            200_000,
            4_096,
            caps(true, true, true, true, false, true, true),
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_defaults_to_anthropic_endpoint() {
        let config = AnthropicConfig::default();
        assert_eq!(config.base_url, DEFAULT_BASE_URL);
        assert_eq!(config.provider_id, ProviderId::new("anthropic"));
        assert_eq!(config.models_url(), "https://api.anthropic.com/v1/models");
    }

    #[test]
    fn builtin_catalog_has_reasoning_and_vision_models() {
        let models = builtin_models();
        let sonnet = models
            .iter()
            .find(|m| m.id == ModelId::new("claude-3-5-sonnet"))
            .expect("sonnet present");
        assert!(sonnet.capabilities.thinking);
        assert!(sonnet.capabilities.image_input);
        assert!(sonnet.capabilities.prompt_cache);
        assert!(sonnet.capabilities.tool_calls);
    }

    #[test]
    fn auth_headers_include_api_key_and_version() {
        let config = AnthropicConfig::default();
        let provider = AnthropicProvider::new(
            config,
            Some(ResolvedCredential::new(
                provider_api::CredentialKind::ApiKey,
                "sk-ant-test",
            )),
        )
        .expect("构造 adapter");
        let headers = provider.auth_headers();
        assert_eq!(
            headers
                .iter()
                .find(|(k, _)| k == "x-api-key")
                .map(|(_, v)| v),
            Some(&"sk-ant-test".to_string())
        );
        assert_eq!(
            headers
                .iter()
                .find(|(k, _)| k == "anthropic-version")
                .map(|(_, v)| v),
            Some(&ANTHROPIC_VERSION.to_string())
        );
    }

    #[test]
    fn auth_headers_without_credential_still_has_version() {
        let config = AnthropicConfig::default();
        let provider = AnthropicProvider::new(config, None).expect("构造 adapter");
        let headers = provider.auth_headers();
        assert!(headers.iter().all(|(k, _)| k != "x-api-key"));
        assert_eq!(
            headers
                .iter()
                .find(|(k, _)| k == "anthropic-version")
                .map(|(_, v)| v),
            Some(&ANTHROPIC_VERSION.to_string())
        );
    }
}
