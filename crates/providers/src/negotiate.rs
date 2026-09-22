//! 能力协商（迁自 V1 `provider-runtime::negotiate`）。
//!
//! [`CapabilityNegotiator::negotiate`] 是纯函数：输入「证据快照 × 请求要求」，
//! 输出 [`ResolvedCapabilities`]。不触网、不读 Provider 名、不读 wall-clock。
//!
//! 两层交集语义：
//! - 证据层（[`crate::registry::merge_capabilities`]）：已出现来源逐字段取交集，
//!   来源整体缺失不约束；override 只能收窄。
//! - 请求层（本模块）：未声明支持的能力进入 `unsupported`，禁止静默丢弃/伪造。
//!   `requested == supported ∪ unsupported`。
//!
//! transport 选择由 `ModelCapabilities::transport`（证据层合并后）驱动；请求
//! `transport_pref` 偏好优先，但仅在模型已声明该 transport 时才采用，否则
//! 退回模型自身 transport；模型未声明现代 transport 时退回 ChatCompletions
//! 基线并记录 `LegacyTransport` fallback。
//!
//! reasoning：显式 `ReasoningConfig` 优先于旧 `ThinkingConfig.level`；
//! 请求 reasoning 但模型 `thinking == false` 时整项 reasoning 进 `unsupported`
//! —— 进 `Reject`；`XHigh / Max` 但模型不支持细粒度 effort 时 clamp 为 `High`
//! 并记录 `ClampedEffort`。clamp helper 供 adapter 复用（不形成双轨）。

use pawork_domain::{
    CapabilityFallback, CapabilityRequirements, ModelCapabilities, ModelTransport, ReasoningConfig,
    ReasoningEffort, ResolvedCapabilities, ThinkingConfig, ThinkingLevel,
};

use crate::registry::CapabilityEvidence;

/// 能力协商器（无状态，纯函数入口）。
#[derive(Debug, Default, Clone, Copy)]
pub struct CapabilityNegotiator;

impl CapabilityNegotiator {
    /// 以「证据快照 × 请求要求」协商出 [`ResolvedCapabilities`]。
    ///
    /// 证据层先取交集得到 `supported_caps`；请求层逐项判定 supported /
    /// unsupported，所有请求项都进入 `requested`，满足
    /// `requested == supported ∪ unsupported`。
    pub fn negotiate(
        evidence: &CapabilityEvidence,
        requirements: &CapabilityRequirements,
    ) -> ResolvedCapabilities {
        // 证据层合并：已出现来源逐字段取交集（fail-closed）。
        let supported_caps = evidence.merged();
        let mut resolved = ResolvedCapabilities::default();
        resolved.chosen_transport =
            Self::choose_transport(&supported_caps, &requirements.transport_pref, &mut resolved);

        // hosted tools：逐项判定。
        for tag in &requirements.required_tools {
            let key = String::from(tag.capability_key());
            resolved.requested.insert(key.clone());
            if supported_caps.hosted_tool_tags.contains(tag) {
                resolved.supported.insert(key);
            } else {
                resolved.unsupported.insert(key.clone());
                resolved.fallback.insert(
                    key,
                    CapabilityFallback::Reject(format!(
                        "server tool `{tag:?}` not declared by model"
                    )),
                );
            }
        }

        // citations。
        if requirements.citations {
            resolved.requested.insert("citations".into());
            if supported_caps.citations {
                resolved.supported.insert("citations".into());
            } else {
                resolved.unsupported.insert("citations".into());
                resolved.fallback.insert(
                    "citations".into(),
                    CapabilityFallback::Reject("citations not supported by model".into()),
                );
            }
        }

        // image_input（VISION-1）：请求含图片内容时置位。
        if requirements.image_input {
            resolved.requested.insert("image_input".into());
            if supported_caps.image_input {
                resolved.supported.insert("image_input".into());
            } else {
                resolved.unsupported.insert("image_input".into());
                resolved.fallback.insert(
                    "image_input".into(),
                    CapabilityFallback::Reject(
                        "model does not declare image input capability".into(),
                    ),
                );
            }
        }

        // reasoning：显式 ReasoningConfig 优先。
        if let Some(reasoning) = &requirements.reasoning {
            Self::negotiate_reasoning(reasoning, &supported_caps, &mut resolved);
        }

        resolved
    }

    /// 选择 transport：偏好优先但仅采用模型已声明的现代 transport；否则退回
    /// 模型自身 transport；模型未声明现代 transport 时退回 ChatCompletions。
    fn choose_transport(
        caps: &ModelCapabilities,
        pref: &[ModelTransport],
        resolved: &mut ResolvedCapabilities,
    ) -> ModelTransport {
        // 请求偏好且模型已声明该 transport → 采用。
        for wanted in pref {
            if caps.transport == *wanted {
                return *wanted;
            }
        }
        // 模型声明了任何 transport（非默认 ChatCompletions）→ 用模型声明。
        if caps.transport != ModelTransport::ChatCompletions {
            return caps.transport;
        }
        // 模型只声明了基线 ChatCompletions，但请求偏好现代 transport → 降级。
        if pref.iter().any(|transport| transport.is_modern()) {
            resolved
                .fallback
                .insert("transport".into(), CapabilityFallback::LegacyTransport);
        }
        ModelTransport::ChatCompletions
    }

    /// 协商 reasoning 一项：effort 维度 + state 维度。
    fn negotiate_reasoning(
        reasoning: &ReasoningConfig,
        caps: &ModelCapabilities,
        resolved: &mut ResolvedCapabilities,
    ) {
        let key = "reasoning".to_string();
        resolved.requested.insert(key.clone());

        // 模型未声明任何 reasoning 能力（v1 thinking=false 且 v2 reasoning 空）。
        // effort 词汇不含 none：ReasoningConfig 存在即要求模型声明 reasoning 能力。
        let model_supports_reasoning = caps.thinking
            || caps.reasoning.state.requires_signature
            || caps.reasoning.state.requires_encrypted
            || caps.reasoning.state.supports_interleaved
            || caps.reasoning.supports_granular_effort;
        if !model_supports_reasoning {
            resolved.unsupported.insert(key.clone());
            resolved.fallback.insert(
                key,
                CapabilityFallback::Reject(format!(
                    "reasoning effort {:?} not supported by model",
                    reasoning.effort
                )),
            );
            return;
        }

        // XHigh / Max 但模型不支持细粒度 effort → clamp 为 High。
        let needs_clamp = matches!(
            reasoning.effort,
            ReasoningEffort::XHigh | ReasoningEffort::Max
        ) && !caps.reasoning.supports_granular_effort;
        if needs_clamp {
            resolved
                .fallback
                .insert(format!("{key}.effort"), CapabilityFallback::ClampedEffort);
        }

        // state 维度：签名 / 加密 continuation 由模型声明的 reasoning 维度
        // 决定（声明即支持）；模型未声明该维度则进 unsupported。
        let state = reasoning.state;
        if state.requires_signature && !caps.reasoning.state.requires_signature {
            let state_key = format!("{key}.signature");
            resolved.requested.insert(state_key.clone());
            resolved.unsupported.insert(state_key.clone());
            resolved.fallback.insert(
                state_key,
                CapabilityFallback::Reject("signature continuation not supported".into()),
            );
        }
        if state.requires_encrypted && !caps.reasoning.state.requires_encrypted {
            let state_key = format!("{key}.encrypted");
            resolved.requested.insert(state_key.clone());
            resolved.unsupported.insert(state_key.clone());
            resolved.fallback.insert(
                state_key,
                CapabilityFallback::Reject("encrypted continuation not supported".into()),
            );
        }
        if state.supports_interleaved && !caps.reasoning.state.supports_interleaved {
            let state_key = format!("{key}.interleaved");
            resolved.requested.insert(state_key.clone());
            resolved.unsupported.insert(state_key.clone());
            resolved.fallback.insert(
                state_key,
                CapabilityFallback::Reject("interleaved thinking not supported".into()),
            );
        }
        resolved.supported.insert(key);
    }
}

/// 把请求的 canonical `ReasoningConfig` clamp 为旧 `ThinkingConfig`。
///
/// adapter 复用入口：`XHigh / Max` 在旧 adapter 路径上显式 clamp 为 `High`
/// （并记录），不形成双轨。`ReasoningConfig::effort` 优先；为 `None` 时回退
/// 到旧 `thinking.level`（若也无则 `Off`）。
pub fn clamp_reasoning_to_thinking(
    reasoning: Option<&ReasoningConfig>,
    thinking: Option<&ThinkingConfig>,
) -> ThinkingConfig {
    if let Some(reasoning) = reasoning {
        let level = pawork_domain::clamp_effort_to_thinking_level(reasoning.effort);
        return ThinkingConfig {
            level,
            budget_tokens: thinking.and_then(|config| config.budget_tokens),
        };
    }
    thinking.cloned().unwrap_or(ThinkingConfig {
        level: ThinkingLevel::Off,
        budget_tokens: None,
    })
}

/// VISION-1 / SEARCH-1 前置闸门：发 HTTP 前按证据校验请求的图片与
/// hosted/extension 工具要求；任一 Reject 即返回 `InvalidRequest`，不触网。
///
/// Anthropic adapter 经 `prepare_request` 走完整 negotiate 收口；其余通道由
/// 装配 / Run 服务在派发前统一调用本函数（按证据判定，不按厂商分支）。
pub fn capability_gate(
    evidence: &CapabilityEvidence,
    request: &pawork_domain::CanonicalModelRequest,
) -> Result<(), pawork_domain::ProviderError> {
    use pawork_domain::{ProviderError, ProviderErrorKind, ToolCapabilityTag};

    let mut required_tools = std::collections::BTreeSet::new();
    for hosted in &request.hosted_tools {
        required_tools.insert(hosted.kind);
        required_tools.extend(hosted.capabilities.iter().copied());
    }
    for extension in &request.extensions {
        required_tools.extend(extension.capabilities.iter().copied());
        if extension.capabilities.is_empty() {
            required_tools.insert(ToolCapabilityTag::ServerSideMcp);
        }
    }
    let image_input = request
        .messages
        .iter()
        .flat_map(|message| message.content.iter())
        .any(content_part_has_image);
    let requirements = CapabilityRequirements {
        required_tools,
        image_input,
        ..CapabilityRequirements::default()
    };
    let resolved = CapabilityNegotiator::negotiate(evidence, &requirements);
    resolved
        .fallback
        .values()
        .find_map(|item| match item {
            CapabilityFallback::Reject(reason) => Some(reason.clone()),
            _ => None,
        })
        .map_or(Ok(()), |reason| {
            Err(ProviderError::new(
                ProviderErrorKind::InvalidRequest,
                reason,
            ))
        })
}

fn content_part_has_image(part: &pawork_domain::ContentPart) -> bool {
    match part {
        pawork_domain::ContentPart::Image(_) => true,
        pawork_domain::ContentPart::ToolResult(result) => {
            result.content.iter().any(content_part_has_image)
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    use pawork_domain::ReasoningStateCapability;
    use pawork_domain::{ModelId, ToolCapabilityTag};

    use crate::registry::merge_capabilities;

    fn evidence_from(caps: ModelCapabilities) -> CapabilityEvidence {
        CapabilityEvidence {
            model: ModelId::new("test-model"),
            provider: None,
            static_declared: Some(caps),
            probe_declared: None,
            override_declared: None,
        }
    }

    fn full_caps() -> ModelCapabilities {
        ModelCapabilities {
            text: true,
            image_input: true,
            tool_calls: true,
            parallel_tool_calls: true,
            thinking: true,
            structured_output: true,
            prompt_cache: true,
            transport: ModelTransport::Responses,
            supported_efforts: None,
            hosted_tool_tags: [
                ToolCapabilityTag::WebSearch,
                ToolCapabilityTag::CodeExecution,
            ]
            .into_iter()
            .collect(),
            citations: true,
            reasoning: ReasoningStateCapability {
                state: pawork_domain::ReasoningStateDescriptor {
                    requires_signature: true,
                    requires_encrypted: true,
                    supports_interleaved: true,
                },
                supports_granular_effort: true,
            },
        }
    }

    fn gate_request(
        with_image: bool,
        with_web_search: bool,
    ) -> pawork_domain::CanonicalModelRequest {
        use pawork_domain::{
            CanonicalModelRequest, ContentPart, HostedToolRequest, ImageContent, ImageSource,
            Message, MessageId, MessageRole, PromptCachePreference, RequestBudget, RequestId,
            ResponseFormat, TextContent, ToolChoice,
        };
        let content = if with_image {
            vec![ContentPart::Image(ImageContent {
                source: ImageSource::Url("https://example.test/a.png".into()),
                media_type: "image/png".into(),
                alt_text: None,
            })]
        } else {
            vec![ContentPart::Text(TextContent { text: "hi".into() })]
        };
        let mut request = CanonicalModelRequest {
            request_id: RequestId::from("r1"),
            session_id: None,
            model: ModelId::from("test-model"),
            messages: vec![Message {
                id: MessageId::from("m1"),
                role: MessageRole::User,
                content,
                metadata: Default::default(),
            }],
            tools: Vec::new(),
            hosted_tools: Vec::new(),
            extensions: Vec::new(),
            tool_choice: ToolChoice::Auto,
            thinking: None,
            reasoning: None,
            temperature: None,
            max_output_tokens: None,
            stop_sequences: Vec::new(),
            response_format: ResponseFormat::Text,
            prompt_cache: PromptCachePreference::Automatic,
            budget: RequestBudget::default(),
            provider_options: Default::default(),
            trace_id: None,
        };
        if with_web_search {
            request.hosted_tools.push(HostedToolRequest {
                name: "web_search".into(),
                kind: ToolCapabilityTag::WebSearch,
                description: String::new(),
                capabilities: Vec::new(),
                config: None,
            });
        }
        request
    }

    #[test]
    fn capability_gate_fail_closed_on_undeclared_image_and_web_search() {
        // VISION-1 / SEARCH-1：发 HTTP 前闸门。声明则放行，未声明则
        // InvalidRequest；空证据（未知模型）只放行纯文本。
        let mut caps = full_caps();
        caps.image_input = false;
        caps.hosted_tool_tags.clear();
        let no_decl = evidence_from(caps);

        let error = capability_gate(&no_decl, &gate_request(true, false))
            .err()
            .expect("image without declaration must reject");
        assert_eq!(error.kind, pawork_domain::ProviderErrorKind::InvalidRequest);
        let error = capability_gate(&no_decl, &gate_request(false, true))
            .err()
            .expect("web search without declaration must reject");
        assert_eq!(error.kind, pawork_domain::ProviderErrorKind::InvalidRequest);
        assert!(capability_gate(&no_decl, &gate_request(false, false)).is_ok());

        let declared = evidence_from(full_caps());
        assert!(capability_gate(&declared, &gate_request(true, true)).is_ok());

        let unknown = CapabilityEvidence {
            model: ModelId::new("unknown-model"),
            provider: None,
            static_declared: None,
            probe_declared: None,
            override_declared: None,
        };
        assert!(capability_gate(&unknown, &gate_request(false, false)).is_ok());
        assert!(capability_gate(&unknown, &gate_request(true, false)).is_err());
        assert!(capability_gate(&unknown, &gate_request(false, true)).is_err());
    }

    #[test]
    fn capability_gate_rejects_nested_tool_result_image_without_declaration() {
        use pawork_domain::{
            CanonicalModelRequest, ContentPart, ImageContent, ImageSource, Message, MessageId,
            MessageRole, PromptCachePreference, RequestBudget, RequestId, ResponseFormat,
            TextContent, ToolCallId, ToolChoice, ToolResultContent,
        };

        let mut caps = full_caps();
        caps.image_input = false;
        let no_decl = evidence_from(caps);
        let request = CanonicalModelRequest {
            request_id: RequestId::from("r1"),
            session_id: None,
            model: ModelId::from("test-model"),
            messages: vec![Message {
                id: MessageId::from("t1"),
                role: MessageRole::Tool,
                content: vec![ContentPart::ToolResult(ToolResultContent {
                    tool_call_id: ToolCallId::from("call-1"),
                    tool_name: Some("computer".into()),
                    content: vec![
                        ContentPart::Text(TextContent {
                            text: "shot".into(),
                        }),
                        ContentPart::Image(ImageContent {
                            source: ImageSource::Base64("aaa".into()),
                            media_type: "image/jpeg".into(),
                            alt_text: None,
                        }),
                    ],
                    is_error: false,
                    metadata: serde_json::json!(null),
                    artifacts: Vec::new(),
                })],
                metadata: Default::default(),
            }],
            tools: Vec::new(),
            hosted_tools: Vec::new(),
            extensions: Vec::new(),
            tool_choice: ToolChoice::Auto,
            thinking: None,
            reasoning: None,
            temperature: None,
            max_output_tokens: None,
            stop_sequences: Vec::new(),
            response_format: ResponseFormat::Text,
            prompt_cache: PromptCachePreference::Automatic,
            budget: RequestBudget::default(),
            provider_options: Default::default(),
            trace_id: None,
        };
        let error = capability_gate(&no_decl, &request)
            .err()
            .expect("nested tool result image without declaration must reject");
        assert_eq!(error.kind, pawork_domain::ProviderErrorKind::InvalidRequest);
        assert!(capability_gate(&evidence_from(full_caps()), &request).is_ok());
    }

    #[test]
    fn supported_intersection_matches_evidence() {
        let evidence = evidence_from(full_caps());
        let requirements = CapabilityRequirements {
            required_tools: [ToolCapabilityTag::WebSearch].into_iter().collect(),
            citations: true,
            reasoning: Some(ReasoningConfig::new(ReasoningEffort::High)),
            transport_pref: vec![ModelTransport::Responses],
            ..Default::default()
        };
        let resolved = CapabilityNegotiator::negotiate(&evidence, &requirements);
        assert_eq!(resolved.chosen_transport, ModelTransport::Responses);
        assert!(resolved.supported.contains("tool:WebSearch"));
        assert!(resolved.supported.contains("citations"));
        assert!(resolved.supported.contains("reasoning"));
        assert!(resolved.unsupported.is_empty(), "全能力模型无 unsupported");
    }

    #[test]
    fn unsupported_capability_is_fail_closed_and_requested_covered() {
        let mut caps = full_caps();
        caps.hosted_tool_tags.clear();
        caps.citations = false;
        caps.thinking = false;
        caps.reasoning = ReasoningStateCapability::default();
        let evidence = evidence_from(caps);
        let requirements = CapabilityRequirements {
            required_tools: [ToolCapabilityTag::WebSearch].into_iter().collect(),
            citations: true,
            reasoning: Some(ReasoningConfig::new(ReasoningEffort::Medium)),
            transport_pref: vec![],
            ..Default::default()
        };
        let resolved = CapabilityNegotiator::negotiate(&evidence, &requirements);
        // requested == supported ∪ unsupported
        let union: BTreeSet<String> = resolved
            .supported
            .iter()
            .chain(resolved.unsupported.iter())
            .cloned()
            .collect();
        assert_eq!(resolved.requested, union);
        assert!(resolved.unsupported.contains("tool:WebSearch"));
        assert!(resolved.unsupported.contains("citations"));
        assert!(resolved.unsupported.contains("reasoning"));
        assert!(matches!(
            resolved.fallback.get("reasoning"),
            Some(CapabilityFallback::Reject(_))
        ));
    }

    #[test]
    fn unsupported_reasoning_state_preserves_requested_partition_invariant() {
        let mut caps = full_caps();
        caps.reasoning.state.requires_signature = false;
        let evidence = evidence_from(caps);
        let requirements = CapabilityRequirements {
            reasoning: Some(ReasoningConfig {
                effort: ReasoningEffort::High,
                state: pawork_domain::ReasoningStateDescriptor {
                    requires_signature: true,
                    requires_encrypted: false,
                    supports_interleaved: false,
                },
            }),
            ..CapabilityRequirements::default()
        };
        let resolved = CapabilityNegotiator::negotiate(&evidence, &requirements);
        let union: BTreeSet<String> = resolved
            .supported
            .iter()
            .chain(resolved.unsupported.iter())
            .cloned()
            .collect();
        assert_eq!(resolved.requested, union);
        assert!(resolved.requested.contains("reasoning.signature"));
        assert!(resolved.unsupported.contains("reasoning.signature"));
        assert!(matches!(
            resolved.fallback.get("reasoning.signature"),
            Some(CapabilityFallback::Reject(_))
        ));
    }

    #[test]
    fn xhigh_clamps_to_high_when_granular_effort_unsupported() {
        let mut caps = full_caps();
        caps.reasoning.supports_granular_effort = false;
        let evidence = evidence_from(caps);
        let requirements = CapabilityRequirements {
            reasoning: Some(ReasoningConfig::new(ReasoningEffort::XHigh)),
            ..Default::default()
        };
        let resolved = CapabilityNegotiator::negotiate(&evidence, &requirements);
        assert!(resolved.supported.contains("reasoning"));
        assert!(matches!(
            resolved.fallback.get("reasoning.effort"),
            Some(CapabilityFallback::ClampedEffort)
        ));
    }

    #[test]
    fn transport_falls_back_to_baseline_when_modern_not_declared() {
        let mut caps = full_caps();
        caps.transport = ModelTransport::ChatCompletions;
        let evidence = evidence_from(caps);
        let requirements = CapabilityRequirements {
            transport_pref: vec![ModelTransport::Responses],
            ..Default::default()
        };
        let resolved = CapabilityNegotiator::negotiate(&evidence, &requirements);
        assert_eq!(resolved.chosen_transport, ModelTransport::ChatCompletions);
        assert!(matches!(
            resolved.fallback.get("transport"),
            Some(CapabilityFallback::LegacyTransport)
        ));
    }

    #[test]
    fn transport_pref_uses_model_declared_transport() {
        let mut caps = full_caps();
        caps.transport = ModelTransport::Messages;
        let evidence = evidence_from(caps);
        let requirements = CapabilityRequirements {
            transport_pref: vec![ModelTransport::Responses, ModelTransport::Messages],
            ..Default::default()
        };
        let resolved = CapabilityNegotiator::negotiate(&evidence, &requirements);
        // 偏好 Responses 优先，但模型只声明 Messages → 选 Messages（模型声明）。
        assert_eq!(resolved.chosen_transport, ModelTransport::Messages);
    }

    #[test]
    fn evidence_layer_intersection_is_respected() {
        // override 收窄：把 reasoning 砍掉，transport 砍成基线。
        let static_caps = full_caps();
        let override_caps = ModelCapabilities {
            thinking: false,
            reasoning: ReasoningStateCapability::default(),
            transport: ModelTransport::ChatCompletions,
            hosted_tool_tags: BTreeSet::new(),
            citations: false,
            ..static_caps.clone()
        };
        let evidence = CapabilityEvidence {
            model: ModelId::new("m"),
            provider: None,
            static_declared: Some(static_caps),
            probe_declared: None,
            override_declared: Some(override_caps),
        };
        let merged = evidence.merged();
        assert!(!merged.thinking, "override 收窄 reasoning");
        assert_eq!(merged.transport, ModelTransport::ChatCompletions);
        assert!(!merge_capabilities(&[]).text, "空证据全不支持");
    }

    #[test]
    fn clamp_reasoning_helper_maps_effort_and_xhigh() {
        let reasoning = ReasoningConfig::new(ReasoningEffort::Max);
        let thinking = clamp_reasoning_to_thinking(Some(&reasoning), None);
        assert_eq!(thinking.level, ThinkingLevel::High, "Max clamp 为 High");
        let reasoning_low = ReasoningConfig::new(ReasoningEffort::Low);
        assert_eq!(
            clamp_reasoning_to_thinking(Some(&reasoning_low), None).level,
            ThinkingLevel::Low
        );
        // 无 reasoning → 回退旧 thinking。
        let legacy = ThinkingConfig {
            level: ThinkingLevel::Medium,
            budget_tokens: Some(64),
        };
        let out = clamp_reasoning_to_thinking(None, Some(&legacy));
        assert_eq!(out.level, ThinkingLevel::Medium);
        assert_eq!(out.budget_tokens, Some(64));
        // 都无 → Off。
        assert_eq!(
            clamp_reasoning_to_thinking(None, None).level,
            ThinkingLevel::Off
        );
    }

    #[test]
    fn no_provider_branch_negotiator_does_not_read_provider_name() {
        // 协商器只消费证据 + 要求，证据里的 provider: None 也能完成协商，
        // 证明不依赖 Provider 名。
        let evidence = CapabilityEvidence {
            model: ModelId::new("m"),
            provider: None,
            static_declared: Some(full_caps()),
            probe_declared: None,
            override_declared: None,
        };
        let requirements = CapabilityRequirements {
            reasoning: Some(ReasoningConfig::new(ReasoningEffort::High)),
            ..Default::default()
        };
        let resolved = CapabilityNegotiator::negotiate(&evidence, &requirements);
        assert!(resolved.supported.contains("reasoning"));
    }

    #[test]
    fn all_tool_tags_negotiate_via_stable_capability_key() {
        // 变体守护：穷举所有 ToolCapabilityTag，协商路径必须使用稳定
        // `tool:PascalCase` key（禁止 Debug 反解），且各 key 唯一。
        const ALL_TAGS: [ToolCapabilityTag; 14] = [
            ToolCapabilityTag::WebSearch,
            ToolCapabilityTag::WebFetch,
            ToolCapabilityTag::FileOrCollectionSearch,
            ToolCapabilityTag::XSearch,
            ToolCapabilityTag::CodeExecution,
            ToolCapabilityTag::HostedShell,
            ToolCapabilityTag::ProviderApplyPatch,
            ToolCapabilityTag::ComputerUse,
            ToolCapabilityTag::ImageGeneration,
            ToolCapabilityTag::ServerSideMcp,
            ToolCapabilityTag::ToolSearch,
            ToolCapabilityTag::Memory,
            ToolCapabilityTag::ProgrammaticToolCalling,
            ToolCapabilityTag::ServerSideMultiAgent,
        ];
        let mut seen = std::collections::BTreeSet::new();
        for tag in ALL_TAGS {
            let key = tag.capability_key();
            assert!(key.starts_with("tool:"), "{key} 必须以 tool: 开头");
            let suffix = key.strip_prefix("tool:").expect("tool: 前缀");
            assert!(
                !suffix.contains('_') && suffix.chars().next().is_some_and(char::is_uppercase),
                "{key} 必须是 PascalCase"
            );
            assert!(seen.insert(key), "{key} 重复");
        }

        // 全能力模型 + 全部 tag 请求 → 每个 tag 都以 capability_key 进入
        // requested/supported，证明协商层不经 Debug 反解构造 key。
        let mut caps = full_caps();
        caps.hosted_tool_tags = ALL_TAGS.into_iter().collect();
        let requirements = CapabilityRequirements {
            required_tools: ALL_TAGS.into_iter().collect(),
            ..Default::default()
        };
        let resolved = CapabilityNegotiator::negotiate(&evidence_from(caps), &requirements);
        assert_eq!(resolved.requested.len(), ALL_TAGS.len());
        assert_eq!(resolved.supported.len(), ALL_TAGS.len());
        for tag in ALL_TAGS {
            let key = String::from(tag.capability_key());
            assert!(resolved.requested.contains(&key), "{key} 未进入 requested");
            assert!(resolved.supported.contains(&key), "{key} 未进入 supported");
        }
    }
}
