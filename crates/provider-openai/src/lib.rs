//! OpenAI 原生适配器（P6-1）。
//!
//! 面向 OpenAI 官方 Chat Completions API 的适配器。OpenAI 协议即 Chat Completions
//! 协议本身，故本 crate 复用 [`provider_openai_compatible`] 的 canonical↔OpenAI
//! 转换与流式组装，并在其上提供：OpenAI 默认端点、`openai` provider 标识、内置
//! 模型目录（含 reasoning / image / tool 能力）。
//!
//! 核心能力由底层兼容引擎统一处理：
//! - 图片输入（P6-6，[`provider_openai_compatible::to_chat_completions_body`]）；
//! - thinking / reasoning 流（P6-5，[`provider_openai_compatible::chunk_to_events`]）；
//! - 结构化输出（P6-8，`response_format`）；
//! - provider_options 透传（P6-9）；
//! - prompt cache 自动命中（P6-7，OpenAI 为自动命中，usage 中体现）。

pub mod provider;

pub use provider::{builtin_models, OpenAiConfig, OpenAiProvider};

/// OpenAI 官方 API 默认基础 URL。
pub const DEFAULT_BASE_URL: &str = "https://api.openai.com/v1";
