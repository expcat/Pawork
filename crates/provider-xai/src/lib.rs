//! xAI Grok Chat Completions adapter (P6-10).
//!
//! The production adapter still implements xAI's OpenAI-compatible Chat Completions
//! path. P15-5 additionally freezes Responses server-tool/citation mapping fixtures;
//! P15-7 freezes the Responses reasoning continuation mapping. The actual
//! Responses transport lands in P15-4.

mod provider;
pub mod reasoning;
pub mod server_tool;

pub use provider::{XaiConfig, XaiProvider};
pub use provider_openai_compatible::{chunk_to_events, to_chat_completions_body, ChunkState};

/// xAI API default base URL.
pub const DEFAULT_BASE_URL: &str = "https://api.x.ai/v1";
