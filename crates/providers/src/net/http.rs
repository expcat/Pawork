//! HTTP 运行时（P2-1）。
//!
//! 跨平台的统一 HTTP 客户端：超时、代理、自定义 header、trace ID 贯穿与
//! 请求取消，作为所有 Provider 网络访问的统一底层。

use std::fmt;
use std::time::Duration;

use bytes::Bytes;
use futures::{Stream, StreamExt};
use pawork_domain::CancellationToken;
use pawork_domain::{ProviderError, ProviderErrorKind};
use std::pin::Pin;

use crate::net::retry::{classify_request_error, classify_status};

/// HTTP 客户端配置。
#[derive(Clone)]
pub struct HttpClientConfig {
    /// 建立连接及单次读操作的超时；每次成功读取后重置读计时，`None` 表示不限。
    ///
    /// 流式响应没有总时长上限，只有在连续超过该时长未收到新数据时才超时。
    pub timeout: Option<Duration>,
    /// HTTP(S) 代理地址（如 `http://proxy:8080`）。
    pub proxy: Option<String>,
    /// 自定义 User-Agent。
    pub user_agent: Option<String>,
    /// 额外固定请求头（每次请求都会附加）。
    pub extra_headers: Vec<(String, String)>,
    /// 是否读取系统代理环境变量（默认 true）。测试中可设 false 以避免环境干扰。
    pub system_proxy: bool,
}

impl fmt::Debug for HttpClientConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HttpClientConfig")
            .field("timeout", &self.timeout)
            .field("proxy", &self.proxy)
            .field("user_agent", &self.user_agent)
            .field("extra_headers", &RedactedHeaders(&self.extra_headers))
            .field("system_proxy", &self.system_proxy)
            .finish()
    }
}

/// Debug 只保留 header 键名，值一律脱敏，避免 extra_headers 里的 token 入日志。
struct RedactedHeaders<'a>(&'a [(String, String)]);

impl fmt::Debug for RedactedHeaders<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_list()
            .entries(self.0.iter().map(|(name, _)| (name.as_str(), "[REDACTED]")))
            .finish()
    }
}

impl Default for HttpClientConfig {
    fn default() -> Self {
        Self {
            timeout: Some(Duration::from_secs(60)),
            proxy: None,
            user_agent: Some("pawork".to_string()),
            extra_headers: Vec::new(),
            system_proxy: true,
        }
    }
}

impl HttpClientConfig {
    pub fn builder() -> HttpClientConfigBuilder {
        HttpClientConfigBuilder(HttpClientConfig::default())
    }
}

/// [`HttpClientConfig`] 的构建器。
pub struct HttpClientConfigBuilder(HttpClientConfig);

impl HttpClientConfigBuilder {
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.0.timeout = Some(timeout);
        self
    }
    pub fn no_timeout(mut self) -> Self {
        self.0.timeout = None;
        self
    }
    pub fn proxy(mut self, proxy: impl Into<String>) -> Self {
        self.0.proxy = Some(proxy.into());
        self
    }
    pub fn user_agent(mut self, ua: impl Into<String>) -> Self {
        self.0.user_agent = Some(ua.into());
        self
    }
    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.0.extra_headers.push((name.into(), value.into()));
        self
    }
    pub fn disable_system_proxy(mut self) -> Self {
        self.0.system_proxy = false;
        self
    }
    pub fn build(self) -> HttpClientConfig {
        self.0
    }
}

/// 由 reqwest 错误构造客户端失败时的归一化错误。
fn http_error(err: reqwest::Error) -> ProviderError {
    classify_request_error(err)
}

/// 统一 HTTP 客户端。
pub struct HttpClient {
    client: reqwest::Client,
    config: HttpClientConfig,
}

/// 字节流（由 [`HttpClient::post_stream`] 返回）。
pub type ByteStream = Pin<Box<dyn Stream<Item = Result<Bytes, ProviderError>> + Send>>;

impl HttpClient {
    /// 按配置构造客户端。
    pub fn new(config: HttpClientConfig) -> Result<Self, ProviderError> {
        let mut builder = reqwest::Client::builder();

        if let Some(timeout) = config.timeout {
            builder = builder.connect_timeout(timeout).read_timeout(timeout);
        }
        if let Some(proxy) = &config.proxy {
            let proxy = loopback_aware_proxy(proxy)
                .map_err(|err| ProviderError::new(ProviderErrorKind::InvalidRequest, err))?;
            builder = builder.proxy(proxy);
        } else if !config.system_proxy {
            builder = builder.no_proxy();
        }
        if let Some(user_agent) = &config.user_agent {
            builder = builder.user_agent(user_agent.clone());
        }

        // Cross-origin redirects must fail closed. reqwest's default policy
        // follows 10 hops and only strips Authorization/Cookie, not x-api-key.
        builder = builder.redirect(reqwest::redirect::Policy::none());

        let client = builder.build().map_err(http_error)?;
        Ok(Self { client, config })
    }

    /// 引用的底层 reqwest 客户端（供 adapter 复用连接池等高级用法）。
    pub fn inner(&self) -> &reqwest::Client {
        &self.client
    }

    /// 访问配置（只读）。
    pub fn config(&self) -> &HttpClientConfig {
        &self.config
    }

    /// 发起 POST 流式请求，返回字节流。
    ///
    /// - trace_id 注入为 `x-trace-id` 请求头；
    /// - 在拿到响应头与取消令牌之间竞争，取消即返回 [`ProviderError::cancelled`]；
    /// - 非 2xx 响应经 [`classify_status`](crate::net::retry::classify_status) 归一为 ProviderError。
    pub async fn post_stream(
        &self,
        url: &str,
        body: serde_json::Value,
        trace_id: Option<&str>,
        cancel: CancellationToken,
    ) -> Result<ByteStream, ProviderError> {
        self.post_stream_with_headers(url, body, trace_id, &[], cancel)
            .await
    }

    /// 与 [`post_stream`](Self::post_stream) 相同，但额外附加 per-request 请求头
    /// （如 Provider 认证头；明文 secret 只在此短暂存在，不持久化、不记录）。
    pub async fn post_stream_with_headers(
        &self,
        url: &str,
        body: serde_json::Value,
        trace_id: Option<&str>,
        per_request_headers: &[(String, String)],
        cancel: CancellationToken,
    ) -> Result<ByteStream, ProviderError> {
        // 构造请求
        let mut request = self.client.post(url).json(&body);
        for (name, value) in &self.config.extra_headers {
            request = request.header(name, value);
        }
        for (name, value) in per_request_headers {
            request = request.header(name, value);
        }
        if let Some(trace) = trace_id {
            request = request.header("x-trace-id", trace);
        }

        // 在「发送请求」与「取消」之间竞争
        let send_fut = request.send();
        tokio::pin!(send_fut);
        tokio::select! {
            biased;
            _ = cancel.cancelled() => Err(ProviderError::cancelled("http request cancelled")),
            response = &mut send_fut => {
                let response = response.map_err(http_error)?;
                self.handle_response(response).await
            }
        }
    }

    /// 发起 GET 请求并返回 JSON 正文（用于 list_models 等）。
    #[allow(clippy::needless_return)]
    pub async fn get_json(
        &self,
        url: &str,
        trace_id: Option<&str>,
        cancel: CancellationToken,
    ) -> Result<serde_json::Value, ProviderError> {
        self.get_json_with_headers(url, trace_id, &[], cancel).await
    }

    /// 与 [`get_json`](Self::get_json) 相同，但额外附加 per-request 请求头。
    #[allow(clippy::needless_return)]
    pub async fn get_json_with_headers(
        &self,
        url: &str,
        trace_id: Option<&str>,
        per_request_headers: &[(String, String)],
        cancel: CancellationToken,
    ) -> Result<serde_json::Value, ProviderError> {
        let mut request = self.client.get(url);
        for (name, value) in &self.config.extra_headers {
            request = request.header(name, value);
        }
        for (name, value) in per_request_headers {
            request = request.header(name, value);
        }
        if let Some(trace) = trace_id {
            request = request.header("x-trace-id", trace);
        }

        let send_fut = request.send();
        tokio::pin!(send_fut);
        tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                return Err(ProviderError::cancelled("http request cancelled"));
            }
            response = &mut send_fut => {
                let response = response.map_err(http_error)?;
                if !response.status().is_success() {
                    let status = response.status();
                    let retry_after = response
                        .headers()
                        .get(reqwest::header::RETRY_AFTER)
                        .and_then(|v| v.to_str().ok())
                        .map(str::to_owned);
                    let body = response.text().await.unwrap_or_default();
                    let snippet = truncate(&body, 512);
                    return Err(classify_status(status, retry_after.as_deref(), &snippet));
                }
                let value = response
                    .json::<serde_json::Value>()
                    .await
                    .map_err(http_error)?;
                Ok(value)
            }
        }
    }

    async fn handle_response(
        &self,
        response: reqwest::Response,
    ) -> Result<ByteStream, ProviderError> {
        if !response.status().is_success() {
            let status = response.status();
            let retry_after = response
                .headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned);
            let body = response.text().await.unwrap_or_default();
            let snippet = truncate(&body, 512);
            return Err(classify_status(status, retry_after.as_deref(), &snippet));
        }

        // 将响应体转为字节流；读取期间的 IO 错误归一为 StreamInterrupted。
        let stream = response
            .bytes_stream()
            .map(|result| result.map_err(http_error));
        Ok(Box::pin(stream))
    }
}

/// 判断目标 host 是否为本机/回环（显式代理不应劫持本地网关流量）。
pub fn is_local_target(host: &str) -> bool {
    matches!(
        host,
        "localhost" | "127.0.0.1" | "::1" | "[::1]" | "0.0.0.0"
    ) || host.ends_with(".local")
        || host.ends_with(".localhost")
}

/// 构造回环感知代理：远端目标走 proxy，本机/回环目标直连。
///
/// 参照 CLIProxyAPI `proxy-url` 语义：代理只服务出站上游请求，
/// `http://127.0.0.1:xxxx` 等本地端点保持直连，避免全局代理破坏本地网关。
pub fn loopback_aware_proxy(proxy: &str) -> Result<reqwest::Proxy, String> {
    let parsed: reqwest::Url = proxy
        .parse()
        .map_err(|err| format!("invalid proxy {proxy:?}: {err}"))?;
    Ok(reqwest::Proxy::custom(move |url| {
        if is_local_target(url.host_str().unwrap_or_default()) {
            None
        } else {
            Some(parsed.clone())
        }
    }))
}

/// 截断字符串到指定字节长度（在 UTF-8 边界安全处）。
fn truncate(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_string();
    }
    let mut end = max_bytes;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &value[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_respects_utf8_boundary() {
        // 中文每字 3 字节，截到 4 字节应回退到 3
        let out = truncate("你好世界", 4);
        assert!(out.ends_with('…'));
        assert!(out.chars().count() >= 1);
    }

    #[test]
    fn local_targets_are_detected() {
        for host in ["localhost", "127.0.0.1", "::1", "gateway.local"] {
            assert!(is_local_target(host), "{host} should be local");
        }
        for host in ["auth.openai.com", "api.z.ai", "example.com"] {
            assert!(!is_local_target(host), "{host} should be remote");
        }
    }

    #[test]
    fn loopback_aware_proxy_validates_url() {
        assert!(loopback_aware_proxy("http://127.0.0.1:38081").is_ok());
        assert!(loopback_aware_proxy("socks5://127.0.0.1:1080").is_ok());
        assert!(loopback_aware_proxy("not a url").is_err());
    }

    #[test]
    fn config_builder_sets_fields() {
        let config = HttpClientConfig::builder()
            .timeout(Duration::from_secs(10))
            .user_agent("test")
            .header("x-custom", "v")
            .build();
        assert_eq!(config.timeout, Some(Duration::from_secs(10)));
        assert_eq!(config.user_agent.as_deref(), Some("test"));
        assert_eq!(config.extra_headers, vec![("x-custom".into(), "v".into())]);
    }

    #[test]
    fn client_constructs_with_default_config() {
        let client = HttpClient::new(HttpClientConfig::default()).expect("构造客户端");
        assert_eq!(client.config().timeout, Some(Duration::from_secs(60)));
    }

    #[test]
    fn extra_headers_debug_redacts_values_and_keeps_names() {
        let config = HttpClientConfig::builder()
            .header("x-api-key", "sk-secret-plaintext")
            .header("X-Custom", "also-secret")
            .build();
        let debug = format!("{config:?}");
        assert!(debug.contains("x-api-key"), "{debug}");
        assert!(debug.contains("X-Custom"), "{debug}");
        assert!(debug.contains("[REDACTED]"), "{debug}");
        assert!(
            !debug.contains("sk-secret-plaintext"),
            "header value must be redacted: {debug}"
        );
        assert!(
            !debug.contains("also-secret"),
            "header value must be redacted: {debug}"
        );
    }
}
