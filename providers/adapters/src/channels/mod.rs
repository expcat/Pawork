//! 首发六通道适配器内聚入口。
//!
//! feature 名与门控保持原 adapters `lib.rs` 形状，根 crate 继续 re-export
//! `ApiKeyChannel` / `ChatGptProvider` 等既有对外路径。

#[cfg(feature = "anthropic")]
pub mod anthropic;

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

#[cfg(feature = "anthropic")]
pub use anthropic::{
    builtin_models, event_to_events, to_messages_body, AnthropicConfig, AnthropicProvider,
    AnthropicStreamState, ANTHROPIC_VERSION,
};

#[cfg(feature = "chatgpt-oauth")]
pub use chatgpt::{ChatGptConfig, ChatGptProvider};

#[cfg(feature = "xai-oauth")]
pub use xai::{builtin_models as xai_builtin_models, XaiConfig, XaiProvider};

#[cfg(any(
    feature = "glm-coding",
    feature = "opencode-go",
    feature = "qwen-token-plan",
    feature = "deepseek"
))]
pub use api_key::{ApiKeyChannel, ApiKeyChannelConfig, ApiKeyChannelProvider};
