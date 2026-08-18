//! `ProviderError.kind` → 可读错误。S0 只呈现，不重试、不换号、不打印 Secret。

use pawork_domain::{ProviderError, ProviderErrorKind};

pub fn format_provider_error(err: &ProviderError) -> String {
    match &err.kind {
        ProviderErrorKind::Authentication => {
            let code = err.http_status.unwrap_or(401);
            format!(
                "认证失败 ({code})。检查环境变量 PAWORK_API_KEY_<PROVIDER_ID> 是否有效。"
            )
        }
        ProviderErrorKind::RateLimited => {
            let code = err.http_status.unwrap_or(429);
            match err.retry_after_ms {
                Some(ms) => format!(
                    "请求过于频繁 ({code})。建议等待 {}s 后重试。",
                    ms.div_ceil(1000)
                ),
                None => format!("请求过于频繁 ({code})。请稍后重试。"),
            }
        }
        ProviderErrorKind::Timeout => "请求超时。检查网络或稍后重试。".to_string(),
        ProviderErrorKind::Network => {
            format!("无法连接。检查网络与配置中的 base_url。 ({})", err.message)
        }
        ProviderErrorKind::Cancelled => "已取消。".to_string(),
        other => match err.http_status {
            Some(status) => format!("{other:?} ({status}): {}", err.message),
            None => format!("{other:?}: {}", err.message),
        },
    }
}

#[cfg(test)]
mod tests {
    use pawork_domain::ProviderError;

    use super::*;

    #[test]
    fn formats_auth_rate_limit_timeout_network() {
        let auth = ProviderError {
            http_status: Some(401),
            ..ProviderError::new(ProviderErrorKind::Authentication, "HTTP 401: nope")
        };
        let text = format_provider_error(&auth);
        assert!(text.contains("认证失败 (401)"));
        assert!(text.contains("PAWORK_API_KEY_"));
        assert!(!text.contains("nope"));

        let limited = ProviderError {
            http_status: Some(429),
            retry_after_ms: Some(1500),
            ..ProviderError::new(ProviderErrorKind::RateLimited, "slow")
        };
        assert_eq!(
            format_provider_error(&limited),
            "请求过于频繁 (429)。建议等待 2s 后重试。"
        );

        assert_eq!(
            format_provider_error(&ProviderError::new(
                ProviderErrorKind::Timeout,
                "timed out"
            )),
            "请求超时。检查网络或稍后重试。"
        );

        let network = format_provider_error(&ProviderError::new(
            ProviderErrorKind::Network,
            "error sending request for url (https://example.test/v1)",
        ));
        assert!(network.contains("无法连接"));
        assert!(network.contains("base_url"));
        assert!(network.contains("example.test"));
    }
}
