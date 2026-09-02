//! 首发六通道适配器内聚入口。
//!
//! feature 名与门控保持原 adapters `lib.rs` 形状，根 crate 继续 re-export
//! `ApiKeyChannelConfig` / `ChatGptProvider` 等既有对外路径。
//! 通道 preset 自 R5 波 A 起单点登记在 registry（纯数据，行不带 cfg；
//! ApiKeyChannel 枚举已删除，由注册表行驱动）。

pub mod registry;

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
    feature = "deepseek",
    feature = "kimi-platform"
))]
pub mod api_key;

#[cfg(feature = "kimi-code")]
pub mod kimi;

#[cfg(feature = "anthropic")]
pub use anthropic::{
    builtin_models, event_to_events, parse_event, to_messages_body, to_messages_body_with_plan,
    AnthropicConfig, AnthropicProvider, AnthropicStreamState, MessagesWirePlan, StreamOutput,
    ANTHROPIC_VERSION,
};

#[cfg(feature = "chatgpt-oauth")]
pub use chatgpt::{ChatGptConfig, ChatGptProvider};

#[cfg(feature = "xai-oauth")]
pub use xai::{builtin_models as xai_builtin_models, XaiConfig, XaiProvider};

#[cfg(any(
    feature = "glm-coding",
    feature = "opencode-go",
    feature = "qwen-token-plan",
    feature = "deepseek",
    feature = "kimi-platform"
))]
pub use api_key::{verify_api_key, ApiKeyChannelConfig, ApiKeyChannelProvider};

#[cfg(feature = "kimi-code")]
pub use kimi::{builtin_models as kimi_code_builtin_models, KimiCodeConfig, KimiCodeProvider};

pub use registry::{
    channel_preset, is_enabled, ChannelKind, ChannelPreset, OAuthFlow, OAuthPreset,
    CHANNEL_REGISTRY,
};
