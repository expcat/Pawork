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

pub mod provider;
pub mod request;
pub mod stream;
mod usage;
pub mod error_table;

#[cfg(feature = "anthropic")]
pub mod anthropic;

pub mod memory_protector;

mod responses_reasoning;

pub mod responses;

#[cfg(feature = "chatgpt-oauth")]
pub mod chatgpt;

#[cfg(feature = "xai-oauth")]
pub mod xai;

#[cfg(any(
    feature = "glm-coding",
    feature = "opencode-go",
    feature = "qwen-token-plan",
    feature = "deepseek"
))]
pub mod api_key;

pub use provider::{OpenAiCompatibleConfig, OpenAiCompatibleProvider};
pub use request::to_chat_completions_body;
pub use stream::{chunk_to_events, is_done, ChunkState};
pub use error_table::{normalize_vendor_error, VendorErrorRule, VENDOR_ERROR_RULES};

pub use memory_protector::InMemoryReasoningProtector;

#[cfg(feature = "anthropic")]
pub use anthropic::{
    builtin_models as anthropic_builtin_models, event_to_events, to_messages_body, AnthropicConfig,
    AnthropicProvider, AnthropicStreamState, ANTHROPIC_VERSION,
};

// S2 装配链仍用 `builtin_models` 指 Anthropic 静态目录；完整化后保持该别名。
#[cfg(feature = "anthropic")]
pub use anthropic::builtin_models;

#[cfg(feature = "chatgpt-oauth")]
pub use chatgpt::{
    ChatGptConfig, ChatGptProvider, DEFAULT_BASE_URL as CHATGPT_DEFAULT_BASE_URL,
};

#[cfg(feature = "xai-oauth")]
pub use xai::{
    builtin_models as xai_builtin_models, XaiConfig, XaiProvider,
    DEFAULT_BASE_URL as XAI_DEFAULT_BASE_URL,
};

#[cfg(any(
    feature = "glm-coding",
    feature = "opencode-go",
    feature = "qwen-token-plan",
    feature = "deepseek"
))]
pub use api_key::{ApiKeyChannel, ApiKeyChannelConfig, ApiKeyChannelProvider};
