//! pawork-auth 的错误类型。
//!
//! 所有错误均为 `Send + Sync`，且**不携带任何明文 secret**：keyring 返回的
//! 原始错误统一归一为 `Storage(String)`，仅保留可读的归因描述。

use thiserror::Error;

/// 认证 / Secret 管理过程中可能出现的错误。
///
/// 任意变体的 `Display` 输出都不应包含明文 token；构造错误时严禁把 secret
/// 拼进 message。
#[derive(Debug, Error)]
pub enum AuthError {
    /// Secret 存储后端（如文件后端）操作失败。
    #[error("secret storage error: {0}")]
    Storage(String),

    /// 指定 `(service, account)` 对应的条目不存在。
    #[error("credential not found")]
    NotFound,

    /// secret 本身非法（如为空、长度不足、格式不符）。
    #[error("invalid secret: {0}")]
    InvalidSecret(String),

    /// `StoredCredential` 元数据不完整或前后不一致。
    #[error("malformed credential metadata: {0}")]
    MalformedMetadata(String),

    // ---- OAuth（P6-4）相关错误。所有变体都不得携带明文 token。----
    /// 通用 OAuth 流程错误（仅含可读描述，绝不包含明文 token）。
    #[error("oauth error: {0}")]
    OAuth(String),

    /// OAuth token endpoint 返回的标准错误（`error` + 可选 `error_description`）。
    #[error("oauth token endpoint error: {error}")]
    TokenEndpoint {
        error: String,
        description: Option<String>,
    },

    /// Device Flow（RFC 8628）的 device_code 已过期或被拒绝。
    #[error("oauth device flow expired")]
    ExpiredToken,

    /// 一次性回调服务器在接收授权码时出错。
    #[error("oauth callback error: {0}")]
    Callback(String),

    /// HTTP（reqwest）请求失败。Display 只保留错误类别与 host 级来源。
    #[error("http error: {}", redact_http_error(.0))]
    Http(#[from] reqwest::Error),

    /// 回调服务器的底层 IO 错误。
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// URL 解析失败。
    #[error("url parse error: {0}")]
    Url(#[from] url::ParseError),
}

fn redact_http_error(error: &reqwest::Error) -> String {
    match error.url() {
        Some(url) => format!("{} from {}", http_error_kind(error), redact_url_origin(url)),
        None => http_error_kind(error).to_string(),
    }
}

fn http_error_kind(error: &reqwest::Error) -> &'static str {
    if error.is_timeout() {
        "timeout"
    } else if error.is_connect() {
        "connect"
    } else if error.is_body() {
        "body"
    } else if error.is_decode() {
        "decode"
    } else if error.is_request() {
        "request"
    } else if error.is_builder() {
        "builder"
    } else {
        "error"
    }
}

fn redact_url_origin(url: &reqwest::Url) -> String {
    let scheme = url.scheme();
    let host = url.host_str().unwrap_or("invalid-host");
    match url.port() {
        Some(port) => format!("{scheme}://{host}:{port}"),
        None => format!("{scheme}://{host}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_display_redacts_token_endpoint_url() {
        const SECRET: &str = "s3cret-oauth-pass";
        let raw = format!("https://alice:{SECRET}@auth.example/oauth/token?code=leak");
        let inner = reqwest::Client::builder()
            .user_agent("\0")
            .build()
            .expect_err("invalid user-agent")
            .with_url(raw.parse().expect("url"));
        assert!(
            inner.to_string().contains(SECRET) || inner.url().is_some(),
            "precondition: reqwest Display/url carries the endpoint"
        );
        let err = AuthError::from(inner);
        let display = err.to_string();
        assert!(!display.contains(SECRET), "{display}");
        assert!(!display.contains("alice"), "{display}");
        assert!(!display.contains("/oauth/token"), "{display}");
        assert!(!display.contains("code=leak"), "{display}");
        assert!(display.contains("auth.example"), "{display}");
    }
}
