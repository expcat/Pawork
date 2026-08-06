//! Google Gemini 原生 [`ModelProvider`](provider_api::ModelProvider) 实现。
//!
//! 认证走 query 参数 `?key=<secret>&alt=sse`（明文 secret 只在构造 URL 时短暂
//! 出现，不记录、不持久化）。流式响应由 [`SseParser`] 驱动，经
//! [`chunk_to_events`](crate::stream::chunk_to_events) 映射为 canonical 事件。

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

use crate::stream::{chunk_to_events, ChunkState};
use crate::DEFAULT_BASE_URL;

/// Google Gemini 适配器配置。
#[derive(Clone, Debug)]
pub struct GoogleConfig {
    /// 基础 URL（默认 `https://generativelanguage.googleapis.com`）。
    pub base_url: String,
    /// Provider 标识（默认 `google`）。
    pub provider_id: ProviderId,
    /// HTTP 客户端配置。
    pub http: HttpClientConfig,
    /// 请求超时（覆盖 http.timeout）。
    pub request_timeout: Option<Duration>,
}

impl Default for GoogleConfig {
    fn default() -> Self {
        Self {
            base_url: DEFAULT_BASE_URL.into(),
            provider_id: ProviderId::new("google"),
            http: HttpClientConfig::default(),
            request_timeout: None,
        }
    }
}

impl GoogleConfig {
    /// 以自定义基础 URL 构造（provider_id 仍为 `google`）。
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            ..Default::default()
        }
    }

    /// 覆盖 provider 标识。
    pub fn with_provider_id(mut self, id: impl Into<String>) -> Self {
        self.provider_id = ProviderId::new(id.into());
        self
    }
}

/// Google Gemini 原生 Provider。
pub struct GoogleProvider {
    config: GoogleConfig,
    client: HttpClient,
    credential: Option<ResolvedCredential>,
}

impl GoogleProvider {
    /// 构造适配器。`credential` 为 None 时不带 key（仅用于测试或受控环境）。
    pub fn new(
        config: GoogleConfig,
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

    /// 构造 streamGenerateContent 端点 URL：把 key 与 `alt=sse` 拼为 query 参数。
    fn stream_url(&self, model: &str) -> String {
        let base = self.config.base_url.trim_end_matches('/');
        match &self.credential {
            Some(cred) => format!(
                "{base}/v1beta/models/{model}:streamGenerateContent?alt=sse&key={}",
                cred.expose_secret()
            ),
            None => format!("{base}/v1beta/models/{model}:streamGenerateContent?alt=sse"),
        }
    }

    async fn drive_stream(
        &self,
        request: &CanonicalModelRequest,
        sink: &dyn ProviderEventSink,
        cancel: CancellationToken,
    ) -> Result<ModelResponseSummary, ProviderError> {
        // canonical → Gemini generateContent 请求体
        let body = crate::request::to_generate_content_body(request);
        let model_str = request.model.to_string();
        let url = self.stream_url(&model_str);

        // 认证信息只在 URL query 中；无额外 per-request 头。
        let mut byte_stream = self
            .client
            .post_stream_with_headers(&url, body, request.trace_id.as_deref(), &[], cancel.clone())
            .await?;

        sink.emit(ProviderStreamEvent::ResponseStarted {
            response_id: request.trace_id.clone(),
        })
        .await?;

        let mut sse = SseParser::new();
        let mut chunk_state = ChunkState::default();
        let mut summary = ModelResponseSummary {
            stop_reason: StopReason::Error,
            usage: TokenUsage::default(),
            response_id: None,
            provider_metadata: Value::Null,
        };
        let mut saw_completion = false;
        let mut finish_meta: Option<Value> = None;

        use futures::StreamExt;
        while let Some(item) = byte_stream.next().await {
            if cancel.is_cancelled() {
                return Err(ProviderError::cancelled("stream cancelled"));
            }
            let bytes = item?;
            for event in sse.feed(&bytes) {
                let data = event.data.trim();
                if data.is_empty() {
                    continue;
                }
                for ev in chunk_to_events(data, &mut chunk_state) {
                    apply_event(&ev, &mut summary, &mut saw_completion, &mut finish_meta);
                    sink.emit(ev).await?;
                }
            }
        }

        // 冲刷残留
        if let Some(event) = sse.finish() {
            let data = event.data.trim();
            if !data.is_empty() {
                for ev in chunk_to_events(data, &mut chunk_state) {
                    apply_event(&ev, &mut summary, &mut saw_completion, &mut finish_meta);
                    sink.emit(ev).await?;
                }
            }
        }

        if !saw_completion {
            return Err(ProviderError::new(
                ProviderErrorKind::StreamInterrupted,
                "stream ended without finishReason",
            ));
        }

        // 组装 provider_metadata：{ model, finishReason, ... }
        let mut meta = serde_json::Map::new();
        meta.insert("model".into(), Value::String(model_str));
        if let Some(m) = finish_meta {
            if let Some(obj) = m.as_object() {
                for (k, v) in obj {
                    meta.insert(k.clone(), v.clone());
                }
            }
        }
        summary.provider_metadata = Value::Object(meta);
        summary.response_id = request.trace_id.clone();
        Ok(summary)
    }
}

/// 把单个事件副作用应用到 summary（usage / stop_reason / metadata）。
fn apply_event(
    ev: &ProviderStreamEvent,
    summary: &mut ModelResponseSummary,
    saw_completion: &mut bool,
    finish_meta: &mut Option<Value>,
) {
    match ev {
        ProviderStreamEvent::UsageUpdated(u) => summary.usage = u.clone(),
        ProviderStreamEvent::ProviderMetadata(m) => *finish_meta = Some(m.clone()),
        ProviderStreamEvent::ResponseCompleted(stop) => {
            summary.stop_reason = stop.clone();
            *saw_completion = true;
        }
        _ => {}
    }
}

#[async_trait]
impl ModelProvider for GoogleProvider {
    fn id(&self) -> ProviderId {
        self.config.provider_id.clone()
    }

    async fn list_models(
        &self,
        _credential: Option<&ResolvedCredential>,
    ) -> Result<Vec<ModelDefinition>, ProviderError> {
        // 远端 /models 不返回能力信息，直接返回能力完整的内置目录。
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

/// Google Gemini 内置模型目录（含 thinking / image / tool 能力）。
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

    // Gemini 2.5 系：thinking + 视觉 + 工具 + 结构化输出 + prompt cache。
    let gemini25 = caps(true, true, true, true, true, true, true);
    // Gemini 2.0 Flash：视觉 + 工具 + 结构化，无 thinking。
    let gemini20 = caps(true, true, true, true, false, true, true);
    // Gemini 1.5 Pro：视觉 + 工具 + 结构化 + prompt cache。
    let gemini15 = caps(true, true, true, true, false, true, true);

    vec![
        def(
            "gemini-2.5-pro",
            "Gemini 2.5 Pro",
            2_097_152,
            8_192,
            gemini25.clone(),
        ),
        def(
            "gemini-2.5-flash",
            "Gemini 2.5 Flash",
            1_048_576,
            8_192,
            gemini25,
        ),
        def(
            "gemini-2.0-flash",
            "Gemini 2.0 Flash",
            1_048_576,
            8_192,
            gemini20,
        ),
        def(
            "gemini-1.5-pro",
            "Gemini 1.5 Pro",
            2_097_152,
            8_192,
            gemini15,
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_defaults_to_google_endpoint() {
        let config = GoogleConfig::default();
        assert_eq!(config.base_url, DEFAULT_BASE_URL);
        assert_eq!(config.provider_id, ProviderId::new("google"));
    }

    #[test]
    fn stream_url_embeds_key_as_query_param() {
        let config = GoogleConfig::new("https://example.test");
        let provider = GoogleProvider::new(
            config,
            Some(ResolvedCredential::new(
                provider_api::CredentialKind::ApiKey,
                "secret-key",
            )),
        )
        .expect("构造 adapter");
        let url = provider.stream_url("gemini-2.5-pro");
        assert!(url.contains("/v1beta/models/gemini-2.5-pro:streamGenerateContent"));
        assert!(url.contains("alt=sse"));
        assert!(url.contains("key=secret-key"));
    }

    #[test]
    fn builtin_catalog_has_thinking_and_image_models() {
        let models = builtin_models();
        let pro = models
            .iter()
            .find(|m| m.id == ModelId::new("gemini-2.5-pro"))
            .expect("gemini-2.5-pro present");
        assert!(pro.capabilities.thinking);
        assert!(pro.capabilities.image_input);
        assert!(models.iter().any(|m| m.capabilities.image_input));
    }
}
