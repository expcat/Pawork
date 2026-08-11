//! 跨 Provider / 适配器共享的纯函数：时间与脱敏（单一事实源）。
//!
//! 依据 P14 review §3.3 / §3.5：时间算法（Hinnant 日历换算、自然月 reset
//! 边界、Unix 毫秒时钟）与脱敏规则（endpoint 清洗、secret 掩码、来源标签
//! 脱敏、canonical 端点）只允许在这里存在一份实现，Provider / adapter /
//! domain 一律调用本模块，禁止复制。时钟相关的纯函数显式接收 `now`（不读
//! 墙钟），测试可固定边界时间。

use std::time::{SystemTime, UNIX_EPOCH};

use agent_domain::Timestamp;
use url::Url;

/// 当前 Unix 毫秒时间戳。适配器统一用此口径，避免引入 `chrono`。
pub fn now_millis() -> Timestamp {
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or_default();
    Timestamp::from_unix_millis(ms)
}

/// Unix 天数 → UTC 民用日期（Howard Hinnant 算法）。
pub fn epoch_to_utc_from_days(days: i64) -> (i32, u32, u32, u32, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = ((doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365) as i64;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe as u64 + yoe as u64 / 4 - yoe as u64 / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = (if mo <= 2 { y + 1 } else { y }) as i32;
    (y, mo, d, 0, 0, 0)
}

/// 民用日期 → Unix 天数（UTC）。
pub fn civil_to_days(y: i32, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { (y - 1) as i64 } else { y as i64 };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u64;
    let m_adj = if m > 2 { m as i64 - 3 } else { m as i64 + 9 };
    let doy = (153 * m_adj as u64 + 2) / 5 + d as u64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe as i64 - 719_468
}

/// Unix 秒 → UTC 民用时间（含时分秒；日期部分复用 [`epoch_to_utc_from_days`]）。
pub fn epoch_to_utc(secs: u64) -> (i32, u32, u32, u32, u32, u32) {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let h = (rem / 3_600) as u32;
    let mi = ((rem % 3_600) / 60) as u32;
    let s = (rem % 60) as u32;
    let (y, mo, d, _, _, _) = epoch_to_utc_from_days(days);
    (y, mo, d, h, mi, s)
}

/// 下月 1 号 00:00 UTC 的 Timestamp（自然月 reset 时刻）。显式接收 `now`。
pub fn next_month_start_timestamp(now: Timestamp) -> Timestamp {
    let secs = now.as_unix_millis() / 1_000;
    let days = (secs / 86_400) as i64;
    let (y, mo, _, _, _, _) = epoch_to_utc_from_days(days);
    // 下月：mo in 1..=12；mo==12 → 次年 1 月。
    let (ny, nmo) = if mo == 12 { (y + 1, 1) } else { (y, mo + 1) };
    let secs = civil_to_days(ny, nmo, 1) * 86_400;
    Timestamp::from_unix_millis(secs as u64 * 1_000)
}

/// 当月 UTC 起点的 Unix 秒（half-open 区间 [month_start, now)）。显式接收 `now`。
pub fn month_start_unix_seconds(now: Timestamp) -> u64 {
    let secs = now.as_unix_millis() / 1_000;
    let days = (secs / 86_400) as i64;
    let (y, mo, _, _, _, _) = epoch_to_utc_from_days(days);
    let month_start_day = civil_to_days(y, mo, 1);
    (month_start_day * 86_400) as u64
}

/// 把端点 URL 中的 query 与 fragment 抹掉，仅保留 scheme://host/path。
///
/// provenance 中的 endpoint 不得包含 query string（可能携带凭证或会话标识）。
pub fn redact_endpoint(raw: &str) -> String {
    match Url::parse(raw) {
        Ok(url) => {
            let mut stripped = String::new();
            stripped.push_str(url.scheme());
            stripped.push_str("://");
            if let Some(host) = url.host_str() {
                stripped.push_str(host);
            }
            if let Some(port) = url.port() {
                stripped.push(':');
                stripped.push_str(&port.to_string());
            }
            if !url.path().is_empty() && url.path() != "/" {
                stripped.push_str(url.path());
            }
            stripped
        }
        Err(_) => redact_secrets(raw),
    }
}

/// 抹去疑似 secret 的子串，并截断到安全长度。仅用于错误 / 审计文本。
///
/// 该函数是“尽力而为”的最后一道防线：真正的防线是 secret 永不进入这些文本。
pub fn redact_secrets(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for chunk in value.split_whitespace() {
        out.push_str(&mask_token_like(chunk));
        out.push(' ');
    }
    let out = out.trim_end().to_string();
    truncate_chars(&out, 512)
}

const REDACT_SECRET_LABELS: [&str; 7] = [
    "token",
    "secret",
    "authorization",
    "password",
    "cookie",
    "access_key",
    "session",
];

fn has_redact_secret_label(value: &str) -> bool {
    REDACT_SECRET_LABELS
        .iter()
        .any(|label| value.contains(label))
}

fn mask_token_like(chunk: &str) -> String {
    // Error strings should carry categories, not credential-shaped values.
    // Be deliberately conservative here: false-positive redaction is safer
    // than exposing a short cookie or token returned by a remote endpoint.
    let lower = chunk.to_ascii_lowercase();
    let sensitive_label = has_redact_secret_label(&lower);
    let common_prefix = lower.contains("sk-")
        || lower.contains("sk_")
        || lower.contains("bearer")
        || lower.contains("x-api-key");
    let high_entropy = chunk.len() >= 40
        && chunk
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-._~+/=".contains(&byte));
    let looks_secret = sensitive_label || common_prefix || high_entropy;
    if looks_secret {
        "[REDACTED]".to_string()
    } else {
        chunk.to_string()
    }
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }
    let end = value
        .char_indices()
        .nth(max_chars)
        .map(|(i, _)| i)
        .unwrap_or(value.len());
    format!("{}…", &value[..end])
}

/// 来源标签安全截断上限（字符）。与 refresh 的 `REDACTED_SOURCE_MAX_LEN`
/// 同一量级：异常超长的 provider 来源不得撑爆告警 / 审计条目。
const REDACT_SOURCE_MAX_CHARS: usize = 128;

/// 将来源标签清洗为可安全写入告警 / 审计的形态：
///
/// - 截断首个 `?`（query）与 `#`（fragment）之前的部分——签名 URL 的
///   token / sig 通常携带于此，整体抹掉；
/// - 屏蔽 `Bearer` 及其后的凭证 token，屏蔽 `sk-` / `sk_` 前缀 token 形态
///   子串（从标记到空白处整体掩掉）；
/// - 屏蔽 `token` / `secret` / `sig` / `cookie` / `key` 等敏感键的
///   `key=value`、`key:value` 与 JSON `"key":"value"` 值；
/// - 按字符安全截断（不切坏 UTF-8，超长以 `…` 标记）。
///
/// 与 [`redact_secrets`] 相同，这是“尽力而为”的最后一道防线：真正的防线是
/// secret 永不进入来源标签。
pub(crate) fn redact_source(raw: &str) -> String {
    let trimmed = raw.trim();
    let cut = trimmed
        .char_indices()
        .find(|(_, ch)| *ch == '?' || *ch == '#')
        .map_or(trimmed.len(), |(index, _)| index);
    let cleaned = trimmed[..cut].trim();
    if cleaned.is_empty() {
        return String::new();
    }

    // 先在完整字符串上处理键值对，避免 `token: abc` 或带空白的 JSON 被
    // `split_whitespace` 拆开后只掩到键、漏掉下一块中的值。
    let masked_pairs = mask_sensitive_pairs(cleaned);
    let mut out = String::with_capacity(masked_pairs.len());
    let mut chunks = masked_pairs.split_whitespace().peekable();
    while let Some(chunk) = chunks.next() {
        if is_bearer_label(chunk) {
            out.push_str("[REDACTED]");
            // Bearer 后紧跟的凭证 token 一并掩掉。
            if chunks.peek().is_some() {
                out.push_str(" [REDACTED]");
                chunks.next();
            }
        } else {
            out.push_str(&mask_source_chunk(chunk));
        }
        out.push(' ');
    }
    truncate_chars(out.trim_end(), REDACT_SOURCE_MAX_CHARS)
}

fn is_bearer_label(chunk: &str) -> bool {
    let lower = chunk.to_ascii_lowercase();
    lower == "bearer" || lower.ends_with(":bearer") || lower.ends_with("=bearer")
}

fn mask_source_chunk(chunk: &str) -> String {
    let lower = chunk.to_ascii_lowercase();
    // `sk-` / `sk_` 前缀 token 形态：从标记起整段掩掉（如
    // `https://api.x.com/sk-live-abc123/usage` → `https://api.x.com/[REDACTED]`）。
    if let Some(start) = lower.find("sk-").or_else(|| lower.find("sk_")) {
        return format!("{}{}", &chunk[..start], "[REDACTED]");
    }
    // `Bearer` 出现在 chunk 中（非独立 label 的变体）也整段掩掉。
    if lower.contains("bearer") {
        return "[REDACTED]".to_string();
    }
    // `token: value` / JSON pretty-print 会在这里按空白拆成 `token:` 与
    // `[REDACTED]` 两块；完整键值扫描已经掩掉后一块，保留安全的键名与分隔符。
    if let Some(separator) = chunk.len().checked_sub(1) {
        if matches!(chunk.as_bytes()[separator], b'=' | b':')
            && source_key_before(chunk, separator).is_some_and(is_sensitive_source_key)
        {
            return chunk.to_string();
        }
    }
    // 键值扫描已安全替换的 chunk 保留其结构；其余内容复用
    // `redact_secrets` 的 token/secret/high-entropy 判定作为兜底。
    if lower.contains("[redacted]") {
        chunk.to_string()
    } else {
        mask_token_like(chunk)
    }
}

fn mask_sensitive_pairs(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = String::with_capacity(value.len());
    let mut cursor = 0;
    let mut search = 0;
    let mut changed = false;

    while search < bytes.len() {
        let Some(relative) = bytes[search..]
            .iter()
            .position(|byte| matches!(byte, b'=' | b':'))
        else {
            break;
        };
        let separator = search + relative;
        if !source_key_before(value, separator).is_some_and(is_sensitive_source_key) {
            search = separator + 1;
            continue;
        }

        let mut value_start = separator + 1;
        while value_start < bytes.len() && bytes[value_start].is_ascii_whitespace() {
            value_start += 1;
        }

        let (content_start, content_end, next_search) = match bytes.get(value_start).copied() {
            Some(quote @ (b'"' | b'\'')) => {
                let content_start = value_start + 1;
                let content_end = find_closing_quote(bytes, content_start, quote);
                let next_search =
                    (content_end + usize::from(content_end < bytes.len())).min(bytes.len());
                (content_start, content_end, next_search)
            }
            _ => {
                let mut content_end = find_unquoted_value_end(bytes, value_start);
                // `Authorization: Bearer abc`：Bearer 与后续凭证属于同一个值，
                // 中间空白不能成为泄漏边界。
                if value[value_start..content_end].eq_ignore_ascii_case("bearer") {
                    let mut token_start = content_end;
                    while token_start < bytes.len() && bytes[token_start].is_ascii_whitespace() {
                        token_start += 1;
                    }
                    content_end = find_unquoted_value_end(bytes, token_start);
                }
                (value_start, content_end, content_end)
            }
        };

        out.push_str(&value[cursor..content_start]);
        out.push_str("[REDACTED]");
        cursor = content_end;
        search = next_search.max(separator + 1);
        changed = true;
    }

    if !changed {
        return value.to_string();
    }
    out.push_str(&value[cursor..]);
    out
}

fn source_key_before(value: &str, separator: usize) -> Option<&str> {
    let bytes = value.as_bytes();
    let mut end = separator;
    while end > 0 && bytes[end - 1].is_ascii_whitespace() {
        end -= 1;
    }
    if end == 0 {
        return None;
    }

    if matches!(bytes[end - 1], b'"' | b'\'') {
        let quote = bytes[end - 1];
        let key_end = end - 1;
        let key_start = bytes[..key_end]
            .iter()
            .rposition(|byte| *byte == quote)
            .map(|index| index + 1)?;
        return Some(&value[key_start..key_end]);
    }

    let mut start = end;
    while start > 0
        && (bytes[start - 1].is_ascii_alphanumeric() || matches!(bytes[start - 1], b'_' | b'-'))
    {
        start -= 1;
    }
    (start < end).then_some(&value[start..end])
}

fn is_sensitive_source_key(key: &str) -> bool {
    let lower = key.to_ascii_lowercase();
    has_redact_secret_label(&lower) || lower.contains("sig") || lower.contains("key")
}

fn find_closing_quote(bytes: &[u8], start: usize, quote: u8) -> usize {
    let mut escaped = false;
    for (index, byte) in bytes.iter().copied().enumerate().skip(start) {
        if escaped {
            escaped = false;
        } else if byte == b'\\' {
            escaped = true;
        } else if byte == quote {
            return index;
        }
    }
    bytes.len()
}

fn find_unquoted_value_end(bytes: &[u8], start: usize) -> usize {
    bytes[start..]
        .iter()
        .position(|byte| {
            byte.is_ascii_whitespace() || matches!(byte, b',' | b';' | b'&' | b'}' | b']')
        })
        .map_or(bytes.len(), |relative| start + relative)
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

    fn ts(unix_secs: u64) -> Timestamp {
        Timestamp::from_unix_millis(unix_secs * 1_000)
    }

    #[test]
    fn now_millis_is_recent() {
        let now = now_millis().as_unix_millis();
        let wall = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        assert!(now <= wall && wall - now < 60_000, "clock drift too large");
    }

    #[test]
    fn calendar_conversions_round_trip_known_dates() {
        for (y, m, d) in [
            (2024, 2, 29),  // leap day
            (2025, 12, 31), // year boundary
            (2026, 1, 1),
            (2026, 3, 15),
            (2026, 12, 31),
        ] {
            let days = civil_to_days(y, m, d);
            assert_eq!(
                epoch_to_utc_from_days(days),
                (y, m, d, 0, 0, 0),
                "round trip failed for {y}-{m:02}-{d:02}"
            );
        }
    }

    #[test]
    fn epoch_to_utc_known_timestamp() {
        // 2026-01-01T00:00:00Z = 1767225600。
        assert_eq!(epoch_to_utc(1_767_225_600), (2026, 1, 1, 0, 0, 0));
        // 12:34:56 偏移。
        assert_eq!(
            epoch_to_utc(1_767_225_600 + 12 * 3_600 + 34 * 60 + 56),
            (2026, 1, 1, 12, 34, 56)
        );
    }

    #[test]
    fn next_month_start_lands_on_first_of_next_month_utc() {
        // 2026-03-15T12:34:56Z → 2026-04-01T00:00:00Z。
        let now = ts(civil_to_days(2026, 3, 15) as u64 * 86_400 + 12 * 3_600 + 34 * 60 + 56);
        let expected = civil_to_days(2026, 4, 1) as u64 * 86_400;
        assert_eq!(next_month_start_timestamp(now), ts(expected));

        // 跨年：2026-12-15 → 2027-01-01。
        let now = ts(civil_to_days(2026, 12, 15) as u64 * 86_400);
        let expected = civil_to_days(2027, 1, 1) as u64 * 86_400;
        assert_eq!(next_month_start_timestamp(now), ts(expected));

        // 月初边界：恰好 1 号 00:00:00 UTC 仍是当月起点，reset 指向次月。
        let now = ts(civil_to_days(2026, 3, 1) as u64 * 86_400);
        let expected = civil_to_days(2026, 4, 1) as u64 * 86_400;
        assert_eq!(next_month_start_timestamp(now), ts(expected));
    }

    #[test]
    fn month_start_is_first_day_of_current_month() {
        let noon = civil_to_days(2026, 3, 15) as u64 * 86_400 + 12 * 3_600;
        assert_eq!(
            month_start_unix_seconds(ts(noon)),
            civil_to_days(2026, 3, 1) as u64 * 86_400
        );
        // 1 号 00:00:00 UTC：month_start == now。
        let first = civil_to_days(2026, 3, 1) as u64 * 86_400;
        assert_eq!(month_start_unix_seconds(ts(first)), first);
    }

    #[test]
    fn redact_endpoint_strips_query_and_fragment_keeps_port_and_path() {
        assert_eq!(
            redact_endpoint("https://api.example.com/v1/usage?api_key=sk-secret&page=2#frag"),
            "https://api.example.com/v1/usage"
        );
        assert_eq!(
            redact_endpoint("https://console.example.com:8443/quota#overview"),
            "https://console.example.com:8443/quota"
        );
        // 无 query 的干净 URL 原样保留。
        let clean = "http://127.0.0.1:51827";
        assert_eq!(redact_endpoint(clean), clean);
    }

    #[test]
    fn redact_endpoint_falls_back_to_secret_masking_on_invalid_input() {
        // 非法 URL（非 URL 形态）仍不得泄漏 token 形态子串。
        let redacted = redact_endpoint("sk-live-abcdefghijklmnopqrstuvwxyz");
        assert!(!redacted.contains("sk-live-abcdefghijkl"));
        assert!(redacted.contains("[REDACTED]"));
    }

    #[test]
    fn redact_secrets_masks_token_like_chunks() {
        let msg = "key=sk-abcdefghijklmnopqrstuvwxyz more";
        let redacted = redact_secrets(msg);
        assert!(!redacted.contains("sk-abcdefghij"));
        assert!(redacted.contains("[REDACTED]"));

        assert_eq!(redact_secrets("cookie=s3cr3t"), "[REDACTED]");
        let query = redact_secrets("https://example.test/path?access_token=plain-text-value");
        assert!(!query.contains("plain-text-value"));
    }

    #[test]
    fn redact_secrets_truncates_to_safe_length() {
        // 用普通词（非高熵 token 形态）构造超长文本，验证截断而非掩码。
        let long = "hello ".repeat(200);
        let redacted = redact_secrets(&long);
        assert!(redacted.chars().count() <= 513);
        assert!(redacted.ends_with('…'));
    }

    #[test]
    fn redact_source_strips_query_and_fragment() {
        assert_eq!(
            redact_source("api.x.com/v1/quota?token=abc&sig=xyz"),
            "api.x.com/v1/quota"
        );
        assert_eq!(redact_source("api.x.com/v1#frag"), "api.x.com/v1");
        // 首字符即标记：query / fragment 整体抹掉后为空。
        assert_eq!(redact_source("?token=abc"), "");
    }

    #[test]
    fn redact_source_masks_sk_and_bearer_tokens() {
        // `sk-` 标记后的整段（含后续路径）掩掉，前缀保留。
        assert_eq!(
            redact_source("https://api.x.com/sk-live-abc123/usage"),
            "https://api.x.com/[REDACTED]"
        );
        assert_eq!(redact_source("sk_abc123"), "[REDACTED]");
        // Bearer 与其后 token 一并掩掉。
        assert_eq!(
            redact_source("Bearer sk-live-abcdefgh123"),
            "[REDACTED] [REDACTED]"
        );
        // 明文源原样保留。
        assert_eq!(
            redact_source("api.anthropic.com/v1/usage"),
            "api.anthropic.com/v1/usage"
        );
    }

    #[test]
    fn redact_source_masks_sensitive_key_values() {
        assert_eq!(
            redact_source("header key=abc123 rest"),
            "header key=[REDACTED] rest"
        );
        assert_eq!(
            redact_source("token=plain-text-value cookie=s3cr3t"),
            "token=[REDACTED] cookie=[REDACTED]"
        );
        // 非敏感键保留原值。
        assert_eq!(redact_source("page=2 mode=fast"), "page=2 mode=fast");
    }

    #[test]
    fn redact_source_masks_colon_sensitive_key_values() {
        assert_eq!(
            redact_source("token:abc sig: xyz cookie:session-value"),
            "token:[REDACTED] sig: [REDACTED] cookie:[REDACTED]"
        );
        assert_eq!(
            redact_source("Authorization: Bearer short-token rest"),
            "Authorization: [REDACTED] rest"
        );
    }

    #[test]
    fn redact_source_masks_json_sensitive_key_values() {
        assert_eq!(
            redact_source(r#"{"token":"abc","cookie":"session-value","mode":"safe"}"#),
            r#"{"token":"[REDACTED]","cookie":"[REDACTED]","mode":"safe"}"#
        );
        assert_eq!(
            redact_source(r#"{"api_key" : "abc", "sig": "xyz"}"#),
            r#"{"api_key" : "[REDACTED]", "sig": "[REDACTED]"}"#
        );
    }

    #[test]
    fn redact_source_truncates_on_char_boundary() {
        // 多字节字符开头 + 超长 ASCII：截断不得切坏 UTF-8。
        let long = format!("源{}", "x".repeat(500));
        let redacted = redact_source(&long);
        assert!(redacted.chars().count() <= REDACT_SOURCE_MAX_CHARS + 1);
        assert!(redacted.ends_with('…'));
        assert!(std::str::from_utf8(redacted.as_bytes()).is_ok());
    }

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
