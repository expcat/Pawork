//! Alibaba Qwen provider adapter (P6-12).
//!
//! The adapter targets DashScope's OpenAI-compatible API and delegates canonical
//! request conversion and SSE assembly to [`provider_openai_compatible`]. Qwen
//! extensions such as `enable_thinking` are passed through explicitly with
//! [`provider_api::CanonicalModelRequest::provider_options`].

mod provider;

pub use provider::{builtin_models, QwenConfig, QwenProvider};

/// DashScope OpenAI-compatible API endpoint.
pub const DEFAULT_BASE_URL: &str = "https://dashscope.aliyuncs.com/compatible-mode/v1";
