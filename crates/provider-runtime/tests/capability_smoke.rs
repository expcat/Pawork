//! P15-8 Mock A/B/C 协商 smoke：模型 A 全能力、B 部分、C 仅基线，验证
//! 协商取交集、transport 选择、降级可观察，且协商器不读 Provider 名
//! （no_provider_branch）。

use std::collections::BTreeSet;

use agent_domain::{ModelId, ToolCapabilityTag};
use model_registry::{CapabilityEvidence, ModelRegistry};
use provider_api::{
    CapabilityFallback, CapabilityRequirements, ModelCapabilities,
    ModelTransport, ReasoningConfig, ReasoningEffort, ReasoningStateCapability,
    ReasoningStateDescriptor,
};
use provider_runtime::negotiate::{clamp_reasoning_to_thinking, CapabilityNegotiator};

/// 模型 A：全能力（Responses transport、WebSearch + CodeExecution、citations、
/// reasoning 全维度 + granular effort）。
fn model_a_caps() -> ModelCapabilities {
    ModelCapabilities {
        text: true,
        image_input: true,
        tool_calls: true,
        parallel_tool_calls: true,
        thinking: true,
        structured_output: true,
        prompt_cache: true,
        transport: ModelTransport::Responses,
        hosted_tool_tags: [ToolCapabilityTag::WebSearch, ToolCapabilityTag::CodeExecution]
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
    }
}

/// 模型 B：部分能力（Messages transport、仅 WebSearch、citations=false、
/// reasoning 仅 thinking=true，无 granular effort）。
fn model_b_caps() -> ModelCapabilities {
    ModelCapabilities {
        text: true,
        image_input: false,
        tool_calls: true,
        parallel_tool_calls: true,
        thinking: true,
        structured_output: true,
        prompt_cache: true,
        transport: ModelTransport::Messages,
        hosted_tool_tags: [ToolCapabilityTag::WebSearch].into_iter().collect(),
        citations: false,
        reasoning: ReasoningStateCapability {
            state: ReasoningStateDescriptor::default(),
            supports_granular_effort: false,
        },
    }
}

/// 模型 C：仅基线（ChatCompletions、无 hosted tools、无 citations、无 reasoning）。
fn model_c_caps() -> ModelCapabilities {
    ModelCapabilities {
        text: true,
        thinking: false,
        reasoning: ReasoningStateCapability::default(),
        transport: ModelTransport::ChatCompletions,
        ..ModelCapabilities::default()
    }
}

fn evidence(model: &str, caps: ModelCapabilities) -> CapabilityEvidence {
    CapabilityEvidence {
        model: ModelId::new(model),
        provider: None,
        static_declared: Some(caps),
        probe_declared: None,
        override_declared: None,
    }
}

fn full_requirements() -> CapabilityRequirements {
    CapabilityRequirements {
        transport_pref: vec![ModelTransport::Responses, ModelTransport::Messages],
        required_tools: [ToolCapabilityTag::WebSearch, ToolCapabilityTag::CodeExecution]
            .into_iter()
            .collect(),
        reasoning: Some(ReasoningConfig::new(ReasoningEffort::High)),
        citations: true,
    }
}

#[test]
fn model_a_full_capabilities_all_supported() {
    let resolved = CapabilityNegotiator::negotiate(&evidence("a", model_a_caps()), &full_requirements());
    assert_eq!(resolved.chosen_transport, ModelTransport::Responses);
    assert!(resolved.supported.contains("tool:WebSearch"));
    assert!(resolved.supported.contains("tool:CodeExecution"));
    assert!(resolved.supported.contains("citations"));
    assert!(resolved.supported.contains("reasoning"));
    assert!(resolved.unsupported.is_empty(), "A 全能力：无 unsupported，got {:?}", resolved.unsupported);
    // requested == supported ∪ unsupported（A 全 supported）。
    assert_eq!(resolved.requested, resolved.supported);
}

#[test]
fn model_b_partial_intersection_and_degradation_observable() {
    let resolved = CapabilityNegotiator::negotiate(&evidence("b", model_b_caps()), &full_requirements());
    // B 声明 Messages：偏好 Responses 不命中，退回模型声明的 Messages。
    assert_eq!(resolved.chosen_transport, ModelTransport::Messages);
    // WebSearch 支持，CodeExecution 未声明 → unsupported + Reject。
    assert!(resolved.supported.contains("tool:WebSearch"));
    assert!(resolved.unsupported.contains("tool:CodeExecution"));
    assert!(matches!(
        resolved.fallback.get("tool:CodeExecution"),
        Some(CapabilityFallback::Reject(_))
    ));
    // citations 未声明 → unsupported。
    assert!(resolved.unsupported.contains("citations"));
    // reasoning：B 声明 thinking=true，High 支持。
    assert!(resolved.supported.contains("reasoning"));
    // requested == supported ∪ unsupported。
    let mut union: BTreeSet<String> = resolved.supported.clone();
    union.extend(resolved.unsupported.iter().cloned());
    assert_eq!(resolved.requested, union, "requested 必须等于 supported ∪ unsupported");
    // 降级路径明确可观察（fallback 非空）。
    assert!(!resolved.fallback.is_empty());
}

#[test]
fn model_c_baseline_degrades_transport_and_rejects_reasoning() {
    let resolved = CapabilityNegotiator::negotiate(&evidence("c", model_c_caps()), &full_requirements());
    // C 仅基线 ChatCompletions；请求偏好现代 transport → 降级可观察。
    assert_eq!(resolved.chosen_transport, ModelTransport::ChatCompletions);
    assert!(matches!(
        resolved.fallback.get("transport"),
        Some(CapabilityFallback::LegacyTransport)
    ));
    // hosted tools 全 unsupported。
    assert!(resolved.unsupported.contains("tool:WebSearch"));
    assert!(resolved.unsupported.contains("tool:CodeExecution"));
    // reasoning：C 未声明 thinking → unsupported + Reject（fail-closed）。
    assert!(resolved.unsupported.contains("reasoning"));
    assert!(matches!(
        resolved.fallback.get("reasoning"),
        Some(CapabilityFallback::Reject(_))
    ));
    // requested == supported ∪ unsupported。
    let mut union: BTreeSet<String> = resolved.supported.clone();
    union.extend(resolved.unsupported.iter().cloned());
    assert_eq!(resolved.requested, union);
}

#[test]
fn override_only_narrows_never_amplifies() {
    // registry：静态 A 全能力，override 砍掉 CodeExecution + citations + Responses。
    let mut registry = ModelRegistry::empty();
    registry
        .register(model_registry::CatalogEntry {
            id: ModelId::new("a"),
            provider: agent_domain::ProviderId::new("any"),
            display_name: "A".into(),
            context_window_tokens: 8192,
            max_output_tokens: 1024,
            capabilities: model_a_caps(),
            pricing: None,
            aliases: Vec::new(),
        })
        .expect("register");
    let narrowed = ModelCapabilities {
        hosted_tool_tags: [ToolCapabilityTag::CodeExecution].into_iter().collect(),
        citations: true,
        transport: ModelTransport::Responses,
        ..ModelCapabilities::default()
    };
    registry.set_override("a", narrowed);
    let evidence = registry.capability_evidence("a").expect("evidence");
    let merged = evidence.merged();
    // override 收窄：CodeExecution 被静态支持也被 override 声明 → 保留；
    // WebSearch 静态支持但 override 未声明 → override 只能收窄，合并应不含 WebSearch。
    assert!(!merged.hosted_tool_tags.contains(&ToolCapabilityTag::WebSearch));
    assert!(merged.hosted_tool_tags.contains(&ToolCapabilityTag::CodeExecution));

    let resolved = CapabilityNegotiator::negotiate(&evidence, &full_requirements());
    // override 不能放大：transport 仍由静态 Responses 决定（交集后保留）。
    assert_eq!(resolved.chosen_transport, ModelTransport::Responses);
}

#[test]
fn no_provider_branch_negotiator_ignores_provider_id() {
    // 同一 caps、provider 为 None 与 Some("openai") 协商结果完全一致，
    // 证明 transport 选择只读声明、不读 Provider 名。
    let req = CapabilityRequirements {
        reasoning: Some(ReasoningConfig::new(ReasoningEffort::High)),
        ..Default::default()
    };
    let mut ev_no_provider = evidence("m", model_a_caps());
    ev_no_provider.provider = None;
    let mut ev_with_provider = evidence("m", model_a_caps());
    ev_with_provider.provider = Some(agent_domain::ProviderId::new("openai"));
    let r1 = CapabilityNegotiator::negotiate(&ev_no_provider, &req);
    let r2 = CapabilityNegotiator::negotiate(&ev_with_provider, &req);
    assert_eq!(r1, r2, "协商结果不得随 provider id 变化");
}

#[test]
fn clamp_helper_xhigh_and_max_to_high_for_legacy_adapter() {
    // XHigh / Max 在旧 P6 adapter 路径 clamp 为 High，不形成双轨。
    assert_eq!(
        clamp_reasoning_to_thinking(Some(&ReasoningConfig::new(ReasoningEffort::XHigh)), None).level,
        provider_api::ThinkingLevel::High
    );
    assert_eq!(
        clamp_reasoning_to_thinking(Some(&ReasoningConfig::new(ReasoningEffort::Max)), None).level,
        provider_api::ThinkingLevel::High
    );
    assert_eq!(
        clamp_reasoning_to_thinking(Some(&ReasoningConfig::new(ReasoningEffort::None)), None).level,
        provider_api::ThinkingLevel::Off
    );
}
