//! Pawork 共用的网络运行时与流式解析器。
//!
//! 原 `pawork-net` 已并入 `pawork-providers`。HTTP 在包内常开，不再单独成
//! `http` / `parsers` feature；SSE 解析器始终可用，并保留 `SseParseError` →
//! `ProviderError` 映射。

pub mod http;
pub mod retry;
pub mod sse;

pub(crate) fn redact_url_origin(url: &reqwest::Url) -> String {
    let scheme = url.scheme();
    let host = url.host_str().unwrap_or("invalid-host");
    match url.port() {
        Some(port) => format!("{scheme}://{host}:{port}"),
        None => format!("{scheme}://{host}"),
    }
}
