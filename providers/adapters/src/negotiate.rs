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
    CapabilityFallback, CapabilityRequirements, ModelCapabilities, ModelTransport,
    ReasoningConfig, ReasoningEffort, ResolvedCapabilities, ThinkingConfig, ThinkingLevel,
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
        let model_supports_reasoning = caps.thinking
            || caps.reasoning.state.requires_signature
            || caps.reasoning.state.requires_encrypted
            || caps.reasoning.state.supports_interleaved
            || caps.reasoning.supports_granular_effort;
        if reasoning.requires_reasoning_support() && !model_supports_reasoning {
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
            resolved.unsupported.insert(format!("{key}.signature"));
            resolved.fallback.insert(
                format!("{key}.signature"),
                CapabilityFallback::Reject("signature continuation not supported".into()),
            );
        }
        if state.requires_encrypted && !caps.reasoning.state.requires_encrypted {
            resolved.unsupported.insert(format!("{key}.encrypted"));
            resolved.fallback.insert(
                format!("{key}.encrypted"),
                CapabilityFallback::Reject("encrypted continuation not supported".into()),
            );
        }
        if state.supports_interleaved && !caps.reasoning.state.supports_interleaved {
            resolved.unsupported.insert(format!("{key}.interleaved"));
            resolved.fallback.insert(
                format!("{key}.interleaved"),
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

    #[test]
    fn supported_intersection_matches_evidence() {
        let evidence = evidence_from(full_caps());
        let requirements = CapabilityRequirements {
            required_tools: [ToolCapabilityTag::WebSearch].into_iter().collect(),
            citations: true,
            reasoning: Some(ReasoningConfig::new(ReasoningEffort::High)),
            transport_pref: vec![ModelTransport::Responses],
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
