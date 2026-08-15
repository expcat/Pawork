//! Anthropic Messages 基线适配器（S2）。
//!
//! 将 canonical 请求转换为 Anthropic Messages API（`/v1/messages`），并把流式
//! SSE 事件组装回 [`ProviderStreamEvent`](pawork_api::ProviderStreamEvent)。
//! 本波只覆盖 text / `tool_use` / `tool_result`；`base_url` 必填，不内置官方端点。
//!
//! TODO(S6): prompt cache（`cache_control`）、thinking 请求体、hosted tools、
//! modern Messages（signature / server_tool / citations）。本波字段闲置、不写 wire。

pub mod provider;
pub mod request;
pub mod stream;

pub use provider::{builtin_models, AnthropicConfig, AnthropicProvider};
pub use request::to_messages_body;
pub use stream::{event_to_events, AnthropicStreamState};

/// Anthropic Messages 协议版本（请求头 `anthropic-version`，不是端点）。
pub const ANTHROPIC_VERSION: &str = "2023-06-01";
