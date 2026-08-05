//! OpenAI-compatible 适配器（P2-5）。
//!
//! 将 canonical 请求转换为 OpenAI Chat Completions 协议，并把流式响应
//! 组装回 [`ProviderStreamEvent`](provider_api::ProviderStreamEvent)，可同时
//! 覆盖云端 OpenAI 兼容接口与多数本地服务（Ollama / vLLM / LM Studio）。
//!
//! - 请求转换见 [`request`]；
//! - 流式 chunk → 事件映射见 [`stream`]；
//! - [`OpenAiCompatibleProvider`] 实现了 [`ModelProvider`](provider_api::ModelProvider)。

pub mod provider;
pub mod request;
pub mod stream;

pub use provider::{OpenAiCompatibleConfig, OpenAiCompatibleProvider};
pub use request::to_chat_completions_body;
pub use stream::{chunk_to_events, is_done, ChunkState};
