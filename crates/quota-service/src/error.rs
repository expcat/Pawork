//! 配额读数/解析/访问错误。

use thiserror::Error;

/// 配额查询过程中可能发生的错误。
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum QuotaError {
    /// 适配器/Provider 不支持对该作用域、窗口或单位的配额查询。
    #[error("quota query unsupported: {detail}")]
    Unsupported { detail: String },

    /// 401：未授权（凭证缺失或无效）。
    #[error("quota query unauthorized (401): {detail}")]
    Unauthorized { detail: String },

    /// 403：禁止访问该配额资源。
    #[error("quota query forbidden (403): {detail}")]
    Forbidden { detail: String },

    /// 429：配额查询接口自身被限流。
    #[error("quota query rate limited (429): {detail}")]
    RateLimited {
        detail: String,
        retry_after_ms: Option<u64>,
    },

    /// 需要重新授权（如 OAuth token 过期，需上层刷新后重试）。
    #[error("quota query requires reauthorization: {detail}")]
    ReauthorizationRequired { detail: String },

    /// 请求超时（连接建立 / 读取 / 整体超时）。通常是瞬时状态，可安全重试。
    ///
    /// `detail` 必须是已脱敏的安全字符串（不得含明文 token / key），
    /// `status` 与 `retry_after_ms` 为可选的服务器信息。
    #[error("quota query timed out: {detail}")]
    Timeout {
        detail: String,
        status: Option<u16>,
        retry_after_ms: Option<u64>,
    },

    /// 临时性故障（如 5xx、网关错误、瞬时网络抖动）。可安全重试。
    ///
    /// `detail` 必须是已脱敏的安全字符串（不得含明文 token / key），
    /// `status` 与 `retry_after_ms` 为可选的服务器信息。
    #[error("quota query transient failure: {detail}")]
    Transient {
        detail: String,
        status: Option<u16>,
        retry_after_ms: Option<u64>,
    },

    /// 响应解析失败。
    #[error("quota response parse failed: {detail}")]
    Parse { detail: String },

    /// 调用被取消。
    #[error("quota query cancelled")]
    Cancelled,

    /// 网络/IO 或其他未分类的底层错误。
    #[error("quota query failed: {detail}")]
    Other { detail: String },
}

impl QuotaError {
    pub fn unsupported(detail: impl Into<String>) -> Self {
        Self::Unsupported {
            detail: detail.into(),
        }
    }

    pub fn unauthorized(detail: impl Into<String>) -> Self {
        Self::Unauthorized {
            detail: detail.into(),
        }
    }

    pub fn forbidden(detail: impl Into<String>) -> Self {
        Self::Forbidden {
            detail: detail.into(),
        }
    }

    pub fn rate_limited(detail: impl Into<String>, retry_after_ms: Option<u64>) -> Self {
        Self::RateLimited {
            detail: detail.into(),
            retry_after_ms,
        }
    }

    pub fn reauthorization_required(detail: impl Into<String>) -> Self {
        Self::ReauthorizationRequired {
            detail: detail.into(),
        }
    }

    pub fn timeout(detail: impl Into<String>) -> Self {
        Self::Timeout {
            detail: detail.into(),
            status: None,
            retry_after_ms: None,
        }
    }

    pub fn transient(
        detail: impl Into<String>,
        status: Option<u16>,
        retry_after_ms: Option<u64>,
    ) -> Self {
        Self::Transient {
            detail: detail.into(),
            status,
            retry_after_ms,
        }
    }

    pub fn parse(detail: impl Into<String>) -> Self {
        Self::Parse {
            detail: detail.into(),
        }
    }

    pub fn other(detail: impl Into<String>) -> Self {
        Self::Other {
            detail: detail.into(),
        }
    }

    /// 该错误是否可安全重试。
    ///
    /// 仅限明确的瞬时类错误（超时 / 临时故障 / 接口限流）返回 `true`；
    /// 鉴权、解析、取消、不支持与未分类错误保守返回 `false`，避免无意义重试。
    pub fn retryable(&self) -> bool {
        matches!(
            self,
            Self::Timeout { .. } | Self::Transient { .. } | Self::RateLimited { .. }
        )
    }

    /// 服务端建议的重试等待时间（毫秒）；无建议时返回 `None`。
    pub fn retry_after_ms(&self) -> Option<u64> {
        match self {
            Self::RateLimited { retry_after_ms, .. } | Self::Transient { retry_after_ms, .. } => {
                *retry_after_ms
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsupported_constructs_and_displays() {
        let err = QuotaError::unsupported("model-level not supported");
        assert!(matches!(err, QuotaError::Unsupported { .. }));
        assert_eq!(
            err.to_string(),
            "quota query unsupported: model-level not supported"
        );
    }

    #[test]
    fn rate_limited_carries_retry_hint() {
        let err = QuotaError::rate_limited("slow down", Some(5_000));
        assert!(matches!(
            err,
            QuotaError::RateLimited {
                retry_after_ms: Some(5_000),
                ..
            }
        ));
    }

    #[test]
    fn error_variants_are_distinct() {
        assert_ne!(QuotaError::unsupported("x"), QuotaError::forbidden("x"));
    }

    #[test]
    fn timeout_constructs_and_displays_safely() {
        let err = QuotaError::timeout("connect timed out");
        assert!(matches!(err, QuotaError::Timeout { .. }));
        assert_eq!(err.to_string(), "quota query timed out: connect timed out");
        assert!(err.retryable());
        assert_eq!(err.retry_after_ms(), None);
    }

    #[test]
    fn transient_carries_optional_status_and_retry_after() {
        let err = QuotaError::transient("upstream gateway error", Some(503), Some(3_000));
        assert!(matches!(
            err,
            QuotaError::Transient {
                status: Some(503),
                retry_after_ms: Some(3_000),
                ..
            }
        ));
        assert!(err.retryable());
        assert_eq!(err.retry_after_ms(), Some(3_000));

        let bare = QuotaError::transient("flaky network", None, None);
        assert!(matches!(
            bare,
            QuotaError::Transient {
                status: None,
                retry_after_ms: None,
                ..
            }
        ));
        assert_eq!(bare.retry_after_ms(), None);
    }

    #[test]
    fn retryable_classification_is_explicit_per_variant() {
        let cases = [
            (QuotaError::unsupported("x"), false),
            (QuotaError::unauthorized("x"), false),
            (QuotaError::forbidden("x"), false),
            (QuotaError::rate_limited("x", None), true),
            (QuotaError::reauthorization_required("x"), false),
            (QuotaError::timeout("x"), true),
            (QuotaError::transient("x", None, None), true),
            (QuotaError::parse("x"), false),
            (QuotaError::Cancelled, false),
            (QuotaError::other("x"), false),
        ];
        for (err, expected) in cases {
            assert_eq!(err.retryable(), expected, "unexpected retryable for {err}");
        }
    }

    #[test]
    fn retry_after_ms_only_from_rate_limited_and_transient() {
        assert_eq!(
            QuotaError::rate_limited("x", Some(1_000)).retry_after_ms(),
            Some(1_000)
        );
        assert_eq!(
            QuotaError::transient("x", None, Some(2_000)).retry_after_ms(),
            Some(2_000)
        );
        assert_eq!(QuotaError::timeout("x").retry_after_ms(), None);
        assert_eq!(QuotaError::other("x").retry_after_ms(), None);
        assert_eq!(QuotaError::Cancelled.retry_after_ms(), None);
    }
}
