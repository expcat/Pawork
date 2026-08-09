//! Moonshot Kimi provider adapter (P6-13).
//!
//! Moonshot's chat endpoint is OpenAI-compatible, so canonical requests, Bearer
//! authentication and SSE assembly are delegated to [`provider_openai_compatible`].
//! Kimi `reasoning_content` deltas are thereby exposed as canonical
//! [`provider_api::ProviderStreamEvent::ThinkingDelta`] events.

mod provider;

pub use provider::{builtin_models, MoonshotConfig, MoonshotProvider};

/// Moonshot OpenAI-compatible API endpoint.
pub const DEFAULT_BASE_URL: &str = "https://api.moonshot.cn/v1";
