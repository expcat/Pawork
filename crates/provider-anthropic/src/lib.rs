//! Anthropic 原生适配器（P6-2）。
//!
//! 将 canonical 请求转换为 Anthropic Messages API（`/v1/messages`），并把流式
//! SSE 事件组装回 [`ProviderStreamEvent`](provider_api::ProviderStreamEvent)，
//! 覆盖其 thinking / tool / cache / image 差异，核心 Agent 不含特例。
//!
//! - 请求转换见 [`request`]；
//! - 流式事件 → canonical 事件映射见 [`stream`]；
//! - [`AnthropicProvider`] 实现了 [`ModelProvider`](provider_api::ModelProvider)。

pub mod provider;
pub mod reasoning;
pub mod request;
pub mod server_tool;
pub mod stream;

pub use provider::{builtin_models, AnthropicConfig, AnthropicProvider};
pub use reasoning::{
    build_reasoning_item, extract_thinking_payload, reconstruct_block, AnthropicThinkingPayload,
    ANTHROPIC_BLOCK_KIND_KEY,
};
pub use request::to_messages_body;
pub use stream::{event_to_events, AnthropicStreamState};

/// Anthropic API 默认基础 URL。
pub const DEFAULT_BASE_URL: &str = "https://api.anthropic.com";

/// Anthropic API 版本（Messages API）。
pub const ANTHROPIC_VERSION: &str = "2023-06-01";
