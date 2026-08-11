//! 精确货币转换：任何远端金额都无损换算成整数 micros（1/1_000_000），
//! 全程不使用浮点，避免 IEEE-754 的舍入误差污染配额读数。
//!
//! 严格契约：远端金额解析**绝不** clamp / saturate / 静默截断——负数、
//! 溢出、超精度一律返回 [`QuotaError::Parse`]，宁可失败也不伪造读数。
//!
//! 各 Provider 的口径：
//! - OpenAI / xAI：整数「美分」→ micros（× 10_000）。
//! - Moonshot / Qwen：十进制货币字符串（CNY）→ micros（小数部分补足 6 位）。
//! - Anthropic：字符串 amount（最小货币单位取决于 currency，通常整数）→ micros。

use crate::{QuotaError, QuotaMeasure};

/// 把整数「最小货币单位（cents）」换算为 micros。
///
/// 负数与溢出一律 `Parse`，绝不钳位或饱和。
pub fn cents_to_micros(cents: i64) -> Result<u64, QuotaError> {
    if cents < 0 {
        return Err(QuotaError::parse(format!("negative cents: {cents}")));
    }
    cents
        .checked_mul(10_000)
        .map(|v| v as u64)
        .ok_or_else(|| QuotaError::parse(format!("cents overflow: {cents}")))
}

/// 把十进制货币字符串精确换算为 micros（非负）。
///
/// 支持：可选 `+` 号、整数与小数部分、最多 6 位小数（小数位不足补 0）。
/// 负数、超过 6 位小数（超精度）、溢出、非法格式一律 `Parse`——**不截断、
/// 不钳位**。例如 `"12.3456"` → `12_345_600`。空串/非法返回 `Parse`。
pub fn decimal_string_to_micros(value: &str) -> Result<u64, QuotaError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(QuotaError::parse("empty monetary value"));
    }
    let rest = match trimmed.as_bytes() {
        [b'-', ..] => {
            return Err(QuotaError::parse(format!(
                "negative monetary value: {value}"
            )))
        }
        [b'+', rest @ ..] => std::str::from_utf8(rest).unwrap_or(""),
        _ => trimmed,
    };
    if rest.is_empty() {
        return Err(QuotaError::parse(format!(
            "invalid monetary value: {value}"
        )));
    }

    let mut split = rest.splitn(2, '.');
    let int_part = split.next().unwrap_or("");
    let frac_part = split.next().unwrap_or("");
    if int_part.is_empty() && frac_part.is_empty() {
        return Err(QuotaError::parse(format!(
            "invalid monetary value: {value}"
        )));
    }
    if !int_part.bytes().all(|b| b.is_ascii_digit())
        || !frac_part.bytes().all(|b| b.is_ascii_digit())
    {
        return Err(QuotaError::parse(format!(
            "invalid monetary value: {value}"
        )));
    }
    if frac_part.len() > 6 {
        // 1 micro = 1e-6 货币单位：更多小数位无法无损换算，必须报错而非截断。
        return Err(QuotaError::parse(format!(
            "monetary value exceeds 6 fractional digits: {value}"
        )));
    }

    // 小数部分补足 6 位。
    let mut frac6: [u8; 6] = [b'0'; 6];
    frac6[..frac_part.len()].copy_from_slice(frac_part.as_bytes());

    let int_value: u64 = if int_part.is_empty() {
        0
    } else {
        int_part
            .parse()
            .map_err(|_| QuotaError::parse(format!("integer overflow: {value}")))?
    };
    let frac_value: u64 = std::str::from_utf8(&frac6)
        .unwrap_or("0")
        .parse()
        .map_err(|_| QuotaError::parse(format!("fraction overflow: {value}")))?;

    int_value
        .checked_mul(1_000_000)
        .and_then(|v| v.checked_add(frac_value))
        .ok_or_else(|| QuotaError::parse(format!("monetary overflow: {value}")))
}

/// 从 JSON 值中取出十进制货币字符串，**不经过 f64**。
///
/// 只接受 JSON 字符串与 number（number 经 serde_json 最短表示序列化，
/// 不调用 `as_f64`，也不做任何浮点运算）；布尔、null、对象、数组一律
/// `Parse`——远端契约是 decimal string / number，浮点路径无法保证无损。
pub fn json_decimal_string(value: &serde_json::Value, what: &str) -> Result<String, QuotaError> {
    match value {
        serde_json::Value::String(s) => Ok(s.clone()),
        serde_json::Value::Number(n) => Ok(n.to_string()),
        _ => Err(QuotaError::parse(format!(
            "{what} must be a decimal string or number"
        ))),
    }
}

/// 把 micros 转为 [`QuotaMeasure::Exact`]（micros 本身即非负 u64，无需钳位）。
pub fn micros_measure(micros: u64) -> QuotaMeasure {
    QuotaMeasure::exact(micros)
}

/// 计算剩余额度 = limit - used。
///
/// 这是派生值（非远端读数），不做饱和：limit < used 时剩余为负，无法在
/// 非负 [`QuotaMeasure`] 中表示，诚实返回 `Unknown`（不伪造 0）。
/// 任一端 `Unknown`/`Infinite` 时返回 `Unknown`；`Infinite` 语义由调用方决定。
pub fn remaining(limit: QuotaMeasure, used: QuotaMeasure) -> QuotaMeasure {
    match (limit.exact_value(), used.exact_value()) {
        (Some(limit), Some(used)) => match limit.checked_sub(used) {
            Some(v) => QuotaMeasure::exact(v),
            None => QuotaMeasure::Unknown,
        },
        _ => QuotaMeasure::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cents_to_micros_is_exact() {
        assert_eq!(cents_to_micros(1).unwrap(), 10_000);
        assert_eq!(cents_to_micros(0).unwrap(), 0);
        assert_eq!(cents_to_micros(100).unwrap(), 1_000_000);
    }

    #[test]
    fn cents_to_micros_rejects_negative_and_overflow() {
        assert!(matches!(cents_to_micros(-1), Err(QuotaError::Parse { .. })));
        assert!(matches!(
            cents_to_micros(i64::MIN),
            Err(QuotaError::Parse { .. })
        ));
        // i64::MAX × 10_000 溢出 → Parse，不饱和。
        assert!(matches!(
            cents_to_micros(i64::MAX),
            Err(QuotaError::Parse { .. })
        ));
        // i64::MAX / 10_000 以内可精确表示。
        assert!(cents_to_micros(i64::MAX / 10_000).is_ok());
    }

    #[test]
    fn decimal_to_micros_handles_fraction_and_sign() {
        assert_eq!(decimal_string_to_micros("12.3456").unwrap(), 12_345_600);
        assert_eq!(decimal_string_to_micros("0.5").unwrap(), 500_000);
        assert_eq!(decimal_string_to_micros("100").unwrap(), 100_000_000);
        assert_eq!(decimal_string_to_micros(".25").unwrap(), 250_000);
        assert_eq!(decimal_string_to_micros("+1.5").unwrap(), 1_500_000);
        assert_eq!(decimal_string_to_micros("1.123456").unwrap(), 1_123_456);
        assert_eq!(decimal_string_to_micros("0.000001").unwrap(), 1);
    }

    #[test]
    fn decimal_to_micros_rejects_invalid() {
        assert!(decimal_string_to_micros("").is_err());
        assert!(decimal_string_to_micros("abc").is_err());
        assert!(decimal_string_to_micros("1.2.3").is_err());
        assert!(decimal_string_to_micros("-").is_err());
        assert!(decimal_string_to_micros("+").is_err());
    }

    #[test]
    fn decimal_to_micros_rejects_negative_without_clamping() {
        assert!(matches!(
            decimal_string_to_micros("-0.5"),
            Err(QuotaError::Parse { .. })
        ));
        assert!(matches!(
            decimal_string_to_micros("-100"),
            Err(QuotaError::Parse { .. })
        ));
    }

    #[test]
    fn decimal_to_micros_rejects_over_precision_without_truncation() {
        // 超过 6 位小数：报错而非截断。
        assert!(matches!(
            decimal_string_to_micros("1.1234567"),
            Err(QuotaError::Parse { .. })
        ));
        assert!(matches!(
            decimal_string_to_micros("0.0000001"),
            Err(QuotaError::Parse { .. })
        ));
    }

    #[test]
    fn decimal_to_micros_rejects_overflow() {
        assert!(matches!(
            decimal_string_to_micros("999999999999999999999999"),
            Err(QuotaError::Parse { .. })
        ));
        // u64::MAX micros = 18446744073709.551615；再多 1 micro 即溢出。
        assert!(matches!(
            decimal_string_to_micros("18446744073709.551616"),
            Err(QuotaError::Parse { .. })
        ));
        assert_eq!(
            decimal_string_to_micros("18446744073709.551615").unwrap(),
            u64::MAX
        );
    }

    #[test]
    fn json_decimal_string_accepts_string_and_number_without_f64() {
        assert_eq!(
            json_decimal_string(&serde_json::json!("123.45"), "x").unwrap(),
            "123.45"
        );
        assert_eq!(
            json_decimal_string(&serde_json::json!(123), "x").unwrap(),
            "123"
        );
        assert_eq!(
            json_decimal_string(&serde_json::json!(123.45), "x").unwrap(),
            "123.45"
        );
        for bad in [
            serde_json::json!(true),
            serde_json::json!(null),
            serde_json::json!([1]),
        ] {
            assert!(matches!(
                json_decimal_string(&bad, "x"),
                Err(QuotaError::Parse { .. })
            ));
        }
    }

    #[test]
    fn micros_measure_is_exact_without_clamping() {
        assert_eq!(micros_measure(0), QuotaMeasure::exact(0));
        assert_eq!(micros_measure(42), QuotaMeasure::exact(42));
    }

    #[test]
    fn remaining_subtracts_without_saturating() {
        assert_eq!(
            remaining(QuotaMeasure::exact(10), QuotaMeasure::exact(3)),
            QuotaMeasure::exact(7)
        );
        // used > limit：负数无法表示，诚实返回 Unknown，不伪造 0。
        assert_eq!(
            remaining(QuotaMeasure::exact(3), QuotaMeasure::exact(10)),
            QuotaMeasure::Unknown
        );
        assert_eq!(
            remaining(QuotaMeasure::Unknown, QuotaMeasure::exact(1)),
            QuotaMeasure::Unknown
        );
        assert_eq!(
            remaining(QuotaMeasure::Infinite, QuotaMeasure::exact(1)),
            QuotaMeasure::Unknown
        );
    }
}
