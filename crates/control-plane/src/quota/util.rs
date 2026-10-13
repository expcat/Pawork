//! 额度读链共用的时钟与 endpoint 清洗。
//!
//! `now_millis` 供 [`crate::quota::service::SystemQuotaClock`] 取墙钟；
//! `canonical_endpoint` 供 [`crate::quota::domain::QuotaProvenance`] 去掉
//! query / fragment。时钟不在此读墙钟以外的日历。

use std::time::{SystemTime, UNIX_EPOCH};

use pawork_domain::Timestamp;

/// 当前 Unix 毫秒时间戳。适配器统一用此口径，避免引入 `chrono`。
pub fn now_millis() -> Timestamp {
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or_default();
    Timestamp::from_unix_millis(ms)
}

/// 将原始 endpoint 清洗为 canonical 形式：
///
/// - 去除首尾空白；
/// - 截断首个 `?`（query）与 `#`（fragment）之前的部分；
/// - 结果为空白则返回 `None`，异常输入不会把 query/fragment 带入结果。
pub fn canonical_endpoint(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    let cut = trimmed
        .char_indices()
        .find(|(_, ch)| *ch == '?' || *ch == '#')
        .map_or(trimmed.len(), |(index, _)| index);
    let cleaned = trimmed[..cut].trim();
    if cleaned.is_empty() {
        None
    } else {
        Some(cleaned.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_endpoint_strips_query_and_fragment() {
        assert_eq!(
            canonical_endpoint("https://api.example.com/v1/usage?api_key=sk-secret&page=2#frag")
                .as_deref(),
            Some("https://api.example.com/v1/usage")
        );
        assert_eq!(
            canonical_endpoint("https://x/y?token=abc?more").as_deref(),
            Some("https://x/y")
        );
        assert_eq!(
            canonical_endpoint("https://x/y#frag?token=secret").as_deref(),
            Some("https://x/y")
        );
    }

    #[test]
    fn canonical_endpoint_rejects_abnormal_input_without_leak() {
        assert_eq!(canonical_endpoint(""), None);
        assert_eq!(canonical_endpoint("   "), None);
        assert_eq!(canonical_endpoint("#fragment-only"), None);
        assert_eq!(canonical_endpoint("?token=secret"), None);
        assert_eq!(
            canonical_endpoint("  https://x/y?token=secret  ").as_deref(),
            Some("https://x/y")
        );
    }
}
