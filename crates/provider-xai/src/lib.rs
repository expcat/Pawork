//! xAI Grok adapter（Chat Completions + Responses 双传输，P6-10 / P15-4）。
//!
//! 双传输并存（P15-4）：模型声明 Responses transport 时走 xAI Responses 子适配器，
//! 否则降级到 P6-10 Chat Completions；transport 由 P15-8 CapabilityNegotiator
//! 选择，不在 Core 走 xAI 名称分支。reasoning `encrypted_content` 只经
//! [`responses::ReasoningProtector`] 边界往返（ADR-032）。

pub mod provider;
pub mod reasoning;
pub mod responses;
pub mod server_tool;

pub use provider::{builtin_models, XaiConfig, XaiProvider};
pub use provider_openai_compatible::{chunk_to_events, to_chat_completions_body, ChunkState};
pub use provider_runtime::reasoning::{
    InMemoryReasoningProtector, ProtectedBlobStoreProtector, ReasoningProtectError,
    ReasoningProtector,
};
pub use responses::{
    live_search_source_to_source, normalize_responses_error, requirements_from_request,
    to_responses_body, AcceptedResponsesTools, ResponsesAssemblyEvent, ResponsesFinalState,
    ResponsesStreamAssembler,
};

/// xAI API default base URL.
pub const DEFAULT_BASE_URL: &str = "https://api.x.ai/v1";
