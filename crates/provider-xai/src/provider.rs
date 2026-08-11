//! xAI Grok [`ModelProvider`](provider_api::ModelProvider) 实现。
//!
//! 双传输并存（P15-4）：经 P15-8 能力协商选择，模型声明 `transport = Responses`
//! 时走 xAI Responses（`/v1/responses`）现代路径，否则降级到 P6-10 Chat
//! Completions（复用 [`OpenAiCompatibleProvider`](provider_openai_compatible::OpenAiCompatibleProvider)
//! 作为流式引擎）。两条路径都固定 xAI 官方端点与 `xai` provider 标识，鉴权复用
//! P6-10 的 API Key / OAuth bearer。
//!
//! Responses 路径不在 Core 走任何 xAI 名称分支：transport 选择只由
//! [`CapabilityNegotiator`](provider_runtime::negotiate::CapabilityNegotiator)（纯函数，
//! 读模型能力声明）驱动；reasoning `encrypted_content` 只经
//! [`ReasoningProtector`](crate::responses::ReasoningProtector) 边界往返（ADR-032）。

use std::sync::Arc;
use std::time::Duration;

use agent_domain::{CancellationToken, ModelId, ProviderId, StopReason, TokenUsage};
use async_trait::async_trait;
use model_registry::CapabilityEvidence;
use provider_api::{
    CanonicalModelRequest, CredentialKind, ModelCapabilities, ModelDefinition, ModelProvider,
    ModelResponseSummary, ProviderError, ProviderErrorKind, ProviderEventSink,
    ProviderStreamEvent, ResolvedCapabilities, ResolvedCredential,
};
use provider_openai_compatible::{OpenAiCompatibleConfig, OpenAiCompatibleProvider};
use provider_runtime::http::{HttpClient, HttpClientConfig};
use provider_runtime::negotiate::CapabilityNegotiator;
use provider_runtime::sse::SseParser;

use crate::reasoning::{parse_responses_reasoning, to_reasoning_item};
use crate::responses::{
    normalize_responses_error, requirements_from_request, resolve_reasoning_inputs,
    to_responses_body, AcceptedResponsesTools, InMemoryReasoningProtector, ReasoningProtector,
    ResponsesAssemblyEvent, ResponsesStreamAssembler,
};
use crate::DEFAULT_BASE_URL;

/// xAI adapter configuration（Chat Completions + Responses 共用）。
#[derive(Clone, Debug)]
pub struct XaiConfig {
    /// Base URL, defaulting to `https://api.x.ai/v1`.
    pub base_url: String,
    /// HTTP client configuration.
    pub http: HttpClientConfig,
    /// Connection and idle-read timeout override.
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
    /// Uses a custom OpenAI-compatible base URL while retaining provider id `xai`.
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            ..Self::default()
        }
    }
}

/// xAI Grok provider：双传输并存（P15-4）。
pub struct XaiProvider {
    inner: OpenAiCompatibleProvider,
    responses_client: HttpClient,
    base_url: String,
    provider_id: ProviderId,
    credential: Option<ResolvedCredential>,
    reasoning_protector: Option<Arc<dyn ReasoningProtector>>,
}

impl XaiProvider {
    /// Builds the adapter with either an API key or an OAuth bearer access token.
    pub fn new(
        config: XaiConfig,
        credential: Option<ResolvedCredential>,
    ) -> Result<Self, ProviderError> {
        if let Some(credential) = &credential {
            if !matches!(
                credential.kind(),
                CredentialKind::ApiKey | CredentialKind::OAuthBearer
            ) {
                return Err(ProviderError::new(
                    ProviderErrorKind::Authentication,
                    "xAI requires an API key or OAuth bearer credential",
                ));
            }
        }

        let compatible = OpenAiCompatibleConfig {
            base_url: config.base_url.clone(),
            provider_id: ProviderId::new("xai"),
            http: Self::http_config(&config),
            request_timeout: None,
        };
        let responses_client = HttpClient::new(Self::http_config(&config))?;
        Ok(Self {
            inner: OpenAiCompatibleProvider::new(compatible, credential.clone())?,
            responses_client,
            base_url: config.base_url,
            provider_id: ProviderId::new("xai"),
            credential,
            reasoning_protector: None,
        })
    }

    /// 注入 reasoning continuation 的 Protected Blob Store 边界实现（P15-7 host
    /// 接入点）。未注入时使用进程内 in-memory 默认 protector（仅保证进程内回放）。
    pub fn with_reasoning_protector(mut self, protector: Arc<dyn ReasoningProtector>) -> Self {
        self.reasoning_protector = Some(protector);
        self
    }

    fn http_config(config: &XaiConfig) -> HttpClientConfig {
        match config.request_timeout {
            Some(timeout) => {
                let mut c = config.http.clone();
                c.timeout = Some(timeout);
                c
            }
            None => config.http.clone(),
        }
    }

    fn auth_header(&self) -> Option<(String, String)> {
        self.credential.as_ref().map(|cred| {
            (
                "Authorization".to_string(),
                format!("Bearer {}", cred.expose_secret()),
            )
        })
    }

    fn responses_url(&self) -> String {
        format!("{}/responses", self.base_url.trim_end_matches('/'))
    }

    /// 协商 transport 与能力：以内置目录的模型能力声明为 evidence，请求能力
    /// 由 [`crate::responses::requirements_from_request`] 折叠。纯函数，不触网。
    fn resolve_capabilities(&self, request: &CanonicalModelRequest) -> ResolvedCapabilities {
        let requirements = requirements_from_request(request);
        let static_caps = builtin_models()
            .into_iter()
            .find(|model| model.id == request.model)
            .map(|model| model.capabilities);
        let evidence = CapabilityEvidence {
            model: request.model.clone(),
            provider: Some(self.provider_id.clone()),
            static_declared: static_caps,
            probe_declared: None,
            override_declared: None,
        };
        CapabilityNegotiator::negotiate(&evidence, &requirements)
    }

    /// Responses 传输路径：构造请求 → POST `/responses` → SSE 组装 → 归一事件。
    async fn stream_responses(
        &self,
        request: CanonicalModelRequest,
        sink: &dyn ProviderEventSink,
        cancel: CancellationToken,
    ) -> Result<ModelResponseSummary, ProviderError> {
        let resolved = self.resolve_capabilities(&request);
        let accepted = AcceptedResponsesTools::from_supported(&resolved.supported);

        // reasoning protector：未注入则用进程内默认实现（保证事件只携带引用）。
        let default_protector: Arc<dyn ReasoningProtector>;
        let protector: &dyn ReasoningProtector = match &self.reasoning_protector {
            Some(arc) => arc.as_ref(),
            None => {
                default_protector = Arc::new(InMemoryReasoningProtector::default());
                default_protector.as_ref()
            }
        };

        // 解析历史 reasoning items → Responses input（经 protector 解密）。
        let (reasoning_inputs, _warnings) = resolve_reasoning_inputs(&request, protector).await;
        let body = to_responses_body(&request, reasoning_inputs, &accepted);

        // 认证头（明文 secret 只在此短暂存在，不持久化、不记录）。
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

        let mut byte_stream = self
            .responses_client
            .post_stream_with_headers(
                &self.responses_url(),
                body,
                request.trace_id.as_deref(),
                per_request_headers,
                cancel.clone(),
            )
            .await
            .map_err(normalize_responses_error)?;

        // 用 SSE 解析器消费字节流。
        let mut sse = SseParser::new();
        let mut assembler = ResponsesStreamAssembler::new();
        let mut saw_completion = false;
        let mut summary = ModelResponseSummary {
            stop_reason: StopReason::Error,
            usage: TokenUsage::default(),
            response_id: None,
            provider_metadata: serde_json::Value::Null,
        };

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
                for assembly_event in assembler.feed(data) {
                    self.emit_assembly_event(assembly_event, protector, sink, &mut summary)
                        .await?;
                    if matches!(
                        summary.stop_reason,
                        StopReason::Completed | StopReason::ToolUse | StopReason::MaxTokens
                    ) {
                        saw_completion = true;
                    }
                }
            }
        }

        // 冲刷残留 SSE。
        if let Some(event) = sse.finish()? {
            let data = event.data.trim();
            if !data.is_empty() {
                for assembly_event in assembler.feed(data) {
                    self.emit_assembly_event(assembly_event, protector, sink, &mut summary)
                        .await?;
                    if matches!(
                        summary.stop_reason,
                        StopReason::Completed | StopReason::ToolUse | StopReason::MaxTokens
                    ) {
                        saw_completion = true;
                    }
                }
            }
        }

        let final_state = assembler.finish();
        if let Some(id) = final_state.response_id {
            summary.response_id = Some(id);
        }
        if final_state.usage != TokenUsage::default() {
            summary.usage = final_state.usage;
        }
        if !saw_completion && final_state.completed {
            saw_completion = true;
        }
        if !saw_completion {
            return Err(ProviderError::new(
                ProviderErrorKind::StreamInterrupted,
                "xai responses stream ended without completion event",
            ));
        }
        Ok(summary)
    }

    /// 把单条组装事件落到 sink：reasoning candidate 先经 protector 保护，再以
    /// `ReasoningItem` 事件发射（只携带 Protected Blob 引用）。
    async fn emit_assembly_event(
        &self,
        assembly_event: ResponsesAssemblyEvent,
        protector: &dyn ReasoningProtector,
        sink: &dyn ProviderEventSink,
        summary: &mut ModelResponseSummary,
    ) -> Result<(), ProviderError> {
        match assembly_event {
            ResponsesAssemblyEvent::Canonical(event) => {
                match &event {
                    ProviderStreamEvent::UsageUpdated(usage) => summary.usage = usage.clone(),
                    ProviderStreamEvent::ResponseCompleted(stop) => {
                        summary.stop_reason = stop.clone();
                    }
                    _ => {}
                }
                sink.emit(event).await?;
            }
            ResponsesAssemblyEvent::ReasoningOutputItem { wire } => {
                // encrypted_content 必须存在才能构造 continuation；缺省则跳过。
                match parse_responses_reasoning(&wire) {
                    Ok(parsed) => match parsed.protected() {
                        Some(continuation) => {
                            let payload = continuation.as_str().as_bytes().to_vec();
                            let blob_ref = protector.protect(&payload).await.map_err(|error| {
                                ProviderError::new(
                                    ProviderErrorKind::Unknown,
                                    format!("reasoning protect failed: {error}"),
                                )
                            })?;
                            match to_reasoning_item(
                                parsed,
                                agent_domain::ProtectedBlobRef::from(blob_ref),
                            ) {
                                Ok(item) => {
                                    sink.emit(ProviderStreamEvent::ReasoningItem(item)).await?;
                                }
                                Err(error) => {
                                    tracing::debug!(
                                        error = %error,
                                        "unmapped xAI Responses reasoning item, skipped"
                                    );
                                }
                            }
                        }
                        None => {
                            tracing::debug!(
                                "xAI reasoning item without encrypted_content, skipped"
                            );
                        }
                    },
                    Err(error) => {
                        tracing::debug!(
                            error = %error,
                            "xAI reasoning item extraction failed, skipped"
                        );
                    }
                }
            }
        }
        Ok(())
    }
}

#[async_trait]
impl ModelProvider for XaiProvider {
    fn id(&self) -> ProviderId {
        self.provider_id.clone()
    }

    async fn list_models(
        &self,
        _credential: Option<&ResolvedCredential>,
    ) -> Result<Vec<ModelDefinition>, ProviderError> {
        // xAI 远端 /models 不返回能力信息，直接返回能力完整的内置目录。
        Ok(builtin_models())
    }

    async fn stream(
        &self,
        request: CanonicalModelRequest,
        sink: &dyn ProviderEventSink,
        cancel: CancellationToken,
    ) -> Result<ModelResponseSummary, ProviderError> {
        // transport 选择只由模型能力声明驱动（P15-8），不读 Provider 名。
        let resolved = self.resolve_capabilities(&request);
        if resolved.chosen_transport == provider_api::ModelTransport::Responses {
            self.stream_responses(request, sink, cancel).await
        } else {
            // 降级到 P6-10 Chat Completions。
            self.inner.stream(request, sink, cancel).await
        }
    }
}

/// xAI 内置模型目录（含 reasoning / Live Search / Code / MCP 能力）。
/// 数据快照：2026-08-12；目录更新作为显式跟踪项手动执行。
pub fn builtin_models() -> Vec<ModelDefinition> {
    use agent_domain::ToolCapabilityTag as T;
    use provider_api::{ModelTransport, ReasoningStateCapability, ReasoningStateDescriptor};
    use std::collections::BTreeSet;

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

    // Grok 4 系：声明 Responses 现代传输路径（P15-4）。hosted tool 标签 +
    // citations + reasoning 加密 continuation 由 negotiator 据此选择 Responses。
    let mut grok4 = caps(true, true, true, true, true, true, true);
    grok4.transport = ModelTransport::Responses;
    grok4.citations = true;
    grok4.hosted_tool_tags = [
        T::WebSearch,
        T::XSearch,
        T::FileOrCollectionSearch,
        T::CodeExecution,
        T::ServerSideMcp,
    ]
    .into_iter()
    .collect::<BTreeSet<_>>();
    grok4.reasoning = ReasoningStateCapability {
        state: ReasoningStateDescriptor {
            requires_signature: false,
            requires_encrypted: true,
            supports_interleaved: true,
        },
        supports_granular_effort: true,
    };

    // Grok 4 fast：同 Responses 能力，不支持细粒度 effort（XHigh/Max clamp）。
    let grok4_fast = {
        let mut c = grok4.clone();
        c.reasoning.supports_granular_effort = false;
        c
    };

    // Grok 3 / Grok 2：Chat Completions 基线（降级路径覆盖）。
    let grok3 = caps(true, true, true, true, false, true, true);
    let grok2 = caps(true, false, true, true, false, false, false);

    vec![
        def("grok-4", "Grok 4", 256_000, 32_768, grok4),
        def("grok-4-fast", "Grok 4 Fast", 128_000, 32_768, grok4_fast),
        def("grok-3", "Grok 3", 131_072, 16_384, grok3),
        def("grok-2", "Grok 2", 131_072, 16_384, grok2),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_xai_specific() {
        let config = XaiConfig::default();
        assert_eq!(config.base_url, DEFAULT_BASE_URL);
    }

    #[test]
    fn rejects_session_credentials() {
        let credential = ResolvedCredential::new(CredentialKind::SessionToken, "session");
        let error = XaiProvider::new(XaiConfig::default(), Some(credential))
            .err()
            .expect("session credential must be rejected");
        assert_eq!(error.kind, ProviderErrorKind::Authentication);
    }

    #[test]
    fn builtin_catalog_has_responses_and_baseline_models() {
        let models = builtin_models();
        let grok4 = models
            .iter()
            .find(|m| m.id == ModelId::new("grok-4"))
            .expect("grok-4 present");
        assert_eq!(
            grok4.capabilities.transport,
            provider_api::ModelTransport::Responses
        );
        assert!(grok4.capabilities.citations);
        assert!(grok4.capabilities.reasoning.state.requires_encrypted);
        assert!(grok4
            .capabilities
            .hosted_tool_tags
            .contains(&agent_domain::ToolCapabilityTag::WebSearch));

        let grok2 = models
            .iter()
            .find(|m| m.id == ModelId::new("grok-2"))
            .expect("grok-2 present");
        assert_eq!(
            grok2.capabilities.transport,
            provider_api::ModelTransport::ChatCompletions
        );
    }

    #[test]
    fn resolve_capabilities_chooses_responses_for_grok4() {
        use agent_domain::ToolCapabilityTag as T;
        let provider = XaiProvider::new(XaiConfig::default(), None).expect("construct");
        let mut request = base_request(ModelId::new("grok-4"));
        let resolved = provider.resolve_capabilities(&request);
        assert_eq!(
            resolved.chosen_transport,
            provider_api::ModelTransport::Responses
        );

        // 基线模型 → Chat Completions + LegacyTransport fallback。
        request.model = ModelId::new("grok-2");
        request.hosted_tools.push(provider_api::HostedToolRequest {
            name: "web_search".into(),
            kind: T::WebSearch,
            description: String::new(),
            capabilities: vec![T::WebSearch],
            config: None,
        });
        let resolved = provider.resolve_capabilities(&request);
        assert_eq!(
            resolved.chosen_transport,
            provider_api::ModelTransport::ChatCompletions
        );
        assert!(matches!(
            resolved.fallback.get("transport"),
            Some(provider_api::CapabilityFallback::LegacyTransport)
        ));
        // web_search 未声明（基线模型）→ Reject（fail-closed，不伪装支持）。
        assert!(matches!(
            resolved.fallback.get("tool:WebSearch"),
            Some(provider_api::CapabilityFallback::Reject(_))
        ));
    }

    #[test]
    fn resolve_capabilities_keeps_responses_for_grok4_hosted_tools() {
        use agent_domain::ToolCapabilityTag as T;
        let provider = XaiProvider::new(XaiConfig::default(), None).expect("construct");
        let mut request = base_request(ModelId::new("grok-4"));
        request.hosted_tools.push(provider_api::HostedToolRequest {
            name: "web_search".into(),
            kind: T::WebSearch,
            description: String::new(),
            capabilities: vec![T::WebSearch],
            config: None,
        });
        request.reasoning = Some(provider_api::ReasoningConfig::new(
            provider_api::ReasoningEffort::High,
        ));
        let resolved = provider.resolve_capabilities(&request);
        assert_eq!(
            resolved.chosen_transport,
            provider_api::ModelTransport::Responses
        );
        assert!(resolved.supported.contains("tool:WebSearch"));
        assert!(resolved.supported.contains("reasoning"));
        assert!(resolved.unsupported.is_empty());
    }

    fn base_request(model: ModelId) -> CanonicalModelRequest {
        CanonicalModelRequest {
            request_id: agent_domain::RequestId::from("r1"),
            model,
            messages: Vec::new(),
            tools: Vec::new(),
            hosted_tools: Vec::new(),
            extensions: Vec::new(),
            tool_choice: provider_api::ToolChoice::Auto,
            thinking: None,
            reasoning: None,
            temperature: None,
            max_output_tokens: None,
            stop_sequences: Vec::new(),
            response_format: provider_api::ResponseFormat::Text,
            prompt_cache: provider_api::PromptCachePreference::Automatic,
            budget: provider_api::RequestBudget::default(),
            provider_options: std::collections::BTreeMap::new(),
            trace_id: None,
        }
    }
}
