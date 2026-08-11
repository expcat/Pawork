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

/// 双端点同时失败时的合并分类（单一事实源，P14 review §3.4）。
///
/// 优先级（高 → 低）：`Cancelled` > `Unauthorized` > `ReauthorizationRequired` >
/// `Forbidden` > `RateLimited` > `Timeout` > `Transient` > `Parse` >
/// `Unsupported` > `Other`，不统一降级为 `Other`：
///
/// - `Cancelled` 是本地意图：任一端取消即整体取消；
/// - 鉴权类错误走服务层 reauth 路径，不得被其他类别淹没；`Unauthorized`（凭证
///   无效）高于 `ReauthorizationRequired`（刷新后重试即可），保证合并确定；
/// - `Forbidden` 是持久权限信号（如非 Admin key），重试无法解决，优先于限流与
///   瞬时类；
/// - `RateLimited` / `Timeout` / `Transient` 的 `retry_after_ms` 取两端较大值
///   （与参数顺序无关）；胜者为优先级最高者，同优先级（同一变体）取
///   `retry_after_ms` 更大者，两者都相同（平局）时保留首参数 `limit`，
///   平局时 `Timeout` / `Transient` 的 `status` 即取 limit 的字段；
/// - 组合消息只含固定类别标签，绝不拼接子错误的 detail（可能携带远端正文），
///   胜者的 detail 一律覆盖为组合消息：参数顺序唯一可观察的影响是平局时
///   `status` 取首参数的值。
pub(crate) fn merge_dual_failures(
    limit: QuotaError,
    used: QuotaError,
    context: &str,
) -> QuotaError {
    let detail = format!(
        "{context} (limit: {}, used: {})",
        failure_label(&limit),
        failure_label(&used),
    );
    let retry_after = limit.retry_after_ms().max(used.retry_after_ms());
    let mut winner = pick_winner(limit, used);
    match &mut winner {
        QuotaError::Cancelled => {}
        QuotaError::RateLimited {
            detail: d,
            retry_after_ms: ra,
        }
        | QuotaError::Timeout {
            detail: d,
            retry_after_ms: ra,
            ..
        }
        | QuotaError::Transient {
            detail: d,
            retry_after_ms: ra,
            ..
        } => {
            *d = detail;
            *ra = retry_after;
        }
        QuotaError::Unauthorized { detail: d }
        | QuotaError::ReauthorizationRequired { detail: d }
        | QuotaError::Forbidden { detail: d }
        | QuotaError::Parse { detail: d }
        | QuotaError::Unsupported { detail: d }
        | QuotaError::Other { detail: d } => *d = detail,
    }
    winner
}

/// 取两端错误中优先级最高者；同优先级（同一变体）取 `retry_after_ms` 更大者，
/// 相等时保留首参数 `limit`（非 retryable 变体的 `retry_after_ms()` 恒为
/// `None`，即恒为平局）。平局时 `Timeout` / `Transient` 的 `status` 随之取
/// limit 的值。
fn pick_winner(limit: QuotaError, used: QuotaError) -> QuotaError {
    let limit_rank = failure_rank(&limit);
    let used_rank = failure_rank(&used);
    if limit_rank > used_rank
        || (limit_rank == used_rank
            && limit.retry_after_ms().unwrap_or(0) >= used.retry_after_ms().unwrap_or(0))
    {
        limit
    } else {
        used
    }
}

/// 合并优先级数值（越大越优先）。
fn failure_rank(error: &QuotaError) -> u8 {
    match error {
        QuotaError::Cancelled => 10,
        QuotaError::Unauthorized { .. } => 9,
        QuotaError::ReauthorizationRequired { .. } => 8,
        QuotaError::Forbidden { .. } => 7,
        QuotaError::RateLimited { .. } => 6,
        QuotaError::Timeout { .. } => 5,
        QuotaError::Transient { .. } => 4,
        QuotaError::Parse { .. } => 3,
        QuotaError::Unsupported { .. } => 2,
        QuotaError::Other { .. } => 1,
    }
}

/// 错误的固定类别标签（仅用于组合消息，不含任何远端文本）。
fn failure_label(error: &QuotaError) -> &'static str {
    match error {
        QuotaError::Cancelled => "cancelled",
        QuotaError::Unauthorized { .. } => "unauthorized",
        QuotaError::ReauthorizationRequired { .. } => "reauthorization-required",
        QuotaError::Forbidden { .. } => "forbidden",
        QuotaError::RateLimited { .. } => "rate-limited",
        QuotaError::Timeout { .. } => "timeout",
        QuotaError::Transient { .. } => "transient",
        QuotaError::Parse { .. } => "parse",
        QuotaError::Unsupported { .. } => "unsupported",
        QuotaError::Other { .. } => "other",
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

    #[test]
    fn merge_dual_failures_higher_priority_variant_wins() {
        // Cancelled 是本地意图，优先于一切。
        assert!(matches!(
            merge_dual_failures(QuotaError::Cancelled, QuotaError::forbidden("x"), "t"),
            QuotaError::Cancelled
        ));
        // 鉴权类优先于 Forbidden（服务层走 reauth 路径）。
        assert!(matches!(
            merge_dual_failures(
                QuotaError::forbidden("x"),
                QuotaError::unauthorized("x"),
                "t"
            ),
            QuotaError::Unauthorized { .. }
        ));
        // Unauthorized 高于 ReauthorizationRequired：两种参数顺序下分类一致。
        assert!(matches!(
            merge_dual_failures(
                QuotaError::reauthorization_required("x"),
                QuotaError::unauthorized("x"),
                "t"
            ),
            QuotaError::Unauthorized { .. }
        ));
        assert!(matches!(
            merge_dual_failures(
                QuotaError::unauthorized("x"),
                QuotaError::reauthorization_required("x"),
                "t"
            ),
            QuotaError::Unauthorized { .. }
        ));
        // Forbidden（持久权限信号）优先于 RateLimited：403 与 429 并存时取 403。
        assert!(matches!(
            merge_dual_failures(
                QuotaError::rate_limited("x", Some(1_000)),
                QuotaError::forbidden("x"),
                "t"
            ),
            QuotaError::Forbidden { .. }
        ));
        // RateLimited 优先于瞬时类，retry_after 取两端较大值。
        assert!(matches!(
            merge_dual_failures(
                QuotaError::transient("x", Some(503), None),
                QuotaError::rate_limited("x", Some(3_000)),
                "t"
            ),
            QuotaError::RateLimited {
                retry_after_ms: Some(3_000),
                ..
            }
        ));
        // Timeout（retryable）高于 Parse：保留 Timeout。
        assert!(matches!(
            merge_dual_failures(QuotaError::timeout("x"), QuotaError::parse("x"), "t"),
            QuotaError::Timeout { .. }
        ));
        // Parse 不被塌缩为 Other。
        assert!(matches!(
            merge_dual_failures(QuotaError::parse("a"), QuotaError::other("b"), "t"),
            QuotaError::Parse { .. }
        ));
    }

    #[test]
    fn merge_dual_failures_classification_and_retry_after_are_order_independent() {
        // 不同优先级（RateLimited > Timeout）：反序后分类与 retry_after 不变。
        let forward = merge_dual_failures(
            QuotaError::timeout("t"),
            QuotaError::rate_limited("r", Some(2_000)),
            "ctx",
        );
        let reverse = merge_dual_failures(
            QuotaError::rate_limited("r", Some(2_000)),
            QuotaError::timeout("t"),
            "ctx",
        );
        for merged in [&forward, &reverse] {
            assert!(matches!(
                merged,
                QuotaError::RateLimited {
                    retry_after_ms: Some(2_000),
                    ..
                }
            ));
        }

        // 同变体、retry_after 不同：反序后胜者（retry_after 更大者）及其
        // status 不变。
        let forward = merge_dual_failures(
            QuotaError::transient("t", Some(502), Some(3_000)),
            QuotaError::transient("t", Some(503), Some(1_000)),
            "ctx",
        );
        let reverse = merge_dual_failures(
            QuotaError::transient("t", Some(503), Some(1_000)),
            QuotaError::transient("t", Some(502), Some(3_000)),
            "ctx",
        );
        for merged in [&forward, &reverse] {
            match merged {
                QuotaError::Transient {
                    status,
                    retry_after_ms,
                    ..
                } => {
                    assert_eq!(*status, Some(502));
                    assert_eq!(*retry_after_ms, Some(3_000));
                }
                other => panic!("expected Transient, got {other:?}"),
            }
        }
    }

    #[test]
    fn merge_dual_failures_message_contains_only_fixed_labels() {
        let combined = merge_dual_failures(
            QuotaError::forbidden("remote detail from body"),
            QuotaError::parse("remote value: EUR"),
            "openai: both endpoints failed",
        );
        let detail = match combined {
            QuotaError::Forbidden { detail } => detail,
            other => panic!("expected Forbidden, got {other:?}"),
        };
        assert_eq!(
            detail,
            "openai: both endpoints failed (limit: forbidden, used: parse)"
        );
        assert!(!detail.contains("remote"));
    }

    #[test]
    fn merge_dual_failures_retry_after_takes_max_and_tie_keeps_first_param() {
        // 同变体（Timeout）且 retry_after 均为 None（平局）：保留首参数 limit。
        let combined = merge_dual_failures(
            QuotaError::timeout("t"),
            QuotaError::timeout("t"),
            "xai: both postpaid endpoints failed",
        );
        match combined {
            QuotaError::Timeout {
                detail,
                status,
                retry_after_ms,
            } => {
                assert_eq!(status, None);
                assert_eq!(retry_after_ms, None);
                assert!(detail.contains("xai: both postpaid endpoints failed"));
                assert!(detail.contains("timeout"));
            }
            other => panic!("expected Timeout, got {other:?}"),
        }

        // 同变体、retry_after 相同（平局）：status 取首参数（limit），
        // 反序后首参数不同，status 随之改变。
        let first = merge_dual_failures(
            QuotaError::transient("t", Some(502), Some(2_000)),
            QuotaError::transient("t", Some(503), Some(2_000)),
            "ctx",
        );
        let swapped = merge_dual_failures(
            QuotaError::transient("t", Some(503), Some(2_000)),
            QuotaError::transient("t", Some(502), Some(2_000)),
            "ctx",
        );
        let status_of = |err: &QuotaError| match err {
            QuotaError::Transient { status, .. } => *status,
            other => panic!("expected Transient, got {other:?}"),
        };
        assert_eq!(status_of(&first), Some(502));
        assert_eq!(status_of(&swapped), Some(503));
        assert_eq!(first.retry_after_ms(), Some(2_000));
        assert_eq!(swapped.retry_after_ms(), Some(2_000));
    }
}
