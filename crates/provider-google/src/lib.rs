//! Google Gemini 原生适配器（P6-3）。
//!
//! 将 canonical 请求转换为 Google Gemini `generateContent` 协议（流式走
//! `streamGenerateContent?alt=sse`），并把 SSE chunk 组装回
//! [`ProviderStreamEvent`](provider_api::ProviderStreamEvent)。
//!
//! - 请求转换见 [`request`]；
//! - 流式 chunk → 事件映射见 [`stream`]；
//! - [`GoogleProvider`] 实现了 [`ModelProvider`](provider_api::ModelProvider)。

pub mod provider;
pub mod request;
pub mod stream;

pub use provider::{builtin_models, GoogleConfig, GoogleProvider};
pub use request::to_generate_content_body;
pub use stream::{chunk_to_events, ChunkState};

/// Google Gemini API 默认基础 URL。
pub const DEFAULT_BASE_URL: &str = "https://generativelanguage.googleapis.com";
