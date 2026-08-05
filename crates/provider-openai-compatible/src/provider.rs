//! OpenAI-compatible [`ModelProvider`] 实现。
//!
//! 一个适配同时覆盖云端 OpenAI 兼容接口与多数本地服务（Ollama / vLLM / LM Studio）。
//! 认证头 `Authorization: Bearer <secret>` 来自 [`ResolvedCredential`]，绝不记录。

use std::time::Duration;

use agent_domain::{CancellationToken, ModelId, ProviderId, StopReason, TokenUsage};
use async_trait::async_trait;
use provider_api::{
    CanonicalModelRequest, ModelCapabilities, ModelDefinition, ModelProvider, ModelResponseSummary,
    ProviderError, ProviderErrorKind, ProviderEventSink, ProviderStreamEvent, ResolvedCredential,
};
use provider_runtime::http::{HttpClient, HttpClientConfig};
use provider_runtime::sse::SseParser;
use serde_json::Value;

use crate::stream::{chunk_to_events, is_done, ChunkState};

/// OpenAI-compatible 适配器配置。
#[derive(Clone, Debug)]
pub struct OpenAiCompatibleConfig {
    /// 基础 URL，如 `https://api.openai.com/v1` 或本地 `http://localhost:11434/v1`。
    pub base_url: String,
    /// Provider 标识（默认 `openai-compatible`）。
    pub provider_id: ProviderId,
    /// HTTP 客户端配置。
    pub http: HttpClientConfig,
    /// 请求超时（覆盖 http.timeout 时的便捷字段）。
    pub request_timeout: Option<Duration>,
}

impl OpenAiCompatibleConfig {
    /// 构造云端 OpenAI 兼容配置（base_url 以 `/v1` 结尾）。
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            provider_id: ProviderId::new("openai-compatible"),
            http: HttpClientConfig::default(),
            request_timeout: None,
        }
    }

    pub fn with_provider_id(mut self, id: impl Into<String>) -> Self {
        self.provider_id = ProviderId::new(id.into());
        self
    }

    fn chat_url(&self) -> String {
        format!("{}/chat/completions", self.base_url.trim_end_matches('/'))
    }

    fn models_url(&self) -> String {
        format!("{}/models", self.base_url.trim_end_matches('/'))
    }
}

/// OpenAI-compatible Provider。
pub struct OpenAiCompatibleProvider {
    config: OpenAiCompatibleConfig,
    client: HttpClient,
    credential: Option<ResolvedCredential>,
}

impl OpenAiCompatibleProvider {
    /// 构造适配器。`credential` 为 None 时不带认证头（本地服务常用）。
    pub fn new(
        config: OpenAiCompatibleConfig,
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

    fn auth_header(&self) -> Option<(String, String)> {
        self.credential.as_ref().map(|cred| {
            (
                "Authorization".to_string(),
                format!("Bearer {}", cred.expose_secret()),
            )
        })
    }

    async fn drive_stream(
        &self,
        request: &CanonicalModelRequest,
        sink: &dyn ProviderEventSink,
        cancel: CancellationToken,
    ) -> Result<ModelResponseSummary, ProviderError> {
        // 构造请求体（canonical → OpenAI）
        let body = crate::request::to_chat_completions_body(request);

        // 认证头（明文 secret 只在此短暂存在，不持久化、不记录）
        let auth_header = self.auth_header();
        let per_request_headers: [(String, String); 1] = match &auth_header {
            Some(pair) => [pair.clone()],
            None => [("".to_string(), "".to_string())],
        };
        let per_request_headers: &[(String, String)] = if auth_header.is_some() {
            &per_request_headers[..]
        } else {
            &[]
        };

        // 发起 POST 流式请求
        let mut byte_stream = self
            .client
            .post_stream_with_headers(
                &self.config.chat_url(),
                body,
                request.trace_id.as_deref(),
                per_request_headers,
                cancel.clone(),
            )
            .await?;

        // 头部 emit ResponseStarted
        let response_id = request.trace_id.clone();
        sink.emit(ProviderStreamEvent::ResponseStarted { response_id })
            .await?;

        // 用 SSE 解析器消费字节流
        let mut sse = SseParser::new();
        let mut chunk_state = ChunkState::default();
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
                let data = event.data.trim();
                if is_done(data) {
                    saw_completion = true;
                    break;
                }
                if data.is_empty() {
                    continue;
                }
                for ev in chunk_to_events(data, &mut chunk_state) {
                    match &ev {
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

        // 冲刷残留
        if let Some(event) = sse.finish() {
            let data = event.data.trim();
            if !is_done(data) && !data.is_empty() {
                for ev in chunk_to_events(data, &mut chunk_state) {
                    match &ev {
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
                ProviderErrorKind::StreamInterrupted,
                "stream ended without finish_reason or [DONE]",
            ));
        }

        summary.response_id = request.trace_id.clone();
        Ok(summary)
    }
}

#[async_trait]
impl ModelProvider for OpenAiCompatibleProvider {
    fn id(&self) -> ProviderId {
        self.config.provider_id.clone()
    }

    async fn list_models(
        &self,
        _credential: Option<&ResolvedCredential>,
    ) -> Result<Vec<ModelDefinition>, ProviderError> {
        let value = self
            .client
            .get_json(&self.config.models_url(), None, CancellationToken::new())
            .await?;

        let models = value
            .get("data")
            .and_then(|d| d.as_array())
            .cloned()
            .unwrap_or_default();

        Ok(models
            .into_iter()
            .filter_map(|m| {
                let id = m.get("id").and_then(|i| i.as_str())?.to_string();
                Some(ModelDefinition {
                    id: ModelId::new(id),
                    display_name: m
                        .get("id")
                        .and_then(|i| i.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    context_window_tokens: 128_000,
                    max_output_tokens: 16_384,
                    capabilities: ModelCapabilities {
                        text: true,
                        tool_calls: true,
                        ..ModelCapabilities::default()
                    },
                })
            })
            .collect())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chat_url_trims_trailing_slash() {
        let config = OpenAiCompatibleConfig::new("https://api.example.com/v1/");
        assert_eq!(
            config.chat_url(),
            "https://api.example.com/v1/chat/completions"
        );
        assert_eq!(config.models_url(), "https://api.example.com/v1/models");
    }
}
