//! Anthropic 原生 [`ModelProvider`](provider_api::ModelProvider) 实现。
//!
//! 认证头 `x-api-key: <secret>` + `anthropic-version: 2023-06-01`，明文 secret
//! 只在构造 header 时短暂出现，不持久化、不记录。流式响应由 [`SseParser`] 驱动，
//! 经 [`event_to_events`](crate::stream::event_to_events) 映射为 canonical 事件。

use std::collections::BTreeMap;
use std::time::Duration;

use agent_domain::{
    CancellationToken, ContentPart, ModelId, ProviderId, ReasoningItemId, StopReason, TokenUsage,
    ToolCapabilityTag, TranscriptItem,
};
use async_trait::async_trait;
use provider_api::{
    CanonicalModelRequest, ModelCapabilities, ModelDefinition, ModelProvider, ModelResponseSummary,
    ModelTransport, ProviderError, ProviderErrorKind, ProviderEventSink, ProviderStreamEvent,
    ReasoningStateCapability, ReasoningStateDescriptor, ResolvedCredential,
};
use provider_runtime::http::{HttpClient, HttpClientConfig};
use provider_runtime::negotiate::clamp_reasoning_to_thinking;
use provider_runtime::sse::SseParser;
use serde_json::Value;

use crate::modern::{
    resolve, server_tool_whitelist, to_modern_messages_body, ReasoningContinuationStore,
    ThinkingPlan, TransportChoice,
};
use crate::reasoning::{build_reasoning_item, extract_thinking_payload, AnthropicThinkingPayload};
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
    /// P15-3 现代路径的 reasoning 续传不透明存取（缺省 None = 无法保护
    /// thinking signature，现代路径捕获到 signature 时 fail-closed）。
    continuation_store: Option<ReasoningContinuationStore>,
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
            continuation_store: None,
        })
    }

    /// 注入 reasoning 续传不透明 handle（P15-7 / ADR-032）。
    ///
    /// 接线方（engine / app-service）用 `provider-runtime::reasoning::
    /// ReasoningStateBridge` + `BlobScope` 构造两个闭包；adapter 不接触
    /// 加密存储实现。
    pub fn with_reasoning_continuation(mut self, store: ReasoningContinuationStore) -> Self {
        self.continuation_store = Some(store);
        self
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

    /// P6-2 基线路径：clamp reasoning → thinking budget，非保留 note 透出。
    async fn drive_legacy_stream(
        &self,
        request: &CanonicalModelRequest,
        notes: &[String],
        sink: &dyn ProviderEventSink,
        cancel: CancellationToken,
    ) -> Result<ModelResponseSummary, ProviderError> {
        for note in notes {
            sink.emit(ProviderStreamEvent::ProviderMetadata(
                serde_json::json!({ "degradation": note }),
            ))
            .await?;
        }
        let mut legacy_request = request.clone();
        if request.reasoning.is_some() {
            // 复用 clamp_reasoning_to_thinking：XHigh/Max → High，effort 优先。
            legacy_request.thinking = Some(clamp_reasoning_to_thinking(
                request.reasoning.as_ref(),
                request.thinking.as_ref(),
            ));
        }
        let body = crate::request::to_messages_body(&legacy_request);
        let mut state = AnthropicStreamState::default();
        self.pump_messages(body, request.trace_id.as_deref(), &mut state, sink, cancel)
            .await
    }

    /// P15-3 现代路径：output_config / effort / adaptive thinking / server tools。
    async fn drive_modern_stream(
        &self,
        request: &CanonicalModelRequest,
        resolution: &crate::modern::ModernResolution,
        sink: &dyn ProviderEventSink,
        cancel: CancellationToken,
    ) -> Result<ModelResponseSummary, ProviderError> {
        for note in &resolution.notes {
            sink.emit(ProviderStreamEvent::ProviderMetadata(
                serde_json::json!({ "degradation": note }),
            ))
            .await?;
        }
        if matches!(resolution.thinking, ThinkingPlan::Adaptive { .. })
            && self.continuation_store.is_none()
        {
            return Err(ProviderError::new(
                ProviderErrorKind::InvalidRequest,
                "adaptive thinking requires a reasoning continuation store",
            ));
        }

        // 取回会话中的 reasoning 续传载荷（不透明字节 → AnthropicThinkingPayload）。
        let mut continuations: BTreeMap<ReasoningItemId, AnthropicThinkingPayload> =
            BTreeMap::new();
        for message in &request.messages {
            for part in &message.content {
                if let ContentPart::Reasoning(item) = part {
                    let Some(store) = &self.continuation_store else {
                        return Err(ProviderError::new(
                            ProviderErrorKind::InvalidRequest,
                            "modern Messages reasoning continuation requires a continuation store",
                        ));
                    };
                    let bytes = store
                        .resolve(&item.protected_blob_ref)
                        .await
                        .map_err(|error| {
                            ProviderError::new(
                                ProviderErrorKind::InvalidRequest,
                                format!("cannot resolve reasoning continuation: {error}"),
                            )
                        })?;
                    let payload: AnthropicThinkingPayload = serde_json::from_slice(&bytes)
                        .map_err(|error| {
                            ProviderError::new(
                                ProviderErrorKind::InvalidRequest,
                                format!("corrupt continuation payload: {error}"),
                            )
                        })?;
                    continuations.insert(item.id.clone(), payload);
                }
            }
        }

        let body =
            to_modern_messages_body(request, resolution, &continuations).map_err(|error| {
                ProviderError::new(ProviderErrorKind::InvalidRequest, error.to_string())
            })?;
        let mut state = AnthropicStreamState {
            capture_signatures: true,
            server_tool_names: server_tool_whitelist(request),
            ..AnthropicStreamState::default()
        };
        self.pump_messages(body, request.trace_id.as_deref(), &mut state, sink, cancel)
            .await
    }

    /// 公共 SSE 泵：映射事件、保护 thinking signature、续接 transcript 信封。
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
        let mut server_tool_events: Vec<agent_domain::ServerToolEvent> = Vec::new();

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
                self.process_chunk(
                    data,
                    state,
                    sink,
                    &mut summary,
                    &mut saw_completion,
                    &mut server_tool_events,
                )
                .await?;
            }
        }

        if let Some(event) = sse.finish()? {
            let data = event.data.trim();
            if !data.is_empty() {
                self.process_chunk(
                    data,
                    state,
                    sink,
                    &mut summary,
                    &mut saw_completion,
                    &mut server_tool_events,
                )
                .await?;
            }
        }

        // P15-5：server tool 续接走 ProviderTranscript（provider-neutral）。
        if !server_tool_events.is_empty() {
            sink.emit(ProviderStreamEvent::TranscriptEnvelope(
                agent_domain::ProviderTranscriptEnvelope {
                    items: server_tool_events
                        .into_iter()
                        .map(TranscriptItem::ServerTool)
                        .collect(),
                    cursor: None,
                    continuation_reference: summary.response_id.clone(),
                },
            ))
            .await?;
        }

        if !saw_completion {
            return Err(ProviderError::new(
                provider_api::ProviderErrorKind::StreamInterrupted,
                "anthropic stream ended without message_stop",
            ));
        }

        Ok(summary)
    }

    /// 处理一条 SSE data：事件映射 + thinking signature 保护 + 事件收集。
    async fn process_chunk(
        &self,
        data: &str,
        state: &mut AnthropicStreamState,
        sink: &dyn ProviderEventSink,
        summary: &mut ModelResponseSummary,
        saw_completion: &mut bool,
        server_tool_events: &mut Vec<agent_domain::ServerToolEvent>,
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
                ProviderStreamEvent::ServerTool(event) => {
                    server_tool_events.push(event.clone());
                }
                _ => {}
            }
            sink.emit(ev).await?;
        }

        // 捕获到的 thinking signature → 保护为不透明 blob → ReasoningItem。
        let pending = state.drain_pending_thinking();
        if !pending.is_empty() {
            let Some(store) = &self.continuation_store else {
                return Err(ProviderError::new(
                    ProviderErrorKind::InvalidRequest,
                    "anthropic thinking signature captured but no continuation store configured",
                ));
            };
            for block in pending {
                let payload = extract_thinking_payload(&block).map_err(|error| {
                    ProviderError::new(ProviderErrorKind::MalformedResponse, error.to_string())
                })?;
                state.reasoning_seq += 1;
                let item_id =
                    ReasoningItemId::from(format!("anthropic-thinking-{}", state.reasoning_seq));
                let encoded = serde_json::to_vec(&payload).map_err(|error| {
                    ProviderError::new(
                        ProviderErrorKind::Unknown,
                        format!("serialize continuation payload: {error}"),
                    )
                })?;
                let blob_ref = store.protect(encoded).await.map_err(|error| {
                    ProviderError::new(
                        ProviderErrorKind::Unknown,
                        format!("protect reasoning continuation: {error}"),
                    )
                })?;
                let item = build_reasoning_item(item_id, blob_ref, &payload);
                sink.emit(ProviderStreamEvent::ReasoningItem(item)).await?;
            }
        }
        Ok(())
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
        let caps = catalog_capabilities(&request.model);
        let (choice, resolution) = resolve(&request, caps.as_ref());
        match choice {
            TransportChoice::Legacy => {
                self.drive_legacy_stream(&request, &resolution.notes, sink, cancel)
                    .await
            }
            TransportChoice::Modern => {
                self.drive_modern_stream(&request, &resolution, sink, cancel)
                    .await
            }
        }
    }
}

/// Anthropic 内置模型目录（含 thinking / image / tool / cache 能力）。
/// 数据快照：2026-08-09；目录更新作为显式跟踪项手动执行。
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
            ..ModelCapabilities::default()
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

    // P15-3：现代 Messages 模型（adapter contract 声明：transport / hosted
    // tools / citations / reasoning 维度；P15-8 协商消费这些声明）。
    let modern_full = ModelCapabilities {
        text: true,
        image_input: true,
        tool_calls: true,
        parallel_tool_calls: true,
        thinking: true,
        structured_output: true,
        prompt_cache: true,
        transport: ModelTransport::Messages,
        hosted_tool_tags: [
            ToolCapabilityTag::WebSearch,
            ToolCapabilityTag::WebFetch,
            ToolCapabilityTag::CodeExecution,
            ToolCapabilityTag::HostedShell,
            ToolCapabilityTag::ProviderApplyPatch,
            ToolCapabilityTag::ComputerUse,
            ToolCapabilityTag::ToolSearch,
            ToolCapabilityTag::Memory,
            ToolCapabilityTag::ServerSideMcp,
        ]
        .into_iter()
        .collect(),
        citations: true,
        reasoning: ReasoningStateCapability {
            state: ReasoningStateDescriptor {
                requires_signature: true,
                requires_encrypted: true,
                supports_interleaved: true,
            },
            supports_granular_effort: true,
        },
    };

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
        def(
            "claude-sonnet-4-5",
            "Claude Sonnet 4.5",
            200_000,
            64_000,
            modern_full.clone(),
        ),
        def(
            "claude-opus-4-1",
            "Claude Opus 4.1",
            200_000,
            32_000,
            modern_full.clone(),
        ),
        def(
            "claude-haiku-4-5",
            "Claude Haiku 4.5",
            200_000,
            64_000,
            modern_full,
        ),
    ]
}

/// 按模型 id 查 adapter contract 能力声明（供 [`resolve`] 选择传输路径）。
fn catalog_capabilities(model: &ModelId) -> Option<ModelCapabilities> {
    builtin_models()
        .into_iter()
        .find(|definition| definition.id == *model)
        .map(|definition| definition.capabilities)
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
