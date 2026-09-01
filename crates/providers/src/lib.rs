//! Pawork 首发模型渠道适配器。
//!
//! OpenAI-compatible Chat Completions 始终编译；S2 已有的 `anthropic` 基线继续
//! 默认开启。S6 首发范围只增加 ChatGPT OAuth、xAI Grok OAuth，以及 GLM
//! Coding Plan / OpenCode Go / Qwen Token Plan / DeepSeek 四条 API-key 通道。
//! 其它厂商留到后续需求，不预留伪实现 feature。

pub(crate) fn is_credential_header(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "authorization" | "proxy-authorization" | "api-key" | "x-api-key" | "x-goog-api-key"
    )
}

pub mod channels;
pub mod error;
pub mod error_table;
pub mod memory_protector;
pub mod negotiate;
pub mod net;
pub mod pricing;
pub mod provider;
pub mod reasoning;
pub mod registry;
pub mod request;
pub mod responses;
mod responses_reasoning;
pub mod stream;
pub mod usage;

pub use error::RegistryError;
pub use error_table::{normalize_vendor_error, VendorErrorRule, VENDOR_ERROR_RULES};
pub use memory_protector::InMemoryReasoningProtector;
pub use negotiate::{clamp_reasoning_to_thinking, CapabilityNegotiator};
pub use pricing::{estimate_cost, ModelPricing, BUILTIN_RATE_CARD, BUILTIN_RATE_VERSION};
pub use provider::{OpenAiCompatibleConfig, OpenAiCompatibleProvider};
pub use reasoning::{ReasoningProtectError, ReasoningProtector};
pub use registry::{
    caps, merge_capabilities, CapabilityEvidence, CapabilitySource, CatalogEntry, ModelRegistry,
    ProbeError, ProviderCapabilitySource, ProviderProbe,
};
pub use request::to_chat_completions_body;
pub use stream::{chunk_to_events, is_done, ChunkState};
pub use usage::{map_stop_reason, normalize_usage, UsageAccumulator};

#[cfg(feature = "anthropic")]
pub use channels::{
    builtin_models as anthropic_builtin_models, event_to_events, parse_event, to_messages_body,
    to_messages_body_with_plan, AnthropicConfig, AnthropicProvider, AnthropicStreamState,
    MessagesWirePlan, StreamOutput, ANTHROPIC_VERSION,
};

// S2 装配链仍用 `builtin_models` 指 Anthropic 静态目录；完整化后保持该别名。
#[cfg(feature = "anthropic")]
pub use channels::builtin_models;

#[cfg(feature = "chatgpt-oauth")]
pub use channels::chatgpt::DEFAULT_BASE_URL as CHATGPT_DEFAULT_BASE_URL;
#[cfg(feature = "chatgpt-oauth")]
pub use channels::{ChatGptConfig, ChatGptProvider};

#[cfg(feature = "xai-oauth")]
pub use channels::xai::DEFAULT_BASE_URL as XAI_DEFAULT_BASE_URL;
#[cfg(feature = "xai-oauth")]
pub use channels::{xai_builtin_models, XaiConfig, XaiProvider};

#[cfg(any(
    feature = "glm-coding",
    feature = "opencode-go",
    feature = "qwen-token-plan",
    feature = "deepseek"
))]
pub use channels::{verify_api_key, ApiKeyChannelConfig, ApiKeyChannelProvider};

// R5 波 A 轨 b：通道 preset 单点登记（纯数据 + 唯一 feature cfg 求值点）。
pub use channels::registry::{
    channel_preset, is_enabled, ChannelKind, ChannelPreset, OAuthFlow, OAuthPreset,
    CHANNEL_REGISTRY,
};

#[cfg(feature = "anthropic")]
pub use channels::anthropic;
#[cfg(any(
    feature = "glm-coding",
    feature = "opencode-go",
    feature = "qwen-token-plan",
    feature = "deepseek"
))]
pub use channels::api_key;
#[cfg(feature = "chatgpt-oauth")]
pub use channels::chatgpt;
#[cfg(feature = "xai-oauth")]
pub use channels::xai;

#[cfg(test)]
mod module_discipline {
    use std::fs;
    use std::path::Path;

    #[test]
    fn core_modules_do_not_reference_net_module() {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let files = [
            "registry.rs",
            "pricing.rs",
            "usage.rs",
            "negotiate.rs",
            "reasoning.rs",
            "error.rs",
        ];
        for name in files {
            let path = src.join(name);
            let contents = fs::read_to_string(&path)
                .unwrap_or_else(|err| panic!("read {}: {err}", path.display()));
            let mentions_net = contents
                .split(|character: char| !character.is_ascii_alphanumeric() && character != '_')
                .any(|identifier| identifier == "net");
            assert!(
                !mentions_net,
                "{} must not contain a reference to the providers::net module",
                path.display()
            );
        }
    }
}
