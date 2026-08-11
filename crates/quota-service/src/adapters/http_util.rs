//! 跨适配器共享的 HTTP / 货币工具。
//!
//! 所有 Provider 配额接口的底层取数都经这里的小工具收口：把
//! [`provider_api::ProviderError`] 归一到 [`QuotaError`]，把端点 URL 中的
//! query string 从 provenance 中抹掉，并为 WebScrape 提供不含明文 cookie /
//! 原始 HTML 的审计片段。
//!
//! 时间与脱敏（[`now_millis`] / [`redact_endpoint`] / [`redact_secrets`]）的
//! 唯一实现位于 [`crate::util`]，此处仅作 re-export 保持既有调用面不变。

use agent_domain::CancellationToken;
use provider_api::{ProviderError, ProviderErrorKind, ResolvedCredential};

use crate::QuotaError;

use provider_runtime::http::HttpClient;
use provider_runtime::retry::{classify_request_error, classify_status};

/// 非 2xx 响应的固定安全描述。响应正文永不读取/复制，错误文本只含该描述与
/// typed 字段（kind / status / Retry-After）。
const ERROR_SNIPPET: &str = "quota endpoint returned an error";

/// 发起一次 JSON GET，把 [`ProviderError`] 归一为 [`QuotaError`]。
///
/// 复用 [`HttpClient::inner`] 的底层客户端与 provider-runtime 的
/// [`classify_status`] / [`classify_request_error`] 语义；非 2xx（含
/// 401/403/429）绝不读取或复制响应正文，仅保留状态码、Retry-After 与固定
/// 安全描述。网络 / 解码错误只保留 typed kind，不携带 URL/query、header、
/// cookie 或原始正文。
pub async fn api_get(
    http: &HttpClient,
    url: &str,
    headers: &[(String, String)],
    cancel: &CancellationToken,
) -> Result<serde_json::Value, QuotaError> {
    let mut request = http.inner().get(url);
    for (name, value) in headers {
        request = request.header(name, value);
    }

    let send = request.send();
    tokio::pin!(send);
    let response = tokio::select! {
        biased;
        _ = cancel.cancelled() => return Err(QuotaError::Cancelled),
        response = &mut send => {
            response.map_err(request_error_to_quota_error)?
        }
    };

    let status = response.status();
    let retry_after = response
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    if !status.is_success() {
        // Error pages can echo cookies, account metadata, or injected HTML.
        // Status and Retry-After are sufficient for typed classification, so
        // never read or copy a response body into an error or audit string.
        let provider_err = classify_status(status, retry_after.as_deref(), ERROR_SNIPPET);
        return Err(provider_error_to_quota_error(provider_err));
    }
    let read_body = response.json::<serde_json::Value>();
    tokio::pin!(read_body);
    tokio::select! {
        biased;
        _ = cancel.cancelled() => Err(QuotaError::Cancelled),
        body = &mut read_body => body.map_err(request_error_to_quota_error),
    }
}

/// 发起一次纯文本 GET（用于 WebScrape 的 HTML 抓取）。
///
/// 该路径绕过 [`HttpClient::get_json_with_headers`]，直接使用底层 reqwest
/// 客户端，自行与取消令牌竞争，并复用同一套状态码 → 错误映射。读取到的
/// 原始正文只在本调用栈中短暂存在，调用方解析后必须丢弃，不得缓存或记录。
/// 非 2xx 响应正文同样永不读取/复制；正文读取/解码失败只保留 typed kind
/// 与固定安全描述。
pub async fn api_get_text(
    http: &HttpClient,
    url: &str,
    headers: &[(String, String)],
    cancel: &CancellationToken,
) -> Result<String, QuotaError> {
    let mut request = http.inner().get(url);
    for (name, value) in headers {
        request = request.header(name, value);
    }

    let send = request.send();
    tokio::pin!(send);
    let response = tokio::select! {
        biased;
        _ = cancel.cancelled() => return Err(QuotaError::Cancelled),
        response = &mut send => {
            response.map_err(request_error_to_quota_error)?
        }
    };

    let status = response.status();
    let retry_after = response
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    if !status.is_success() {
        // Error pages can echo cookies, account metadata, or injected HTML.
        // Status and Retry-After are sufficient for typed classification, so
        // never copy a response body into an error or audit string.
        let provider_err = classify_status(status, retry_after.as_deref(), ERROR_SNIPPET);
        return Err(provider_error_to_quota_error(provider_err));
    }
    let read_body = response.text();
    tokio::pin!(read_body);
    tokio::select! {
        biased;
        _ = cancel.cancelled() => Err(QuotaError::Cancelled),
        body = &mut read_body => body.map_err(request_error_to_quota_error),
    }
}

/// 把请求阶段（连接 / 发送 / 超时 / 解码）的 [`reqwest::Error`] 归一为配额错误。
///
/// 复用 provider-runtime 的 [`classify_request_error`] 判定 kind，但 detail
/// 一律替换为固定安全描述：reqwest 错误消息可能携带完整 URL/query，绝不能
/// 进入错误或审计文本。解码失败（含 Malformed JSON）映射为 [`QuotaError::Parse`]，
/// 其余 kind 继续走 [`provider_error_to_quota_error`]，保留 status /
/// retry_after 等 typed 字段。
fn request_error_to_quota_error(error: reqwest::Error) -> QuotaError {
    let mut provider_err = classify_request_error(error);
    provider_err.message = match provider_err.kind {
        ProviderErrorKind::Timeout => "quota endpoint request timed out",
        ProviderErrorKind::Network => "quota endpoint unreachable (network error)",
        ProviderErrorKind::StreamInterrupted => "quota response could not be decoded",
        ProviderErrorKind::InvalidRequest => "quota endpoint request could not be built",
        _ => "quota endpoint request failed",
    }
    .to_string();

    if matches!(provider_err.kind, ProviderErrorKind::StreamInterrupted) {
        return QuotaError::Parse {
            detail: provider_err.message,
        };
    }
    provider_error_to_quota_error(provider_err)
}

/// 时间与脱敏的单一事实源（实现见 [`crate::util`]）。
pub use crate::util::{now_millis, redact_endpoint, redact_secrets};

/// 把 [`provider_api::ProviderError`] 归一为配额错误。
///
/// 映射保持语义可区分：401→`Unauthorized`、403→`Forbidden`、
/// 429→`RateLimited`（保留 `retry_after_ms`）、404→`Unsupported`
/// （远端资源不存在视作该作用域不提供配额读数）、取消→`Cancelled`、
/// 解码失败→`Parse`，其余视作 `Other`。所有 detail 都经 [`redact_secrets`]
/// 处理，避免把远端正文里的潜在 token 写进错误消息。
pub fn provider_error_to_quota_error(error: ProviderError) -> QuotaError {
    if matches!(
        error.kind,
        ProviderErrorKind::Timeout
            | ProviderErrorKind::Network
            | ProviderErrorKind::ProviderUnavailable
            | ProviderErrorKind::StreamInterrupted
    ) || error
        .http_status
        .map(|s| (500..600).contains(&s))
        .unwrap_or(false)
    {
        return transient_error(error);
    }

    match error.kind {
        ProviderErrorKind::Cancelled => QuotaError::Cancelled,
        ProviderErrorKind::MalformedResponse | ProviderErrorKind::StreamInterrupted => {
            QuotaError::Parse {
                detail: redact_secrets(&error.message),
            }
        }
        ProviderErrorKind::Authentication => {
            QuotaError::unauthorized(redact_secrets(&error.message))
        }
        ProviderErrorKind::Authorization => QuotaError::forbidden(redact_secrets(&error.message)),
        ProviderErrorKind::RateLimited => {
            QuotaError::rate_limited(redact_secrets(&error.message), error.retry_after_ms)
        }
        _ => match error.http_status {
            Some(401) => QuotaError::unauthorized(redact_secrets(&error.message)),
            Some(403) => QuotaError::forbidden(redact_secrets(&error.message)),
            Some(429) => {
                QuotaError::rate_limited(redact_secrets(&error.message), error.retry_after_ms)
            }
            Some(404) => QuotaError::unsupported(redact_secrets(&error.message)),
            Some(status) if (400..500).contains(&status) => {
                QuotaError::other(redact_secrets(&format!("HTTP {status}: {}", error.message)))
            }
            _ => QuotaError::other(redact_secrets(&error.message)),
        },
    }
}

/// 归一化「网络 / 连接 / 5xx / 流中断」类错误。
///
pub fn transient_error(error: ProviderError) -> QuotaError {
    let detail = redact_secrets(&error.message);
    let status = error.http_status;
    let retry_after_ms = error.retry_after_ms;
    match error.kind {
        ProviderErrorKind::Timeout => QuotaError::Timeout {
            detail,
            status,
            retry_after_ms,
        },
        ProviderErrorKind::Network
        | ProviderErrorKind::ProviderUnavailable
        | ProviderErrorKind::StreamInterrupted => {
            QuotaError::transient(detail, status, retry_after_ms)
        }
        _ if status.is_some_and(|status| (500..600).contains(&status)) => {
            QuotaError::transient(detail, status, retry_after_ms)
        }
        _ => QuotaError::other(detail),
    }
}

/// 把 API key 包装成标准的 `Authorization: Bearer <key>` 头。
pub fn bearer_headers(credential: &ResolvedCredential) -> Vec<(String, String)> {
    vec![(
        "Authorization".to_string(),
        format!("Bearer {}", credential.expose_secret()),
    )]
}

/// 单次带取消竞争的睡眠。用于 WebScrape 的最小请求间隔。
pub async fn sleep_or_cancel(
    duration: std::time::Duration,
    cancel: &CancellationToken,
) -> Result<(), QuotaError> {
    if duration.is_zero() {
        return if cancel.is_cancelled() {
            Err(QuotaError::Cancelled)
        } else {
            Ok(())
        };
    }
    tokio::select! {
        biased;
        _ = cancel.cancelled() => Err(QuotaError::Cancelled),
        _ = tokio::time::sleep(duration) => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use std::{
        io::{Read, Write},
        net::TcpListener,
        sync::mpsc,
        thread,
        time::Duration,
    };

    use super::*;
    use provider_api::CredentialKind;
    use tokio::sync::oneshot;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[test]
    fn maps_status_codes_distinctly() {
        let e1 = ProviderError {
            http_status: Some(401),
            ..ProviderError::new(ProviderErrorKind::Authentication, "bad key")
        };
        assert!(matches!(
            provider_error_to_quota_error(e1),
            QuotaError::Unauthorized { .. }
        ));

        let e2 = ProviderError {
            http_status: Some(403),
            ..ProviderError::new(ProviderErrorKind::Authorization, "no")
        };
        assert!(matches!(
            provider_error_to_quota_error(e2),
            QuotaError::Forbidden { .. }
        ));

        let e3 = ProviderError {
            http_status: Some(429),
            retry_after_ms: Some(2_000),
            ..ProviderError::new(ProviderErrorKind::RateLimited, "slow")
        };
        assert!(matches!(
            provider_error_to_quota_error(e3),
            QuotaError::RateLimited {
                retry_after_ms: Some(2_000),
                ..
            }
        ));

        let e4 = ProviderError {
            http_status: Some(404),
            ..ProviderError::new(ProviderErrorKind::ModelNotFound, "missing")
        };
        assert!(matches!(
            provider_error_to_quota_error(e4),
            QuotaError::Unsupported { .. }
        ));
    }

    #[test]
    fn endpoint_redaction_strips_query() {
        let url = "https://api.openai.com/v1/organization/costs?start_time=1&limit=10";
        assert_eq!(
            redact_endpoint(url),
            "https://api.openai.com/v1/organization/costs"
        );
    }

    #[test]
    fn secret_masking_replaces_token_like_chunks() {
        let msg = "key=sk-abcdefghijklmnopqrstuvwxyz more";
        let redacted = redact_secrets(msg);
        assert!(!redacted.contains("sk-abcdefghij"));
        assert!(redacted.contains("[REDACTED]"));

        let short_cookie = redact_secrets("cookie=s3cr3t");
        assert_eq!(short_cookie, "[REDACTED]");
        let query = redact_secrets("https://example.test/path?access_token=plain-text-value");
        assert!(!query.contains("plain-text-value"));
    }

    #[test]
    fn bearer_header_is_built_from_credential() {
        let cred = ResolvedCredential::new(CredentialKind::ApiKey, "abc123");
        let headers = bearer_headers(&cred);
        assert_eq!(
            headers,
            vec![("Authorization".to_string(), "Bearer abc123".to_string())]
        );
    }

    #[tokio::test]
    async fn sleep_returns_cancelled_when_token_fires_first() {
        let token = CancellationToken::new();
        token.cancel();
        let outcome = sleep_or_cancel(std::time::Duration::from_secs(60), &token).await;
        assert!(matches!(outcome, Err(QuotaError::Cancelled)));
    }

    #[test]
    fn routes_timeout_network_and_5xx_through_transient() {
        let timeout = ProviderError::new(ProviderErrorKind::Timeout, "connect timed out");
        let mapped = provider_error_to_quota_error(timeout);
        assert!(matches!(mapped, QuotaError::Timeout { .. }));

        let net = ProviderError::new(ProviderErrorKind::Network, "dns failure");
        let mapped = provider_error_to_quota_error(net);
        assert!(matches!(mapped, QuotaError::Transient { .. }));

        let server_err = ProviderError {
            http_status: Some(503),
            ..ProviderError::new(ProviderErrorKind::ProviderUnavailable, "upstream gone")
        };
        let mapped = provider_error_to_quota_error(server_err);
        assert!(matches!(
            mapped,
            QuotaError::Transient {
                status: Some(503),
                ..
            }
        ));
    }

    #[tokio::test]
    async fn text_error_never_copies_response_body() {
        let server = MockServer::start().await;
        let secret = "sk-body-secret-must-not-escape";
        Mock::given(method("GET"))
            .and(path("/console"))
            .respond_with(
                ResponseTemplate::new(401)
                    .set_body_string(format!("<html>echoed cookie={secret}</html>")),
            )
            .mount(&server)
            .await;
        let http = test_http();

        let error = api_get_text(
            &http,
            &format!("{}/console", server.uri()),
            &[],
            &CancellationToken::new(),
        )
        .await
        .expect_err("401");
        let rendered = error.to_string();
        assert!(matches!(error, QuotaError::Unauthorized { .. }));
        assert!(!rendered.contains(secret));
        assert!(!rendered.contains("echoed cookie"));
    }

    #[tokio::test]
    async fn json_error_body_and_query_never_leak_in_debug_or_display() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/json"))
            .respond_with(
                ResponseTemplate::new(403)
                    .set_body_string(r#"{"error":"cookie=s3cr3t token=abc123 signature=4f9a2b"}"#),
            )
            .mount(&server)
            .await;
        let http = test_http();
        let url = format!(
            "{}/json?token=abc123&cookie=s3cr3t&signature=4f9a2b",
            server.uri()
        );

        let error = api_get(&http, &url, &[], &CancellationToken::new())
            .await
            .expect_err("403");
        assert!(matches!(&error, QuotaError::Forbidden { .. }));
        assert_no_leaks(&error, &["abc123", "s3cr3t", "4f9a2b", "/json"]);
    }

    #[tokio::test]
    async fn text_error_body_and_query_never_leak_in_debug_or_display() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/console"))
            .respond_with(
                ResponseTemplate::new(401)
                    .set_body_string("<html>cookie=s3cr3t token=abc123 signature=4f9a2b</html>"),
            )
            .mount(&server)
            .await;
        let http = test_http();
        let url = format!(
            "{}/console?token=abc123&cookie=s3cr3t&signature=4f9a2b",
            server.uri()
        );

        let error = api_get_text(&http, &url, &[], &CancellationToken::new())
            .await
            .expect_err("401");
        assert!(matches!(&error, QuotaError::Unauthorized { .. }));
        assert_no_leaks(&error, &["abc123", "s3cr3t", "4f9a2b", "/console"]);
    }

    #[tokio::test]
    async fn json_rate_limit_keeps_retry_after() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/json"))
            .respond_with(
                ResponseTemplate::new(429)
                    .insert_header("Retry-After", "5")
                    .set_body_string(r#"{"error":"token=abc123"}"#),
            )
            .mount(&server)
            .await;
        let http = test_http();

        let error = api_get(
            &http,
            &format!("{}/json", server.uri()),
            &[],
            &CancellationToken::new(),
        )
        .await
        .expect_err("429");
        assert!(matches!(
            &error,
            QuotaError::RateLimited {
                retry_after_ms: Some(5_000),
                ..
            }
        ));
        assert_no_leaks(&error, &["abc123"]);
    }

    #[tokio::test]
    async fn text_rate_limit_keeps_retry_after() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/console"))
            .respond_with(
                ResponseTemplate::new(429)
                    .insert_header("Retry-After", "3")
                    .set_body_string("<html>cookie=s3cr3t</html>"),
            )
            .mount(&server)
            .await;
        let http = test_http();

        let error = api_get_text(
            &http,
            &format!("{}/console", server.uri()),
            &[],
            &CancellationToken::new(),
        )
        .await
        .expect_err("429");
        assert!(matches!(
            &error,
            QuotaError::RateLimited {
                retry_after_ms: Some(3_000),
                ..
            }
        ));
        assert_no_leaks(&error, &["s3cr3t"]);
    }

    #[tokio::test]
    async fn malformed_json_maps_to_safe_parse() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/json"))
            .respond_with(ResponseTemplate::new(200).set_body_string("not-json{token=abc123"))
            .mount(&server)
            .await;
        let http = test_http();
        let url = format!("{}/json?token=abc123&signature=4f9a2b", server.uri());

        let error = api_get(&http, &url, &[], &CancellationToken::new())
            .await
            .expect_err("malformed json");
        assert!(matches!(&error, QuotaError::Parse { .. }));
        assert_no_leaks(&error, &["abc123", "4f9a2b", "not-json", "/json"]);
    }

    #[tokio::test]
    async fn network_error_never_carries_url_or_query() {
        let http = test_http();
        let url = "http://127.0.0.1:1/quota?token=abc123&signature=4f9a2b";

        let error = api_get(&http, url, &[], &CancellationToken::new())
            .await
            .expect_err("connection refused");
        assert!(
            matches!(
                &error,
                QuotaError::Transient { .. } | QuotaError::Timeout { .. }
            ),
            "unexpected error kind: {error:?}"
        );
        assert_no_leaks(&error, &["127.0.0.1", "abc123", "4f9a2b", "/quota"]);
    }

    #[tokio::test]
    async fn json_body_read_can_be_cancelled_after_headers_arrive() {
        let StreamingResponse {
            url,
            mut body_started,
            release_body,
            server,
        } = start_streaming_response("application/json", r#"{"used":"#, "12}");
        let http = test_http();
        let cancel = CancellationToken::new();
        let request = api_get(&http, &url, &[], &cancel);
        tokio::pin!(request);

        tokio::select! {
            biased;
            result = &mut request => {
                panic!("JSON request completed before the streaming body was released: {result:?}")
            }
            ready = &mut body_started => ready.expect("server sent response headers and first body chunk"),
        }
        assert_body_read_is_pending(&mut request).await;

        cancel.cancel();
        let outcome = tokio::time::timeout(Duration::from_millis(500), &mut request).await;
        let _ = release_body.send(());
        server.join().expect("streaming JSON server");

        let outcome = outcome.expect("cancellation must interrupt JSON body reading immediately");
        assert!(matches!(outcome, Err(QuotaError::Cancelled)), "{outcome:?}");
    }

    #[tokio::test]
    async fn text_body_read_can_be_cancelled_after_headers_arrive() {
        let StreamingResponse {
            url,
            mut body_started,
            release_body,
            server,
        } = start_streaming_response("text/html", "<html><body>", "usage 42</body></html>");
        let http = test_http();
        let cancel = CancellationToken::new();
        let request = api_get_text(&http, &url, &[], &cancel);
        tokio::pin!(request);

        tokio::select! {
            biased;
            result = &mut request => {
                panic!("text request completed before the streaming body was released: {result:?}")
            }
            ready = &mut body_started => ready.expect("server sent response headers and first body chunk"),
        }
        assert_body_read_is_pending(&mut request).await;

        cancel.cancel();
        let outcome = tokio::time::timeout(Duration::from_millis(500), &mut request).await;
        let _ = release_body.send(());
        server.join().expect("streaming text server");

        let outcome = outcome.expect("cancellation must interrupt text body reading immediately");
        assert!(matches!(outcome, Err(QuotaError::Cancelled)), "{outcome:?}");
    }

    #[tokio::test]
    async fn json_success_returns_parsed_value() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/json"))
            .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"ok":true,"used":12}"#))
            .mount(&server)
            .await;
        let http = test_http();

        let value = api_get(
            &http,
            &format!("{}/json", server.uri()),
            &[],
            &CancellationToken::new(),
        )
        .await
        .expect("200 json");
        assert_eq!(value["ok"], serde_json::json!(true));
        assert_eq!(value["used"], serde_json::json!(12));
    }

    #[tokio::test]
    async fn text_success_returns_html() {
        let server = MockServer::start().await;
        let html = "<html><body>usage 42</body></html>";
        Mock::given(method("GET"))
            .and(path("/page"))
            .respond_with(ResponseTemplate::new(200).set_body_string(html))
            .mount(&server)
            .await;
        let http = test_http();

        let text = api_get_text(
            &http,
            &format!("{}/page", server.uri()),
            &[],
            &CancellationToken::new(),
        )
        .await
        .expect("200 html");
        assert_eq!(text, html);
    }

    async fn assert_body_read_is_pending<F, T>(future: &mut std::pin::Pin<&mut F>)
    where
        F: std::future::Future<Output = Result<T, QuotaError>>,
        T: std::fmt::Debug,
    {
        tokio::select! {
            biased;
            result = future => panic!("response body completed before its final chunk: {result:?}"),
            _ = tokio::time::sleep(Duration::from_millis(50)) => {}
        }
    }

    struct StreamingResponse {
        url: String,
        body_started: oneshot::Receiver<()>,
        release_body: mpsc::Sender<()>,
        server: thread::JoinHandle<()>,
    }

    /// 启动一个最小 HTTP/1.1 server：先发送完整 headers 与首段正文，再阻塞
    /// 最后一段正文。由此可证明客户端已经离开 `request.send()`，正等待 body。
    fn start_streaming_response(
        content_type: &'static str,
        first_body: &'static str,
        remaining_body: &'static str,
    ) -> StreamingResponse {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind streaming test server");
        let address = listener.local_addr().expect("streaming server address");
        let (body_started_tx, body_started) = oneshot::channel();
        let (release_body, release_body_rx) = mpsc::channel();

        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept streaming request");
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .expect("set request read timeout");

            let mut request = Vec::new();
            let mut buffer = [0_u8; 512];
            while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                let read = stream.read(&mut buffer).expect("read HTTP request");
                assert_ne!(read, 0, "client closed before sending request headers");
                request.extend_from_slice(&buffer[..read]);
            }

            let content_length = first_body.len() + remaining_body.len();
            let headers = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {content_length}\r\nConnection: close\r\n\r\n"
            );
            stream
                .write_all(headers.as_bytes())
                .and_then(|_| stream.write_all(first_body.as_bytes()))
                .and_then(|_| stream.flush())
                .expect("send headers and first body chunk");
            body_started_tx
                .send(())
                .expect("notify body-read test that headers arrived");

            let _ = release_body_rx.recv_timeout(Duration::from_secs(2));
            let _ = stream.write_all(remaining_body.as_bytes());
            let _ = stream.flush();
        });

        StreamingResponse {
            url: format!("http://{address}/stream"),
            body_started,
            release_body,
            server,
        }
    }

    fn test_http() -> HttpClient {
        HttpClient::new(
            provider_runtime::http::HttpClientConfig::builder()
                .disable_system_proxy()
                .build(),
        )
        .expect("client")
    }

    /// 断言错误的 Debug 与 Display 均不包含任何给定片段。
    fn assert_no_leaks(error: &QuotaError, needles: &[&str]) {
        let debug = format!("{error:?}");
        let display = error.to_string();
        for needle in needles {
            assert!(!debug.contains(needle), "Debug leaked {needle:?}: {debug}");
            assert!(
                !display.contains(needle),
                "Display leaked {needle:?}: {display}"
            );
        }
    }
}
