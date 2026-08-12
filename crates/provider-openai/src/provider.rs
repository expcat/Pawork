//! OpenAI 原生 [`ModelProvider`](provider_api::ModelProvider) 实现。
//!
//! 双传输并存（P15-2）：经 P15-8 能力协商选择，模型声明 `transport = Responses`
//! 时走 OpenAI Responses（`/v1/responses`）现代路径，否则降级到 P6-1 Chat
//! Completions（复用 [`OpenAiCompatibleProvider`](provider_openai_compatible::OpenAiCompatibleProvider)
//! 作为流式引擎）。两条路径都固定 OpenAI 官方端点与 `openai` provider 标识。
//!
//! Responses 路径不在 Core 走任何 OpenAI 名称分支：transport 选择只由
//! [`provider_runtime::negotiate::CapabilityNegotiator`]（纯函数，读模型能力声明）
//! 驱动；reasoning `encrypted_content` 只经 [`crate::responses::ReasoningProtector`]
//! 边界往返（ADR-032）。

use std::sync::Arc;
use std::time::Duration;

use agent_domain::{CancellationToken, ModelId, ProviderId, StopReason, TokenUsage};
use async_trait::async_trait;
use model_registry::CapabilityEvidence;
use provider_api::{
    CanonicalModelRequest, ModelCapabilities, ModelDefinition, ModelProvider, ModelResponseSummary,
    ProviderError, ProviderErrorKind, ProviderEventSink, ProviderStreamEvent, ResolvedCapabilities,
    ResolvedCredential,
};
use provider_openai_compatible::{OpenAiCompatibleConfig, OpenAiCompatibleProvider};
use provider_runtime::http::{HttpClient, HttpClientConfig};
use provider_runtime::negotiate::CapabilityNegotiator;
use provider_runtime::sse::SseParser;

use crate::reasoning::{extract_encrypted_content, responses_reasoning_to_canonical};
use crate::responses::{
    normalize_responses_error, requirements_from_request, resolve_reasoning_inputs,
    to_responses_body, AcceptedResponsesTools, InMemoryReasoningProtector, ReasoningProtector,
    ResponsesAssemblyEvent, ResponsesStreamAssembler,
};
use crate::DEFAULT_BASE_URL;

/// OpenAI 适配器配置。
#[derive(Clone, Debug)]
pub struct OpenAiConfig {
    /// 基础 URL（默认 `https://api.openai.com/v1`）。
    pub base_url: String,
    /// Provider 标识（默认 `openai`）。
    pub provider_id: ProviderId,
    /// HTTP 客户端配置。
    pub http: HttpClientConfig,
    /// 建连及流式读取无数据超时（覆盖 `http.timeout`）。
    pub request_timeout: Option<Duration>,
}

impl Default for OpenAiConfig {
    fn default() -> Self {
        Self {
            base_url: DEFAULT_BASE_URL.into(),
            provider_id: ProviderId::new("openai"),
            http: HttpClientConfig::default(),
            request_timeout: None,
        }
    }
}

impl OpenAiConfig {
    /// 以自定义基础 URL 构造（如 Azure OpenAI 风格端点），provider_id 仍为 `openai`。
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

/// OpenAI 原生 Provider。
pub struct OpenAiProvider {
    inner: OpenAiCompatibleProvider,
    responses_client: HttpClient,
    base_url: String,
    provider_id: ProviderId,
    credential: Option<ResolvedCredential>,
    reasoning_protector: Arc<dyn ReasoningProtector>,
}

impl OpenAiProvider {
    /// 构造适配器。`credential` 为 None 时不带认证头。
    pub fn new(
        config: OpenAiConfig,
        credential: Option<ResolvedCredential>,
    ) -> Result<Self, ProviderError> {
        let http_config = Self::http_config(&config);
        let compat = OpenAiCompatibleConfig {
            base_url: config.base_url.clone(),
            provider_id: config.provider_id.clone(),
            http: http_config.clone(),
            request_timeout: None,
        };
        let inner = OpenAiCompatibleProvider::new(compat, credential.clone())?;
        let responses_client = HttpClient::new(http_config)?;
        Ok(Self {
            inner,
            responses_client,
            base_url: config.base_url,
            provider_id: config.provider_id,
            credential,
            reasoning_protector: Arc::new(InMemoryReasoningProtector::default()),
        })
    }

    /// 注入 reasoning continuation 的 Protected Blob Store 边界实现（P15-7 host
    /// 接入点）。未注入时共享实例级 in-memory 默认 protector（仅保证进程内回放）。
    pub fn with_reasoning_protector(mut self, protector: Arc<dyn ReasoningProtector>) -> Self {
        self.reasoning_protector = protector;
        self
    }

    fn http_config(config: &OpenAiConfig) -> HttpClientConfig {
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

        // reasoning protector 与 Provider 实例共享，保证同进程跨轮可解析引用。
        let protector = self.reasoning_protector.as_ref();

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
                "responses stream ended without completion event",
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
                match extract_encrypted_content(&wire) {
                    Ok(Some(secret)) => {
                        let payload = secret.into_inner().into_bytes();
                        let blob_ref = protector.protect(&payload).await.map_err(|error| {
                            ProviderError::new(
                                ProviderErrorKind::Unknown,
                                format!("reasoning protect failed: {error}"),
                            )
                        })?;
                        match responses_reasoning_to_canonical(&wire, blob_ref) {
                            Ok(item) => {
                                sink.emit(ProviderStreamEvent::ReasoningItem(item)).await?;
                            }
                            Err(error) => {
                                tracing::debug!(
                                    error = %error,
                                    "unmapped Responses reasoning item, skipped"
                                );
                            }
                        }
                    }
                    Ok(None) => {
                        tracing::debug!("reasoning item without encrypted_content, skipped");
                    }
                    Err(error) => {
                        tracing::debug!(error = %error, "reasoning item extraction failed, skipped");
                    }
                }
            }
        }
        Ok(())
    }
}

#[async_trait]
impl ModelProvider for OpenAiProvider {
    fn id(&self) -> ProviderId {
        self.provider_id.clone()
    }

    async fn list_models(
        &self,
        _credential: Option<&ResolvedCredential>,
    ) -> Result<Vec<ModelDefinition>, ProviderError> {
        // OpenAI 远端 /models 不返回能力信息，直接返回能力完整的内置目录。
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
            // 降级到 P6-1 Chat Completions。
            self.inner.stream(request, sink, cancel).await
        }
    }
}

/// OpenAI 内置模型目录（含 reasoning / image / tool 能力）。
/// 数据快照：2026-08-09；目录更新作为显式跟踪项手动执行。
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

    // GPT-4o 系：文本 + 视觉 + 工具，无 reasoning，Chat Completions 基线。
    let vision_tools = caps(true, true, true, true, false, true, true);
    // o 系（Chat Completions 路径）：reasoning 模型，支持 thinking + 视觉 + 工具。
    let reasoning = caps(true, true, true, true, true, true, true);
    // GPT-3.5：无视觉、无 reasoning。
    let legacy = caps(true, false, true, true, false, true, true);

    // o3 / gpt-4.1：声明 Responses 现代传输路径（P15-2）。hosted tool 标签 +
    // citations + reasoning 加密 continuation 由 negotiator 据此选择 Responses。
    let mut o3 = reasoning.clone();
    o3.transport = ModelTransport::Responses;
    o3.citations = true;
    o3.hosted_tool_tags = [T::WebSearch, T::CodeExecution, T::FileOrCollectionSearch]
        .into_iter()
        .collect::<BTreeSet<_>>();
    o3.reasoning = ReasoningStateCapability {
        state: ReasoningStateDescriptor {
            requires_signature: false,
            requires_encrypted: true,
            supports_interleaved: true,
        },
        supports_granular_effort: true,
    };

    let mut gpt41 = vision_tools.clone();
    gpt41.transport = ModelTransport::Responses;
    gpt41.citations = true;
    gpt41.hosted_tool_tags = [
        T::WebSearch,
        T::CodeExecution,
        T::ImageGeneration,
        T::ComputerUse,
    ]
    .into_iter()
    .collect::<BTreeSet<_>>();
    gpt41.reasoning = ReasoningStateCapability {
        state: ReasoningStateDescriptor {
            requires_encrypted: true,
            ..ReasoningStateDescriptor::default()
        },
        supports_granular_effort: false,
    };

    vec![
        def("gpt-4o", "GPT-4o", 128_000, 16_384, vision_tools.clone()),
        def("gpt-4o-mini", "GPT-4o mini", 128_000, 16_384, vision_tools),
        def("o1", "o1", 200_000, 100_000, reasoning.clone()),
        def("o1-mini", "o1-mini", 128_000, 65_536, reasoning),
        def("o3", "o3", 200_000, 100_000, o3),
        def("gpt-4.1", "GPT-4.1", 128_000, 16_384, gpt41),
        def("gpt-3.5-turbo", "GPT-3.5 Turbo", 16_385, 4_096, legacy),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_catalog_has_reasoning_and_vision_models() {
        let models = builtin_models();
        let o1 = models
            .iter()
            .find(|m| m.id == ModelId::new("o1"))
            .expect("o1 present");
        assert!(o1.capabilities.thinking);
        assert!(o1.capabilities.image_input);
        let gpt4o = models
            .iter()
            .find(|m| m.id == ModelId::new("gpt-4o"))
            .expect("gpt-4o present");
        assert!(!gpt4o.capabilities.thinking);
        assert!(gpt4o.capabilities.image_input);
    }

    #[test]
    fn config_defaults_to_openai_endpoint() {
        let config = OpenAiConfig::default();
        assert_eq!(config.base_url, DEFAULT_BASE_URL);
        assert_eq!(config.provider_id, ProviderId::new("openai"));
    }

    #[test]
    fn responses_capable_models_declare_modern_transport() {
        let models = builtin_models();
        let o3 = models
            .iter()
            .find(|m| m.id == ModelId::new("o3"))
            .expect("o3 present");
        assert_eq!(
            o3.capabilities.transport,
            provider_api::ModelTransport::Responses
        );
        assert!(o3.capabilities.citations);
        assert!(o3.capabilities.reasoning.state.requires_encrypted);

        let gpt41 = models
            .iter()
            .find(|m| m.id == ModelId::new("gpt-4.1"))
            .expect("gpt-4.1 present");
        assert_eq!(
            gpt41.capabilities.transport,
            provider_api::ModelTransport::Responses
        );

        // 基线模型仍走 Chat Completions（降级路径覆盖）。
        let gpt4o = models
            .iter()
            .find(|m| m.id == ModelId::new("gpt-4o"))
            .expect("gpt-4o present");
        assert_eq!(
            gpt4o.capabilities.transport,
            provider_api::ModelTransport::ChatCompletions
        );
    }

    #[test]
    fn resolve_capabilities_chooses_responses_for_modern_model() {
        use agent_domain::ToolCapabilityTag as T;
        let provider = OpenAiProvider::new(OpenAiConfig::default(), None).expect("construct");
        let mut request = CanonicalModelRequest {
            request_id: agent_domain::RequestId::from("r1"),
            model: ModelId::new("o3"),
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
        };
        let resolved = provider.resolve_capabilities(&request);
        assert_eq!(
            resolved.chosen_transport,
            provider_api::ModelTransport::Responses
        );

        // 基线模型 → Chat Completions + LegacyTransport fallback。
        request.model = ModelId::new("gpt-4o");
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
        // web_search 未声明 → Reject（fail-closed，不伪装支持）。
        assert!(matches!(
            resolved.fallback.get("tool:WebSearch"),
            Some(provider_api::CapabilityFallback::Reject(_))
        ));
    }
}
