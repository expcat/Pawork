//! Zhipu GLM Chat Completions adapter (P6-11).
//!
//! The adapter targets BigModel's OpenAI-compatible v4 API and maps streamed
//! `reasoning_content` through the canonical thinking event path.

mod provider;

pub use provider::{ZhipuConfig, ZhipuProvider};
pub use provider_openai_compatible::{chunk_to_events, to_chat_completions_body, ChunkState};

/// BigModel v4 default base URL.
pub const DEFAULT_BASE_URL: &str = "https://open.bigmodel.cn/api/paas/v4";
