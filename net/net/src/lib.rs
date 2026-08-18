//! Pawork 共用的网络运行时与流式解析器。
//!
//! 本 crate 不包含任何具体 Provider 的业务逻辑，只提供可复用的传输与解析原语。
//!
//! # Features
//!
//! - `parsers`（默认）：SSE 解析器。零重依赖——不拉
//!   `reqwest`、`pawork-domain`。默认路径只暴露 `SseParseError`，
//!   不把解析错误映射为 `ProviderError`。JSONL / partial-JSON 已在 R0 波 B 删除。
//! - `http`：HTTP 客户端与错误归一（`reqwest` + `bytes` + `futures` +
//!   `tokio` + `pawork-domain`）。签名沿用
//!   `CancellationToken`（pawork-domain）与 `ProviderError`（pawork-domain/provider_api），
//!   不另造 NetError。本包只提供 `classify_status` / `classify_request_error` /
//!   `parse_retry_after`，不加重试循环；生产退避由 engine 单点负责。

#[cfg(feature = "parsers")]
pub mod sse;

#[cfg(feature = "http")]
pub mod http;
#[cfg(feature = "http")]
pub mod retry;
