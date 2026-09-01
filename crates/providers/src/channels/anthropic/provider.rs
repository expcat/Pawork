//! Anthropic Messages [`ModelProvider`] 实现。
//!
//! 认证头 `x-api-key` + `anthropic-version`；明文 secret 只在构造 header 时短暂
//! 出现，不持久化、不记录。`base_url` 必填，端点为 `{base_url}/v1/messages`。
//! [`ModelProvider::stream`] 在发 HTTP 前走 CapabilityNegotiator；未声明能力拒绝
//! 而非静默丢弃。thinking signature 经 ReasoningProtector 换成 ref-only
//! [`ReasoningItem`]。

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use pawork_domain::{CancellationToken, ModelId, ProviderId, StopReason, TokenUsage};
use pawork_domain::{
    CanonicalModelRequest, CapabilityFallback, CapabilityRequirements, ContentPart,
    ModelCapabilities, ModelDefinition, ModelProvider, ModelResponseSummary, ModelTransport,
    PromptCachePreference, ProviderError, ProviderErrorKind, ProviderEventSink,
    ProviderStreamEvent, ReasoningConfig, ReasoningEffort, ReasoningItem, ReasoningItemId,
    ReasoningStateCapability, ReasoningStateDescriptor, ResolvedCredential, ThinkingConfig,
    ThinkingLevel,
};
use serde_json::{json, Value};

use crate::memory_protector::InMemoryReasoningProtector;
use crate::negotiate::{clamp_reasoning_to_thinking, CapabilityNegotiator};
use crate::net::http::{HttpClient, HttpClientConfig};
use crate::net::sse::SseParser;
use crate::registry::{CapabilityEvidence, ModelRegistry};
use crate::{ReasoningProtectError, ReasoningProtector};

use super::request::{has_prompt_cache_breakpoint, to_messages_body_with_plan, MessagesWirePlan};
use super::stream::{parse_event, AnthropicStreamState, StreamOutput};
use super::ANTHROPIC_VERSION;

const MIN_THINKING_BUDGET_TOKENS: u64 = 1024;
const ANTHROPIC_MODEL_HINT: &str = "provider_hints.anthropic.model";

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
    reasoning_protector: Arc<dyn ReasoningProtector>,
    registry: Option<Arc<ModelRegistry>>,
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
            reasoning_protector: Arc::new(InMemoryReasoningProtector::default()),
            registry: None,
        })
    }

    pub fn with_reasoning_protector(mut self, protector: Arc<dyn ReasoningProtector>) -> Self {
        self.reasoning_protector = protector;
        self
    }

    pub fn with_registry(mut self, registry: Arc<ModelRegistry>) -> Self {
        self.registry = Some(registry);
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

    async fn drive_legacy_stream(
        &self,
        request: &CanonicalModelRequest,
        sink: &dyn ProviderEventSink,
        cancel: CancellationToken,
    ) -> Result<ModelResponseSummary, ProviderError> {
        let (body, _) = self.prepare_request(request).await?;
        let mut state = AnthropicStreamState::default();
        self.pump_messages(
            body,
            &request.model,
            request.trace_id.as_deref(),
            &mut state,
            sink,
            cancel,
        )
        .await
    }

    async fn prepare_request(
        &self,
        request: &CanonicalModelRequest,
    ) -> Result<(Value, MessagesWirePlan), ProviderError> {
        let evidence = self.capability_evidence(&request.model);
        let caps = evidence.merged();
        let requirements = requirements_from_request(request);
        let resolved = CapabilityNegotiator::negotiate(&evidence, &requirements);
        if let Some(reason) = first_reject(&resolved.fallback) {
            return Err(ProviderError::new(
                ProviderErrorKind::InvalidRequest,
                reason,
            ));
        }

        let write_cache = match request.prompt_cache {
            PromptCachePreference::Disabled => false,
            PromptCachePreference::Automatic => caps.prompt_cache,
            PromptCachePreference::Required => {
                if !caps.prompt_cache {
                    return Err(ProviderError::new(
                        ProviderErrorKind::InvalidRequest,
                        "prompt cache is required but not declared by model",
                    ));
                }
                true
            }
        };

        let thinking_on = request
            .thinking
            .as_ref()
            .is_some_and(|config| config.level != ThinkingLevel::Off)
            || request
                .reasoning
                .as_ref()
                .is_some_and(|config| config.requires_reasoning_support());
        let thinking = if request
            .reasoning
            .as_ref()
            .is_some_and(|config| config.requires_reasoning_support())
        {
            clamp_reasoning_to_thinking(request.reasoning.as_ref(), request.thinking.as_ref())
        } else {
            request.thinking.clone().unwrap_or(ThinkingConfig {
                level: ThinkingLevel::Off,
                budget_tokens: None,
            })
        };
        let thinking_budget = if thinking_on {
            Some(thinking_budget_tokens(&thinking))
        } else {
            None
        };
        if let Some(budget) = thinking_budget {
            if budget < MIN_THINKING_BUDGET_TOKENS {
                return Err(ProviderError::new(
                    ProviderErrorKind::InvalidRequest,
                    format!("thinking budget_tokens must be at least {MIN_THINKING_BUDGET_TOKENS}"),
                ));
            }
            if let Some(temperature) = request.temperature {
                if (temperature - 1.0).abs() > f64::EPSILON {
                    return Err(ProviderError::new(
                        ProviderErrorKind::InvalidRequest,
                        "thinking requires temperature=1.0",
                    ));
                }
            }
            if let Some(max_tokens) = request.max_output_tokens {
                if max_tokens <= budget {
                    return Err(ProviderError::new(
                        ProviderErrorKind::InvalidRequest,
                        "max_output_tokens must be greater than thinking budget_tokens",
                    ));
                }
            }
        }

        let resolved_thinking_blocks = if thinking_budget.is_some() {
            resolve_thinking_blocks(request, self.reasoning_protector.as_ref()).await?
        } else {
            Vec::new()
        };
        let plan = MessagesWirePlan {
            write_cache,
            thinking_budget,
            resolved_thinking_blocks,
        };
        let body = to_messages_body_with_plan(request, &plan);
        if request.prompt_cache == PromptCachePreference::Required
            && !has_prompt_cache_breakpoint(&body)
        {
            return Err(ProviderError::new(
                ProviderErrorKind::InvalidRequest,
                "prompt cache is required but request has no cacheable content",
            ));
        }
        Ok((body, plan))
    }

    fn capability_evidence(&self, model: &ModelId) -> CapabilityEvidence {
        if let Some(registry) = &self.registry {
            if let Some(evidence) = registry.capability_evidence(model.as_str()) {
                return evidence;
            }
        }
        if let Some(definition) = builtin_models()
            .into_iter()
            .find(|definition| definition.id == *model)
        {
            return CapabilityEvidence {
                model: definition.id,
                provider: Some(self.config.provider_id.clone()),
                static_declared: Some(definition.capabilities),
                probe_declared: None,
                override_declared: None,
            };
        }
        CapabilityEvidence {
            model: model.clone(),
            provider: Some(self.config.provider_id.clone()),
            static_declared: Some(messages_capabilities()),
            probe_declared: None,
            override_declared: None,
        }
    }

    async fn pump_messages(
        &self,
        body: Value,
        model: &ModelId,
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
                self.process_chunk(data, model, state, sink, &mut summary, &mut saw_completion)
                    .await?;
            }
        }

        if let Some(event) = sse.finish()? {
            let data = event.data.trim();
            if !data.is_empty() {
                self.process_chunk(data, model, state, sink, &mut summary, &mut saw_completion)
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
        model: &ModelId,
        state: &mut AnthropicStreamState,
        sink: &dyn ProviderEventSink,
        summary: &mut ModelResponseSummary,
        saw_completion: &mut bool,
    ) -> Result<(), ProviderError> {
        for output in parse_event(data, state) {
            let ev = match output {
                StreamOutput::Event(event) => event,
                StreamOutput::MappingError(error) => {
                    return Err(ProviderError::new(
                        ProviderErrorKind::MalformedResponse,
                        format!("server tool mapping unsupported: {error}"),
                    ));
                }
                StreamOutput::ReasoningError(error) => {
                    return Err(ProviderError::new(
                        ProviderErrorKind::MalformedResponse,
                        error,
                    ));
                }
                StreamOutput::PendingSignature {
                    id,
                    summary: item_summary,
                    payload,
                    redacted,
                } => {
                    let blob_ref = self
                        .reasoning_protector
                        .protect(&payload)
                        .await
                        .map_err(protect_error)?;
                    let mut opaque_metadata = std::collections::BTreeMap::new();
                    opaque_metadata.insert(
                        "item_type".into(),
                        json!(if redacted {
                            "redacted_thinking"
                        } else {
                            "thinking"
                        }),
                    );
                    ProviderStreamEvent::ReasoningItem(ReasoningItem {
                        id: ReasoningItemId::from(id),
                        summary: item_summary,
                        protected_blob_ref: blob_ref,
                        opaque_metadata,
                        continuation_metadata: std::collections::BTreeMap::from([(
                            ANTHROPIC_MODEL_HINT.into(),
                            json!(model.as_str()),
                        )]),
                    })
                }
            };
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

fn messages_capabilities() -> ModelCapabilities {
    ModelCapabilities {
        text: true,
        image_input: true,
        tool_calls: true,
        parallel_tool_calls: true,
        structured_output: true,
        prompt_cache: true,
        thinking: true,
        transport: ModelTransport::Messages,
        hosted_tool_tags: BTreeSet::new(),
        citations: false,
        reasoning: ReasoningStateCapability {
            state: ReasoningStateDescriptor {
                requires_signature: true,
                requires_encrypted: false,
                supports_interleaved: false,
            },
            supports_granular_effort: false,
        },
    }
}

/// Anthropic Messages 协议的静态内置模型目录（S5 registry 合并源）。
///
/// 调用方（app 装配层）在选中 Messages 协议时把它并入 ModelRegistry；
/// 本函数不做 Provider 名称分支，id 由调用方提供。
pub fn builtin_models() -> Vec<ModelDefinition> {
    let capabilities = messages_capabilities();
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

fn requirements_from_request(request: &CanonicalModelRequest) -> CapabilityRequirements {
    let mut required_tools = BTreeSet::new();
    for hosted in &request.hosted_tools {
        required_tools.insert(hosted.kind);
        required_tools.extend(hosted.capabilities.iter().copied());
    }
    for extension in &request.extensions {
        required_tools.extend(extension.capabilities.iter().copied());
        if extension.capabilities.is_empty() {
            required_tools.insert(pawork_domain::ToolCapabilityTag::ServerSideMcp);
        }
    }

    let reasoning = request
        .reasoning
        .clone()
        .or_else(|| {
            request.thinking.as_ref().and_then(|thinking| {
                if thinking.level == ThinkingLevel::Off {
                    None
                } else {
                    Some(ReasoningConfig {
                        effort: match thinking.level {
                            ThinkingLevel::Off => ReasoningEffort::None,
                            ThinkingLevel::Low => ReasoningEffort::Low,
                            ThinkingLevel::Medium => ReasoningEffort::Medium,
                            ThinkingLevel::High => ReasoningEffort::High,
                        },
                        state: ReasoningStateDescriptor {
                            requires_signature: true,
                            requires_encrypted: false,
                            supports_interleaved: false,
                        },
                    })
                }
            })
        })
        .map(|mut config| {
            if config.requires_reasoning_support() {
                config.state.requires_signature = true;
            }
            config
        });

    CapabilityRequirements {
        transport_pref: vec![ModelTransport::Messages],
        required_tools,
        reasoning,
        citations: !request.hosted_tools.is_empty(),
    }
}

fn first_reject(
    fallback: &std::collections::BTreeMap<String, CapabilityFallback>,
) -> Option<String> {
    fallback.values().find_map(|item| match item {
        CapabilityFallback::Reject(reason) => Some(reason.clone()),
        _ => None,
    })
}

fn thinking_budget_tokens(thinking: &ThinkingConfig) -> u64 {
    if let Some(budget) = thinking.budget_tokens {
        return budget;
    }
    match thinking.level {
        ThinkingLevel::Off => 0,
        ThinkingLevel::Low => 1024,
        ThinkingLevel::Medium => 2048,
        ThinkingLevel::High => 4096,
    }
}

async fn resolve_thinking_blocks(
    request: &CanonicalModelRequest,
    protector: &dyn ReasoningProtector,
) -> Result<Vec<Value>, ProviderError> {
    let mut blocks = Vec::new();
    for message in &request.messages {
        for part in &message.content {
            let ContentPart::Reasoning(item) = part else {
                continue;
            };
            if let Some(origin_model) = item.continuation_metadata.get(ANTHROPIC_MODEL_HINT) {
                let Some(origin_model) = origin_model.as_str() else {
                    return Err(malformed_thinking_block(
                        "model continuation hint must be a string",
                    ));
                };
                if origin_model != request.model.as_str() {
                    // 保持与 ContentPart::Reasoning 的位置对齐，request writer 跳过 Null。
                    blocks.push(Value::Null);
                    continue;
                }
            }
            let payload = protector
                .resolve(&item.protected_blob_ref)
                .await
                .map_err(protect_error)?;
            let value: Value = serde_json::from_slice(&payload).map_err(|_| {
                ProviderError::new(
                    ProviderErrorKind::MalformedResponse,
                    "protected Anthropic thinking block is not valid JSON",
                )
            })?;
            blocks.push(validate_thinking_block(value)?);
        }
    }
    Ok(blocks)
}

fn validate_thinking_block(mut value: Value) -> Result<Value, ProviderError> {
    let Some(object) = value.as_object_mut() else {
        return Err(malformed_thinking_block("must be a JSON object"));
    };
    match object.get("type").and_then(Value::as_str) {
        Some("thinking") => {
            if object.get("thinking").and_then(Value::as_str).is_none()
                || object
                    .get("signature")
                    .and_then(Value::as_str)
                    .is_none_or(str::is_empty)
            {
                return Err(malformed_thinking_block(
                    "thinking block requires thinking and non-empty signature strings",
                ));
            }
        }
        Some("redacted_thinking") => {
            if object
                .get("data")
                .and_then(Value::as_str)
                .is_none_or(str::is_empty)
            {
                return Err(malformed_thinking_block(
                    "redacted_thinking block requires a non-empty data string",
                ));
            }
            // R5 早期 producer 曾错误给 redacted block 带上 signature；回放前归一化。
            object.remove("signature");
        }
        _ => {
            return Err(malformed_thinking_block(
                "block type must be thinking or redacted_thinking",
            ));
        }
    }
    Ok(value)
}

fn malformed_thinking_block(detail: &str) -> ProviderError {
    ProviderError::new(
        ProviderErrorKind::MalformedResponse,
        format!("protected Anthropic thinking block {detail}"),
    )
}

fn protect_error(error: ReasoningProtectError) -> ProviderError {
    let kind = if error.is_corrupted() {
        ProviderErrorKind::MalformedResponse
    } else {
        ProviderErrorKind::ProviderUnavailable
    };
    ProviderError::new(kind, error.to_string())
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
            Some(ResolvedCredential::new(
                CredentialKind::ApiKey,
                "sk-ant-test",
            )),
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
        let provider =
            AnthropicProvider::new(AnthropicConfig::new("https://gateway.example"), None)
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
        assert!(models.iter().all(|model| model.capabilities.prompt_cache));
        assert!(models.iter().all(|model| model.capabilities.thinking));
        assert!(models
            .iter()
            .all(|model| model.capabilities.transport == ModelTransport::Messages));
        assert!(models
            .iter()
            .all(|model| model.capabilities.reasoning.state.requires_signature));
        assert!(models.iter().all(|model| !model.capabilities.citations));
        assert!(models
            .iter()
            .all(|model| model.capabilities.hosted_tool_tags.is_empty()));
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
            Some(ResolvedCredential::new(
                CredentialKind::ApiKey,
                "sk-ant-test",
            )),
        )
        .err()
        .expect("duplicate credential header must fail");
        assert_eq!(error.kind, ProviderErrorKind::InvalidRequest);
    }

    #[tokio::test]
    async fn required_prompt_cache_without_cap_is_rejected() {
        let registry = ModelRegistry::empty();
        registry.set_override(
            "unknown-model",
            ModelCapabilities {
                text: true,
                ..ModelCapabilities::default()
            },
        );
        let provider =
            AnthropicProvider::new(AnthropicConfig::new("https://gateway.example"), None)
                .expect("adapter")
                .with_registry(Arc::new(registry));
        let request = CanonicalModelRequest {
            request_id: pawork_domain::RequestId::from("r1"),
            model: ModelId::from("unknown-model"),
            messages: Vec::new(),
            tools: Vec::new(),
            hosted_tools: Vec::new(),
            extensions: Vec::new(),
            tool_choice: Default::default(),
            thinking: None,
            temperature: None,
            max_output_tokens: Some(128),
            stop_sequences: Vec::new(),
            response_format: Default::default(),
            prompt_cache: PromptCachePreference::Required,
            budget: Default::default(),
            provider_options: Default::default(),
            trace_id: None,
            reasoning: None,
        };
        let error = provider
            .prepare_request(&request)
            .await
            .expect_err("required cache without cap");
        assert_eq!(error.kind, ProviderErrorKind::InvalidRequest);
    }

    #[tokio::test]
    async fn required_prompt_cache_uses_message_fallback_or_rejects_empty_request() {
        let provider =
            AnthropicProvider::new(AnthropicConfig::new("https://gateway.example"), None)
                .expect("adapter");
        let mut request = sample_request();
        request.prompt_cache = PromptCachePreference::Required;
        let error = provider
            .prepare_request(&request)
            .await
            .expect_err("empty request has no cacheable block");
        assert_eq!(error.kind, ProviderErrorKind::InvalidRequest);

        request.messages.push(pawork_domain::Message {
            id: pawork_domain::MessageId::new("user"),
            role: pawork_domain::MessageRole::User,
            content: vec![ContentPart::Text(pawork_domain::TextContent {
                text: "cache me".into(),
            })],
            metadata: pawork_domain::MessageMetadata::default(),
        });
        let (body, _) = provider
            .prepare_request(&request)
            .await
            .expect("message fallback");
        assert_eq!(
            body["messages"][0]["content"][0]["cache_control"]["type"],
            "ephemeral"
        );
    }

    fn sample_request() -> CanonicalModelRequest {
        CanonicalModelRequest {
            request_id: pawork_domain::RequestId::from("r1"),
            model: ModelId::from("claude-3-5-sonnet"),
            messages: Vec::new(),
            tools: Vec::new(),
            hosted_tools: Vec::new(),
            extensions: Vec::new(),
            tool_choice: Default::default(),
            thinking: None,
            temperature: None,
            max_output_tokens: Some(8192),
            stop_sequences: Vec::new(),
            response_format: Default::default(),
            prompt_cache: PromptCachePreference::Automatic,
            budget: Default::default(),
            provider_options: Default::default(),
            trace_id: None,
            reasoning: None,
        }
    }

    #[tokio::test]
    async fn thinking_rejects_invalid_budget_temperature_and_max_tokens() {
        let provider =
            AnthropicProvider::new(AnthropicConfig::new("https://gateway.example"), None)
                .expect("adapter");
        let mut request = sample_request();
        request.thinking = Some(ThinkingConfig {
            level: ThinkingLevel::High,
            budget_tokens: Some(1024),
        });
        request.temperature = Some(0.2);
        let error = provider
            .prepare_request(&request)
            .await
            .expect_err("temperature");
        assert_eq!(error.kind, ProviderErrorKind::InvalidRequest);

        request.temperature = Some(1.0);
        request.max_output_tokens = Some(1024);
        let error = provider
            .prepare_request(&request)
            .await
            .expect_err("max tokens");
        assert_eq!(error.kind, ProviderErrorKind::InvalidRequest);

        request.max_output_tokens = Some(8192);
        request.thinking.as_mut().expect("thinking").budget_tokens = Some(1023);
        let error = provider
            .prepare_request(&request)
            .await
            .expect_err("minimum thinking budget");
        assert_eq!(error.kind, ProviderErrorKind::InvalidRequest);
        assert!(error.message.contains("at least 1024"));
    }

    #[tokio::test]
    async fn thinking_high_writes_default_budget_and_automatic_cache() {
        let provider =
            AnthropicProvider::new(AnthropicConfig::new("https://gateway.example"), None)
                .expect("adapter");
        let mut request = sample_request();
        request.thinking = Some(ThinkingConfig {
            level: ThinkingLevel::High,
            budget_tokens: None,
        });
        request.temperature = Some(1.0);
        request.max_output_tokens = None;
        request.messages.push(pawork_domain::Message {
            id: pawork_domain::MessageId::new("sys"),
            role: pawork_domain::MessageRole::System,
            content: vec![pawork_domain::ContentPart::Text(
                pawork_domain::TextContent { text: "sys".into() },
            )],
            metadata: pawork_domain::MessageMetadata::default(),
        });
        let (body, plan) = provider.prepare_request(&request).await.expect("plan");
        assert_eq!(plan.thinking_budget, Some(4096));
        assert!(plan.write_cache);
        assert_eq!(
            body["thinking"],
            serde_json::json!({"type":"enabled","budget_tokens":4096})
        );
        assert_eq!(body["system"]["cache_control"]["type"], "ephemeral");
        assert_eq!(body["max_tokens"], 4097);
    }

    #[tokio::test]
    async fn appender_shaped_thinking_replays_signed_block_only() {
        use pawork_domain::{
            Message, MessageId, MessageMetadata, MessageRole, ReasoningItem, ReasoningItemId,
            TextContent,
        };

        let protector = InMemoryReasoningProtector::default();
        let payload = serde_json::to_vec(&serde_json::json!({
            "type": "thinking",
            "thinking": "plan",
            "signature": "sig-secret",
        }))
        .expect("payload");
        let blob_ref = protector.protect(&payload).await.expect("protect");
        let provider =
            AnthropicProvider::new(AnthropicConfig::new("https://gateway.example"), None)
                .expect("adapter")
                .with_reasoning_protector(std::sync::Arc::new(protector));

        let mut request = sample_request();
        request.temperature = Some(1.0);
        request.thinking = Some(ThinkingConfig {
            level: ThinkingLevel::High,
            budget_tokens: Some(1024),
        });
        request.messages.push(Message {
            id: MessageId::new("asst"),
            role: MessageRole::Assistant,
            content: vec![
                pawork_domain::ContentPart::Thinking(pawork_domain::ThinkingContent {
                    text: "plan".into(),
                    reasoning_item_id: Some(ReasoningItemId::from("th_1")),
                    redacted: false,
                }),
                pawork_domain::ContentPart::Reasoning(ReasoningItem {
                    id: ReasoningItemId::from("th_1"),
                    summary: Some("plan".into()),
                    protected_blob_ref: blob_ref.clone(),
                    opaque_metadata: Default::default(),
                    continuation_metadata: Default::default(),
                }),
                pawork_domain::ContentPart::Text(TextContent {
                    text: "hello".into(),
                }),
            ],
            metadata: MessageMetadata::default(),
        });

        let (body, plan) = provider.prepare_request(&request).await.expect("plan");
        assert_eq!(plan.thinking_budget, Some(1024));
        let content = body["messages"][0]["content"].as_array().expect("content");
        let thinking_blocks: Vec<_> = content
            .iter()
            .filter(|block| block["type"] == "thinking")
            .collect();
        assert_eq!(thinking_blocks.len(), 1);
        assert_eq!(thinking_blocks[0]["thinking"], "plan");
        assert_eq!(thinking_blocks[0]["signature"], "sig-secret");
        assert!(!format!("{body}").contains("visible-thought"));

        let ContentPart::Reasoning(item) = &mut request.messages[0].content[1] else {
            panic!("reasoning item");
        };
        item.continuation_metadata
            .insert(ANTHROPIC_MODEL_HINT.into(), json!("claude-3-5-sonnet"));
        request.model = ModelId::from("claude-3-5-haiku");
        let (cross_model_body, cross_model_plan) = provider
            .prepare_request(&request)
            .await
            .expect("cross-model continuation is omitted");
        assert_eq!(cross_model_plan.resolved_thinking_blocks, vec![Value::Null]);
        assert!(cross_model_body["messages"][0]["content"]
            .as_array()
            .expect("content")
            .iter()
            .all(|block| block["type"] != "thinking"));

        request.thinking = None;
        let (body_off, plan_off) = provider.prepare_request(&request).await.expect("off");
        assert!(plan_off.thinking_budget.is_none());
        let content_off = body_off["messages"][0]["content"]
            .as_array()
            .expect("content");
        assert!(content_off.iter().all(|block| block["type"] != "thinking"));
    }

    #[tokio::test]
    async fn thinking_off_does_not_resolve_unavailable_continuation() {
        let provider =
            AnthropicProvider::new(AnthropicConfig::new("https://gateway.example"), None)
                .expect("adapter");
        let mut request = sample_request();
        request.messages.push(pawork_domain::Message {
            id: pawork_domain::MessageId::new("asst"),
            role: pawork_domain::MessageRole::Assistant,
            content: vec![ContentPart::Reasoning(ReasoningItem {
                id: ReasoningItemId::from("th-missing"),
                summary: None,
                protected_blob_ref: pawork_domain::ProtectedBlobRef::from("missing"),
                opaque_metadata: Default::default(),
                continuation_metadata: Default::default(),
            })],
            metadata: pawork_domain::MessageMetadata::default(),
        });

        let (body, plan) = provider
            .prepare_request(&request)
            .await
            .expect("thinking off");
        assert!(plan.resolved_thinking_blocks.is_empty());
        assert!(body.get("thinking").is_none());
        assert!(body["messages"][0]["content"]
            .as_array()
            .expect("content")
            .iter()
            .all(|block| block["type"] != "thinking"));
    }

    #[tokio::test]
    async fn malformed_protected_thinking_block_fails_closed() {
        let protector = InMemoryReasoningProtector::default();
        let blob_ref = protector.protect(b"not-json").await.expect("protect");
        let provider =
            AnthropicProvider::new(AnthropicConfig::new("https://gateway.example"), None)
                .expect("adapter")
                .with_reasoning_protector(Arc::new(protector));
        let mut request = sample_request();
        request.temperature = Some(1.0);
        request.thinking = Some(ThinkingConfig {
            level: ThinkingLevel::High,
            budget_tokens: Some(1024),
        });
        request.messages.push(pawork_domain::Message {
            id: pawork_domain::MessageId::new("asst"),
            role: pawork_domain::MessageRole::Assistant,
            content: vec![ContentPart::Reasoning(ReasoningItem {
                id: ReasoningItemId::from("th-invalid"),
                summary: None,
                protected_blob_ref: blob_ref,
                opaque_metadata: Default::default(),
                continuation_metadata: Default::default(),
            })],
            metadata: pawork_domain::MessageMetadata::default(),
        });

        let error = provider
            .prepare_request(&request)
            .await
            .expect_err("malformed protected payload");
        assert_eq!(error.kind, ProviderErrorKind::MalformedResponse);
        assert!(!error.message.contains("not-json"));
    }
}
