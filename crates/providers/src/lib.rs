//! Pawork 首发模型渠道适配器。
//!
//! OpenAI-compatible Chat Completions 始终编译；S2 已有的 `anthropic` 基线继续
//! 默认开启。S6 首发范围只增加 ChatGPT OAuth、xAI Grok OAuth，以及 GLM
//! Coding Plan / OpenCode Go / Qwen Token Plan / DeepSeek 四条 API-key 通道。
//! 其它厂商留到后续需求，不预留伪实现 feature。

pub(crate) fn is_credential_header(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "authorization"
            | "proxy-authorization"
            | "api-key"
            | "x-api-key"
            | "x-goog-api-key"
    )
}

pub mod net;
pub mod provider;
pub mod request;
pub mod stream;
pub mod usage;
pub mod error_table;
pub mod memory_protector;
mod responses_reasoning;
pub mod responses;
pub mod registry;
pub mod pricing;
pub mod negotiate;
pub mod reasoning;
pub mod error;
pub mod channels;

pub use provider::{OpenAiCompatibleConfig, OpenAiCompatibleProvider};
pub use request::to_chat_completions_body;
pub use stream::{chunk_to_events, is_done, ChunkState};
pub use error_table::{normalize_vendor_error, VendorErrorRule, VENDOR_ERROR_RULES};
pub use memory_protector::InMemoryReasoningProtector;
pub use negotiate::{clamp_reasoning_to_thinking, CapabilityNegotiator};
pub use error::RegistryError;
pub use pricing::{estimate_cost, ModelPricing, BUILTIN_RATE_CARD, BUILTIN_RATE_VERSION};
pub use reasoning::{ReasoningProtectError, ReasoningProtector};
pub use registry::{
    caps, merge_capabilities, CapabilityEvidence, CapabilitySource, CatalogEntry, ModelRegistry,
    ProbeError, ProviderCapabilitySource, ProviderProbe,
};
pub use usage::{map_stop_reason, normalize_usage, UsageAccumulator};

#[cfg(feature = "anthropic")]
pub use channels::{
    builtin_models as anthropic_builtin_models, event_to_events, to_messages_body, AnthropicConfig,
    AnthropicProvider, AnthropicStreamState, ANTHROPIC_VERSION,
};

// S2 装配链仍用 `builtin_models` 指 Anthropic 静态目录；完整化后保持该别名。
#[cfg(feature = "anthropic")]
pub use channels::builtin_models;

#[cfg(feature = "chatgpt-oauth")]
pub use channels::{ChatGptConfig, ChatGptProvider};
#[cfg(feature = "chatgpt-oauth")]
pub use channels::chatgpt::DEFAULT_BASE_URL as CHATGPT_DEFAULT_BASE_URL;

#[cfg(feature = "xai-oauth")]
pub use channels::{xai_builtin_models, XaiConfig, XaiProvider};
#[cfg(feature = "xai-oauth")]
pub use channels::xai::DEFAULT_BASE_URL as XAI_DEFAULT_BASE_URL;

#[cfg(any(
    feature = "glm-coding",
    feature = "opencode-go",
    feature = "qwen-token-plan",
    feature = "deepseek"
))]
pub use channels::{ApiKeyChannel, ApiKeyChannelConfig, ApiKeyChannelProvider};

#[cfg(feature = "anthropic")]
pub use channels::anthropic;
#[cfg(feature = "chatgpt-oauth")]
pub use channels::chatgpt;
#[cfg(feature = "xai-oauth")]
pub use channels::xai;
#[cfg(any(
    feature = "glm-coding",
    feature = "opencode-go",
    feature = "qwen-token-plan",
    feature = "deepseek"
))]
pub use channels::api_key;


#[cfg(test)]
mod module_discipline {
    use std::fs;
    use std::path::Path;

    #[test]
    fn core_modules_do_not_reference_net_module() {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let files = ["registry.rs", "pricing.rs", "usage.rs", "negotiate.rs", "reasoning.rs", "error.rs"];
        for name in files {
            let path = src.join(name);
            let contents = fs::read_to_string(&path).unwrap_or_else(|err| panic!("read {}: {err}", path.display()));
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
