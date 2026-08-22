//! Anthropic Messages 基线适配器（S2）。
//!
//! 将 canonical 请求转换为 Anthropic Messages API（`/v1/messages`），并把流式
//! SSE 事件组装回 [`ProviderStreamEvent`](pawork_domain::ProviderStreamEvent)。
//! 覆盖 text / `tool_use` / `tool_result`，以及 K-10 的 prompt cache、thinking、
//! signature continuation、server_tool 与 citations。未声明的 hosted tools 在
//! 请求前拒绝，不静默丢弃。`base_url` 必填，不内置官方端点。

pub mod provider;
pub mod request;
pub mod stream;

pub use provider::{builtin_models, AnthropicConfig, AnthropicProvider};
pub use request::{to_messages_body, to_messages_body_with_plan, MessagesWirePlan};
pub use stream::{event_to_events, parse_event, AnthropicStreamState, StreamOutput};

/// Anthropic Messages 协议版本（请求头 `anthropic-version`，不是端点）。
pub const ANTHROPIC_VERSION: &str = "2023-06-01";
