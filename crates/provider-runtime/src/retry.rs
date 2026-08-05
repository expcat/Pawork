//! 重试与错误归一化（P2-10）。
//!
//! 把各 Provider 五花八门的 HTTP / 网络错误归一为 [`ProviderError`]，并判定是否
//! 可重试、建议的重试等待时间，以及退避策略。

use std::time::Duration;

use provider_api::{ProviderError, ProviderErrorKind};

/// 归一化 HTTP 响应状态码为 [`ProviderError`]。
///
/// `retry_after` 为响应头 `Retry-After` 的原始值（由调用方提取），`body_snippet`
/// 为截断后的脱敏响应正文片段（仅用于诊断消息，不应包含敏感数据）。
pub fn classify_status(
    status: reqwest::StatusCode,
    retry_after: Option<&str>,
    body_snippet: &str,
) -> ProviderError {
    let kind = match status.as_u16() {
        401 => ProviderErrorKind::Authentication,
        403 => ProviderErrorKind::Authorization,
        404 => ProviderErrorKind::ModelNotFound,
        408 => ProviderErrorKind::Timeout,
        413 => ProviderErrorKind::ContextTooLarge,
        429 => ProviderErrorKind::RateLimited,
        400 => ProviderErrorKind::InvalidRequest,
        451 => ProviderErrorKind::ContentFiltered,
        402 => ProviderErrorKind::QuotaExceeded,
        500 | 502 | 503 | 504 => ProviderErrorKind::ProviderUnavailable,
        _ if status.is_client_error() => ProviderErrorKind::InvalidRequest,
        _ if status.is_server_error() => ProviderErrorKind::ProviderUnavailable,
        _ => ProviderErrorKind::Unknown,
    };

    let mut error = ProviderError::new(kind, format!("HTTP {}: {}", status.as_u16(), body_snippet));
    error.http_status = Some(status.as_u16());

    // Retry-After 仅对可重试类有意义；解析失败时忽略（不 panic）。
    if error.retryable {
        if let Some(retry_after) = retry_after {
            if let Some(duration) = parse_retry_after(retry_after) {
                error.retry_after_ms = Some(duration.as_millis() as u64);
            }
        }
    }

    error
}

/// 归一化请求阶段（连接 / 发送 / 超时）的错误。
pub fn classify_request_error(error: reqwest::Error) -> ProviderError {
    if error.is_timeout() {
        ProviderError::new(ProviderErrorKind::Timeout, error.to_string())
    } else if error.is_connect() {
        ProviderError::new(ProviderErrorKind::Network, error.to_string())
    } else if error.is_body() || error.is_decode() {
        // 响应体读取 / 解码失败视作流中断
        ProviderError::new(ProviderErrorKind::StreamInterrupted, error.to_string())
    } else if error.is_request() {
        ProviderError::new(ProviderErrorKind::InvalidRequest, error.to_string())
    } else {
        ProviderError::new(ProviderErrorKind::Network, error.to_string())
    }
}

/// 解析 HTTP `Retry-After` 头。支持：
/// - 整数秒（`"120"`）；
/// - HTTP-date（RFC 7231，如 `"Wed, 21 Oct 2015 07:28:00 GMT"`）。
///
/// 解析失败返回 `None`，绝不 panic。
pub fn parse_retry_after(value: &str) -> Option<Duration> {
    let value = value.trim();

    // 整数秒
    if let Ok(seconds) = value.parse::<u64>() {
        return Some(Duration::from_secs(seconds));
    }

    // HTTP-date：解析常见 RFC 7231 IMF-fixdate 格式。
    parse_http_date(value)
}

/// 最小化的 HTTP-date 解析器：仅识别 `Wed, 21 Oct 2015 07:28:00 GMT` 这类
/// IMF-fixdate。相对当前时间计算等待时长（已过期为 0 等待，但仍返回 Some(0)）。
fn parse_http_date(value: &str) -> Option<Duration> {
    // 形如 "Day, DD Mon YYYY HH:MM:SS GMT"
    let value = value.trim();
    let mut parts = value.split_whitespace();
    let _weekday = parts.next()?; // "Wed,"
    let day: u32 = parts.next()?.trim_end_matches(',').parse().ok()?;
    let month_str = parts.next()?;
    let year: i64 = parts.next()?.parse().ok()?;
    let time = parts.next()?;
    let tz = parts.next()?;
    if tz != "GMT" {
        None
    } else {
        let month = month_index(month_str)?;
        let (h, m, s) = parse_hms(time)?;
        // 转为 Unix 时间戳（UTC）。用简单民用日期 → 天数算法，避免引入 chrono。
        let days = civil_to_days(year, month, day)?;
        let epoch_seconds = days * 86_400 + h * 3_600 + m * 60 + s;
        let now = current_unix_seconds();
        let delta = epoch_seconds - now;
        Some(if delta < 0 {
            Duration::ZERO
        } else {
            Duration::from_secs(delta as u64)
        })
    }
}

fn month_index(s: &str) -> Option<i64> {
    match s {
        "Jan" => Some(1),
        "Feb" => Some(2),
        "Mar" => Some(3),
        "Apr" => Some(4),
        "May" => Some(5),
        "Jun" => Some(6),
        "Jul" => Some(7),
        "Aug" => Some(8),
        "Sep" => Some(9),
        "Oct" => Some(10),
        "Nov" => Some(11),
        "Dec" => Some(12),
        _ => None,
    }
}

fn parse_hms(time: &str) -> Option<(i64, i64, i64)> {
    let mut it = time.split(':');
    let h: i64 = it.next()?.parse().ok()?;
    let m: i64 = it.next()?.parse().ok()?;
    let s: i64 = it.next()?.parse().ok()?;
    Some((h, m, s))
}

/// Howard Hinnant 的民用日期 → 自 epoch 起的天数算法（返回 UTC 天数）。
fn civil_to_days(y: i64, m: i64, d: u32) -> Option<i64> {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u64; // [0, 399]
    let doy = (153 * ((if m > 2 { m - 3 } else { m + 9 }) as u64) + 2) / 5 + (d as u64) - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    Some(era * 146_097 + doe as i64 - 719_468)
}

/// 获取当前 Unix 时间戳（秒）。优先用 std，跨平台一致。
fn current_unix_seconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// 指数退避 + 抖动策略。遵守 Retry-After：每次取 `max(退避, retry_after)`。
#[derive(Clone, Debug)]
pub struct ExponentialBackoff {
    cap: Duration,
    jitter: bool,
    current: Duration,
    // 简易线性同余生成器，保证可测试的确定性（种子固定）。
    rng_state: u64,
}

impl ExponentialBackoff {
    pub fn new(base: Duration, cap: Duration, jitter: bool) -> Self {
        Self {
            cap,
            jitter,
            current: base,
            rng_state: 0x2545_f491_4f6c_dd1d,
        }
    }

    /// 默认策略：基数 100ms，因子 2，上限 30s，带抖动。
    pub fn default_strategy() -> Self {
        Self::new(Duration::from_millis(100), Duration::from_secs(30), true)
    }

    /// 计算下一次等待时长（并推进内部状态），考虑可选的 Retry-After。
    pub fn next_delay(&mut self, retry_after: Option<Duration>) -> Option<Duration> {
        let backoff = if self.jitter {
            // 全抖动：在 [current, min(current*2, cap)) 区间取值，用确定性 LCG。
            self.rng_state = self
                .rng_state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let r = (self.rng_state >> 33) as f64 / (u32::MAX as f64);
            let lo = self.current.as_secs_f64();
            let hi = (self.current * 2).as_secs_f64().min(self.cap.as_secs_f64());
            Duration::from_secs_f64(lo + (hi - lo) * r)
        } else {
            self.current.min(self.cap)
        };

        let delay = match retry_after {
            Some(after) => backoff.max(after),
            None => backoff,
        };

        // 推进内部状态（指数 ×2，受上限约束）
        self.current = (self.current * 2).min(self.cap);

        Some(delay.min(self.cap))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_common_status_codes() {
        let auth = classify_status(reqwest::StatusCode::UNAUTHORIZED, None, "bad key");
        assert_eq!(auth.kind, ProviderErrorKind::Authentication);
        assert!(!auth.retryable);

        let rl = classify_status(
            reqwest::StatusCode::TOO_MANY_REQUESTS,
            Some("5"),
            "slow down",
        );
        assert_eq!(rl.kind, ProviderErrorKind::RateLimited);
        assert!(rl.retryable);
        assert_eq!(rl.retry_after_ms, Some(5_000));

        let overflow = classify_status(reqwest::StatusCode::PAYLOAD_TOO_LARGE, None, "too big");
        assert_eq!(overflow.kind, ProviderErrorKind::ContextTooLarge);
        assert!(!overflow.retryable);

        let server = classify_status(reqwest::StatusCode::BAD_GATEWAY, None, "upstream gone");
        assert_eq!(server.kind, ProviderErrorKind::ProviderUnavailable);
        assert!(server.retryable);
    }

    #[test]
    fn parse_retry_after_seconds_and_date() {
        assert_eq!(parse_retry_after("120"), Some(Duration::from_secs(120)));
        assert_eq!(parse_retry_after("  3  "), Some(Duration::from_secs(3)));

        // 过期的 HTTP-date 应解析为 0 等待（仍返回 Some）
        let past = parse_retry_after("Wed, 21 Oct 2015 07:28:00 GMT");
        assert_eq!(past, Some(Duration::ZERO));

        // 非法值
        assert_eq!(parse_retry_after("not-a-date"), None);
    }

    #[test]
    fn backoff_grows_and_respects_cap() {
        let mut backoff =
            ExponentialBackoff::new(Duration::from_millis(100), Duration::from_secs(1), false);
        let d1 = backoff.next_delay(None).unwrap();
        let d2 = backoff.next_delay(None).unwrap();
        let d3 = backoff.next_delay(None).unwrap();
        assert_eq!(d1, Duration::from_millis(100));
        assert_eq!(d2, Duration::from_millis(200));
        assert!(d3 <= Duration::from_secs(1)); // 受上限约束
    }

    #[test]
    fn backoff_respects_retry_after() {
        let mut backoff =
            ExponentialBackoff::new(Duration::from_millis(100), Duration::from_secs(30), false);
        let delay = backoff.next_delay(Some(Duration::from_secs(5))).unwrap();
        assert_eq!(delay, Duration::from_secs(5));
    }

    #[test]
    fn non_retryable_kinds_ignore_retry_after() {
        // 401 不可重试，即使带 Retry-After 也不采纳
        let err = classify_status(reqwest::StatusCode::UNAUTHORIZED, Some("10"), "no");
        assert_eq!(err.retry_after_ms, None);
    }
}
