//! 首发渠道错误细化表。
//!
//! HTTP 状态已由 `pawork-net` provider-neutral 地归一；这里只处理远端正文中
//! 无法从状态码判断的稳定错误标记。表内只登记本期六个渠道，不预埋后续厂商。

use pawork_api::{ProviderError, ProviderErrorKind};

#[derive(Clone, Debug)]
pub struct VendorErrorRule {
    pub vendor: &'static str,
    pub needles: &'static [&'static str],
    pub kind: ProviderErrorKind,
    pub retryable: bool,
    pub detail: &'static str,
    pub diagnostic_key: &'static str,
}

pub const VENDOR_ERROR_RULES: &[VendorErrorRule] = &[
    VendorErrorRule {
        vendor: "chatgpt",
        needles: &["usage", "limit"],
        kind: ProviderErrorKind::QuotaExceeded,
        retryable: false,
        detail: "ChatGPT subscription usage limit reached",
        diagnostic_key: "chatgpt_error",
    },
    VendorErrorRule {
        vendor: "chatgpt",
        needles: &["account", "deactivated"],
        kind: ProviderErrorKind::Authorization,
        retryable: false,
        detail: "ChatGPT account is unavailable",
        diagnostic_key: "chatgpt_error",
    },
    VendorErrorRule {
        vendor: "xai",
        needles: &["live_search", "quota"],
        kind: ProviderErrorKind::RateLimited,
        retryable: true,
        detail: "xAI live search quota exceeded",
        diagnostic_key: "xai_error",
    },
    VendorErrorRule {
        vendor: "xai",
        needles: &["collection", "not_ready"],
        kind: ProviderErrorKind::ProviderUnavailable,
        retryable: true,
        detail: "xAI collection is not ready",
        diagnostic_key: "xai_error",
    },
    VendorErrorRule {
        vendor: "xai",
        needles: &["insufficient_quota"],
        kind: ProviderErrorKind::QuotaExceeded,
        retryable: false,
        detail: "xAI quota is insufficient",
        diagnostic_key: "xai_error",
    },
    VendorErrorRule {
        vendor: "qwen-token-plan",
        needles: &["datainspectionfailed"],
        kind: ProviderErrorKind::ContentFiltered,
        retryable: false,
        detail: "Qwen data inspection rejected the request",
        diagnostic_key: "qwen_token_plan_error",
    },
    VendorErrorRule {
        vendor: "qwen-token-plan",
        needles: &["data_inspection_failed"],
        kind: ProviderErrorKind::ContentFiltered,
        retryable: false,
        detail: "Qwen data inspection rejected the request",
        diagnostic_key: "qwen_token_plan_error",
    },
    VendorErrorRule {
        vendor: "qwen-token-plan",
        needles: &["throttling"],
        kind: ProviderErrorKind::RateLimited,
        retryable: true,
        detail: "Qwen Token Plan is throttled",
        diagnostic_key: "qwen_token_plan_error",
    },
    VendorErrorRule {
        vendor: "qwen-token-plan",
        needles: &["quota_exhausted"],
        kind: ProviderErrorKind::QuotaExceeded,
        retryable: false,
        detail: "Qwen Token Plan quota is exhausted",
        diagnostic_key: "qwen_token_plan_error",
    },
    VendorErrorRule {
        vendor: "glm-coding",
        needles: &["1113"],
        kind: ProviderErrorKind::QuotaExceeded,
        retryable: false,
        detail: "GLM Coding Plan quota is unavailable",
        diagnostic_key: "glm_coding_error",
    },
    VendorErrorRule {
        vendor: "glm-coding",
        needles: &["1301"],
        kind: ProviderErrorKind::ContentFiltered,
        retryable: false,
        detail: "GLM content moderation rejected the request",
        diagnostic_key: "glm_coding_error",
    },
    VendorErrorRule {
        vendor: "glm-coding",
        needles: &["敏感"],
        kind: ProviderErrorKind::ContentFiltered,
        retryable: false,
        detail: "GLM content moderation rejected the request",
        diagnostic_key: "glm_coding_error",
    },
];

/// 按 adapter id 过滤规则并细化错误；未命中或无专属规则时原样返回。
pub fn normalize_vendor_error(vendor: &str, mut error: ProviderError) -> ProviderError {
    let message = error.message.to_ascii_lowercase();
    for rule in VENDOR_ERROR_RULES.iter().filter(|rule| rule.vendor == vendor) {
        if rule.needles.iter().all(|needle| message.contains(needle)) {
            error.kind = rule.kind.clone();
            error.retryable = rule.retryable;
            error
                .diagnostics
                .insert(rule.diagnostic_key.into(), rule.detail.into());
            return error;
        }
    }
    error
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initial_channel_rules_are_scoped_and_applied() {
        let glm = normalize_vendor_error(
            "glm-coding",
            ProviderError::new(ProviderErrorKind::InvalidRequest, "HTTP 400: 1113"),
        );
        assert_eq!(glm.kind, ProviderErrorKind::QuotaExceeded);
        assert!(!glm.retryable);

        let qwen = normalize_vendor_error(
            "qwen-token-plan",
            ProviderError::new(ProviderErrorKind::InvalidRequest, "DataInspectionFailed"),
        );
        assert_eq!(qwen.kind, ProviderErrorKind::ContentFiltered);
    }

    #[test]
    fn deferred_vendor_is_passthrough() {
        let error = normalize_vendor_error(
            "google",
            ProviderError::new(ProviderErrorKind::RateLimited, "RESOURCE_EXHAUSTED"),
        );
        assert_eq!(error.kind, ProviderErrorKind::RateLimited);
        assert!(error.diagnostics.is_empty());
    }
}
