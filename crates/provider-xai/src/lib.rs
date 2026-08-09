//! xAI Grok Chat Completions adapter (P6-10).
//!
//! This crate deliberately implements only xAI's OpenAI-compatible Chat Completions
//! path. Responses API and Live Search remain outside this baseline.

mod provider;

pub use provider::{XaiConfig, XaiProvider};
pub use provider_openai_compatible::{chunk_to_events, to_chat_completions_body, ChunkState};

/// xAI API default base URL.
pub const DEFAULT_BASE_URL: &str = "https://api.x.ai/v1";
