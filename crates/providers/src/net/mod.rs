//! Pawork 共用的网络运行时与流式解析器。
//!
//! 原 `pawork-net` 已并入 `pawork-providers`。HTTP 在包内常开，不再单独成
//! `http` / `parsers` feature；SSE 解析器始终可用，并保留 `SseParseError` →
//! `ProviderError` 映射。

pub mod sse;
pub mod http;
pub mod retry;
