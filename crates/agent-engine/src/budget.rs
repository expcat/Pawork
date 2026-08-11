//! 预算控制（P3-6）。
//!
//! 多维预算（迭代/工具/时间/输入 token/输出 token/费用/输出字节/artifact 字节/
//! 并发工具上限）。达到预算时产生明确决策（软警告 / 硬超限）而非静默停止，
//! 由 Agent Loop 翻译为持久化事件（如 [`AgentEvent::Diagnostic`] 或终态事件）。
//!
//! P14-7 预算联动：`BudgetController` 可注入供应商中立的 [`ExternalQuotaSignal`]
//! （远端额度水位），`check()` 将其折算为 [`BudgetDimension::ProviderQuota`] 的
//! 软/硬决策——仅新鲜且精确的 `exhausted` 信号硬停；过期、推算或抓取信号
//! 绝不停机，并始终降级为软告警，可通过 [`BudgetController::quota_signal_note`]
//! 解释原因。
//! 本模块不依赖 `quota-service`，避免 crate 循环依赖。

use std::collections::BTreeSet;
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// 软警告默认阈值（占硬上限的比例）。
pub const DEFAULT_SOFT_RATIO: f64 = 0.8;

/// 单个预算维度。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BudgetDimension {
    Iterations,
    ToolCalls,
    Duration,
    InputTokens,
    OutputTokens,
    Cost,
    OutputBytes,
    ArtifactBytes,
    Concurrency,
    /// 供应商中立的远端额度水位（由外部信号驱动，见 [`ExternalQuotaSignal`]）。
    ProviderQuota,
}

impl BudgetDimension {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Iterations => "iterations",
            Self::ToolCalls => "tool_calls",
            Self::Duration => "duration_ms",
            Self::InputTokens => "input_tokens",
            Self::OutputTokens => "output_tokens",
            Self::Cost => "cost_micros",
            Self::OutputBytes => "output_bytes",
            Self::ArtifactBytes => "artifact_bytes",
            Self::Concurrency => "concurrency",
            Self::ProviderQuota => "provider_quota",
        }
    }
}

/// 外部额度信号的可信度（P14-7），与 canonical `exact > derived > scraped` 一致。
///
/// 额度观测方（quota 适配器 / 窗口聚合）注入信号时附上来源可信度；只有
/// [`Exact`](Self::Exact) 可在信号新鲜且明确耗尽时触发硬停。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuotaSignalConfidence {
    /// 官方 API / 账单端点直接观测的精确值。
    Exact,
    /// 基于远端基线与本地增量推算的值。
    Derived,
    /// 从网页等非结构化来源抓取的值。
    Scraped,
}

impl QuotaSignalConfidence {
    /// canonical 可信度优先级：`exact > derived > scraped`。
    pub const fn priority(self) -> u8 {
        match self {
            Self::Exact => 3,
            Self::Derived => 2,
            Self::Scraped => 1,
        }
    }
}

impl Ord for QuotaSignalConfidence {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.priority().cmp(&other.priority())
    }
}

impl PartialOrd for QuotaSignalConfidence {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// 供应商中立的远端额度水位信号（P14-7 预算联动接口）。
///
/// 由额度观测方经 [`BudgetController::set_external_quota`] 注入；不含任何
/// Provider 名称。`remaining_ratio_ppm` 以百万分之一表达剩余比例
/// （`0` = 已耗尽，`1_000_000` = 满额），`exhausted` 为提供方明确触顶标志，
/// 二者独立，避免窗口对齐误差。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExternalQuotaSignal {
    /// 剩余额度比例（ppm，`0..=1_000_000`）。
    pub remaining_ratio_ppm: u64,
    /// 提供方明确报告已触顶（独立于 ppm）。
    pub exhausted: bool,
    /// 信号已过期（如超过刷新窗口仍未更新）。
    pub stale: bool,
    /// 信号可信度。
    pub confidence: QuotaSignalConfidence,
}

/// 多维预算上限。任意维度为 `None` 表示不受该维度约束。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BudgetLimits {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_iterations: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tool_calls: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_duration_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_input_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_cost_micros: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_artifact_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_concurrency: Option<u64>,
}

/// 当前预算消耗。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BudgetUsage {
    pub iterations: u64,
    pub tool_calls: u64,
    pub elapsed_ms: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_micros: u64,
    pub output_bytes: u64,
    pub artifact_bytes: u64,
    pub concurrency: u64,
}

/// 预算检查结果。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BudgetReport {
    /// 已达软阈值的维度（可继续，但应发出警告事件）。
    pub soft_warnings: BTreeSet<BudgetDimension>,
    /// 已达硬上限的维度（必须停止，发出明确事件）。
    pub hard_exceeded: BTreeSet<BudgetDimension>,
}

impl BudgetReport {
    pub fn is_ok(&self) -> bool {
        self.soft_warnings.is_empty() && self.hard_exceeded.is_empty()
    }

    pub fn must_stop(&self) -> bool {
        !self.hard_exceeded.is_empty()
    }
}

/// 预算控制器：跟踪消耗并按软/硬阈值产出决策。
#[derive(Clone, Debug)]
pub struct BudgetController {
    limits: BudgetLimits,
    usage: BudgetUsage,
    soft_ratio: f64,
    external_quota: Option<ExternalQuotaSignal>,
}

impl BudgetController {
    pub fn new(limits: BudgetLimits) -> Self {
        Self {
            limits,
            usage: BudgetUsage::default(),
            soft_ratio: DEFAULT_SOFT_RATIO,
            external_quota: None,
        }
    }

    pub fn with_soft_ratio(mut self, ratio: f64) -> Self {
        let ratio = ratio.clamp(0.0, 1.0);
        self.soft_ratio = if ratio == 0.0 {
            DEFAULT_SOFT_RATIO
        } else {
            ratio
        };
        self
    }

    pub const fn limits(&self) -> &BudgetLimits {
        &self.limits
    }

    pub const fn usage(&self) -> &BudgetUsage {
        &self.usage
    }

    /// 当前外部额度信号（`None` 表示未注入）。
    pub const fn external_quota(&self) -> Option<&ExternalQuotaSignal> {
        self.external_quota.as_ref()
    }

    /// 注入供应商中立的远端额度水位信号（`remaining_ratio_ppm` 饱和到 `1_000_000`）。
    pub fn set_external_quota(&mut self, signal: ExternalQuotaSignal) {
        self.external_quota = Some(ExternalQuotaSignal {
            remaining_ratio_ppm: signal.remaining_ratio_ppm.min(1_000_000),
            ..signal
        });
    }

    /// 清除外部额度信号，`ProviderQuota` 维度恢复不约束。
    pub fn clear_external_quota(&mut self) {
        self.external_quota = None;
    }

    pub fn record_iteration(&mut self) {
        self.usage.iterations += 1;
    }

    pub fn record_tool_call(&mut self) {
        self.usage.tool_calls += 1;
    }

    pub fn record_tokens(&mut self, input: u64, output: u64) {
        self.usage.input_tokens += input;
        self.usage.output_tokens += output;
    }

    pub fn record_cost(&mut self, micros: u64) {
        self.usage.cost_micros += micros;
    }

    pub fn record_output(&mut self, bytes: u64) {
        self.usage.output_bytes += bytes;
    }

    pub fn record_artifact(&mut self, bytes: u64) {
        self.usage.artifact_bytes += bytes;
    }

    pub fn set_concurrency(&mut self, current: u64) {
        self.usage.concurrency = current;
    }

    pub fn set_elapsed(&mut self, elapsed: Duration) {
        self.usage.elapsed_ms = elapsed.as_millis().try_into().unwrap_or(u64::MAX);
    }

    /// 评估所有维度，返回软/硬超限集合（不修改状态）。
    pub fn check(&self) -> BudgetReport {
        let mut report = BudgetReport::default();
        let l = &self.limits;
        let u = &self.usage;
        let ratio = self.soft_ratio;

        for (used, limit, dim) in [
            (u.iterations, l.max_iterations, BudgetDimension::Iterations),
            (u.tool_calls, l.max_tool_calls, BudgetDimension::ToolCalls),
            (u.elapsed_ms, l.max_duration_ms, BudgetDimension::Duration),
            (
                u.input_tokens,
                l.max_input_tokens,
                BudgetDimension::InputTokens,
            ),
            (
                u.output_tokens,
                l.max_output_tokens,
                BudgetDimension::OutputTokens,
            ),
            (u.cost_micros, l.max_cost_micros, BudgetDimension::Cost),
            (
                u.output_bytes,
                l.max_output_bytes,
                BudgetDimension::OutputBytes,
            ),
            (
                u.artifact_bytes,
                l.max_artifact_bytes,
                BudgetDimension::ArtifactBytes,
            ),
            (
                u.concurrency,
                l.max_concurrency,
                BudgetDimension::Concurrency,
            ),
        ] {
            let Some(limit) = limit else { continue };
            if used >= limit {
                report.hard_exceeded.insert(dim);
            } else if (used as f64) >= (limit as f64) * ratio {
                report.soft_warnings.insert(dim);
            }
        }

        // P14-7：外部额度信号折算为 ProviderQuota 维度。
        match self.assess_quota() {
            Some(QuotaAssessment::Hard { .. }) => {
                report.hard_exceeded.insert(BudgetDimension::ProviderQuota);
            }
            Some(QuotaAssessment::Soft { .. }) => {
                report.soft_warnings.insert(BudgetDimension::ProviderQuota);
            }
            None | Some(QuotaAssessment::AboveSoft { .. }) => {}
        }
        report
    }

    /// 记录一次迭代并立即检查（循环每轮常用入口）。
    pub fn tick_iteration(&mut self) -> BudgetReport {
        self.record_iteration();
        self.check()
    }

    /// 对当前外部额度信号的可解释说明（未注入信号时返回 `None`）。
    ///
    /// 与 `check()` 共用同一判定：硬停仅来自新鲜 [`QuotaSignalConfidence::Exact`]
    /// 的 `exhausted` 信号；stale / Derived / Scraped 信号始终降级为软告警，
    /// 说明文字会明确给出原因。
    pub fn quota_signal_note(&self) -> Option<String> {
        let soft_pct = (1.0 - self.soft_ratio) * 100.0;
        let note = match self.assess_quota()? {
            QuotaAssessment::Hard { remaining_ppm } => {
                let pct = remaining_ppm as f64 / 10_000.0;
                format!(
                    "provider quota exhausted from fresh exact signal (remaining {pct:.2}%); hard stop"
                )
            }
            QuotaAssessment::Soft { remaining_ppm } => {
                let signal = self
                    .external_quota
                    .as_ref()
                    .expect("quota assessment requires an external signal");
                let pct = remaining_ppm as f64 / 10_000.0;
                let degradation = quota_signal_degradation(signal);
                if signal.exhausted {
                    format!(
                        "provider quota exhausted but observed by {}; soft warning only, not hard stop (remaining {pct:.2}%)",
                        degradation.expect("non-exact or stale exhausted signal is degraded")
                    )
                } else if remaining_ppm <= self.quota_soft_remaining_ppm() {
                    match degradation {
                        Some(why) => format!(
                            "provider quota low: remaining {pct:.2}% below soft threshold {soft_pct:.2}% ({why}; soft warning only, not hard stop)"
                        ),
                        None => format!(
                            "provider quota low: remaining {pct:.2}% below soft threshold {soft_pct:.2}%"
                        ),
                    }
                } else {
                    format!(
                        "provider quota signal degraded: remaining {pct:.2}% above soft threshold {soft_pct:.2}% ({}; soft warning only, not hard stop)",
                        degradation.expect("above-threshold soft signal is degraded")
                    )
                }
            }
            QuotaAssessment::AboveSoft { remaining_ppm } => {
                let pct = remaining_ppm as f64 / 10_000.0;
                format!("provider quota remaining {pct:.2}% above soft threshold {soft_pct:.2}%")
            }
        };
        Some(note)
    }

    fn quota_soft_remaining_ppm(&self) -> u64 {
        ((1.0 - self.soft_ratio) * 1_000_000.0).round() as u64
    }

    /// 额度信号判定（`check()` 与说明共用，保证决策一致）。
    fn assess_quota(&self) -> Option<QuotaAssessment> {
        let signal = self.external_quota.as_ref()?;
        let remaining_ppm = signal.remaining_ratio_ppm.min(1_000_000);
        let fresh_exact = !signal.stale && signal.confidence == QuotaSignalConfidence::Exact;
        if signal.exhausted {
            return Some(if fresh_exact {
                QuotaAssessment::Hard { remaining_ppm }
            } else {
                QuotaAssessment::Soft { remaining_ppm }
            });
        }
        // 低于现有软阈值（剩余比例 ≤ 1 - soft_ratio）→ 软告警。
        // stale / Derived / Scraped 即使高于软阈值也要暴露降级状态。
        if remaining_ppm <= self.quota_soft_remaining_ppm() || !fresh_exact {
            Some(QuotaAssessment::Soft { remaining_ppm })
        } else {
            Some(QuotaAssessment::AboveSoft { remaining_ppm })
        }
    }
}

fn quota_signal_degradation(signal: &ExternalQuotaSignal) -> Option<&'static str> {
    match (signal.stale, signal.confidence) {
        (false, QuotaSignalConfidence::Exact) => None,
        (true, QuotaSignalConfidence::Exact) => Some("stale exact signal"),
        (false, QuotaSignalConfidence::Derived) => Some("derived signal"),
        (true, QuotaSignalConfidence::Derived) => Some("stale derived signal"),
        (false, QuotaSignalConfidence::Scraped) => Some("scraped signal"),
        (true, QuotaSignalConfidence::Scraped) => Some("stale scraped signal"),
    }
}

/// 额度信号评估结果（内部类型）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum QuotaAssessment {
    /// 剩余高于软阈值，不告警。
    AboveSoft { remaining_ppm: u64 },
    /// 软告警（附原因）。
    Soft { remaining_ppm: u64 },
    /// 硬停（仅新鲜 Exact 的 exhausted 信号）。
    Hard { remaining_ppm: u64 },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limits() -> BudgetLimits {
        BudgetLimits {
            max_iterations: Some(3),
            max_tool_calls: Some(2),
            max_duration_ms: Some(1_000),
            max_input_tokens: Some(1_000),
            max_output_tokens: Some(500),
            max_cost_micros: Some(1_000),
            max_output_bytes: Some(10_000),
            max_artifact_bytes: Some(5_000),
            max_concurrency: Some(4),
        }
    }

    #[test]
    fn under_limits_reports_ok() {
        let mut ctrl = BudgetController::new(limits());
        ctrl.record_iteration();
        ctrl.record_tool_call();
        ctrl.record_tokens(10, 5);
        assert!(ctrl.check().is_ok());
    }

    #[test]
    fn soft_warning_before_hard_limit() {
        let limits = BudgetLimits {
            max_iterations: Some(10),
            ..BudgetLimits::default()
        };
        let mut ctrl = BudgetController::new(limits);
        for _ in 0..8 {
            ctrl.record_iteration();
        }
        let report = ctrl.check();
        assert!(report.soft_warnings.contains(&BudgetDimension::Iterations));
        assert!(!report.hard_exceeded.contains(&BudgetDimension::Iterations));
        assert!(!report.must_stop());
    }

    #[test]
    fn hard_limit_stops_and_is_not_silent() {
        let mut ctrl = BudgetController::new(limits());
        for _ in 0..3 {
            ctrl.record_iteration();
        }
        let report = ctrl.check();
        assert!(report.hard_exceeded.contains(&BudgetDimension::Iterations));
        assert!(report.must_stop());
    }

    #[test]
    fn token_and_cost_accumulate_and_exceed() {
        let limits = BudgetLimits {
            max_input_tokens: Some(100),
            max_cost_micros: Some(1_000),
            ..BudgetLimits::default()
        };
        let mut ctrl = BudgetController::new(limits);
        ctrl.record_tokens(60, 0);
        assert!(ctrl.check().is_ok());
        ctrl.record_tokens(50, 0);
        ctrl.record_cost(1_000);
        let report = ctrl.check();
        assert!(report.hard_exceeded.contains(&BudgetDimension::InputTokens));
        assert!(report.hard_exceeded.contains(&BudgetDimension::Cost));
    }

    #[test]
    fn concurrency_exceeds() {
        let limits = BudgetLimits {
            max_concurrency: Some(4),
            ..BudgetLimits::default()
        };
        let mut ctrl = BudgetController::new(limits);
        ctrl.set_concurrency(4);
        assert!(ctrl
            .check()
            .hard_exceeded
            .contains(&BudgetDimension::Concurrency));
        ctrl.set_concurrency(3);
        assert!(ctrl.check().is_ok());
    }

    #[test]
    fn duration_exceeds_after_elapsed() {
        let limits = BudgetLimits {
            max_duration_ms: Some(100),
            ..BudgetLimits::default()
        };
        let mut ctrl = BudgetController::new(limits);
        ctrl.set_elapsed(Duration::from_millis(99));
        assert!(!ctrl.check().must_stop());
        ctrl.set_elapsed(Duration::from_millis(100));
        assert!(ctrl.check().must_stop());
    }

    #[test]
    fn tick_iteration_records_and_checks() {
        let mut ctrl = BudgetController::new(BudgetLimits {
            max_iterations: Some(1),
            ..BudgetLimits::default()
        });
        let report = ctrl.tick_iteration();
        assert!(report.must_stop());
        assert_eq!(ctrl.usage().iterations, 1);
    }

    #[test]
    fn none_limits_never_exceed() {
        let mut ctrl = BudgetController::new(BudgetLimits::default());
        ctrl.record_tokens(u64::MAX / 2, u64::MAX / 2);
        ctrl.record_cost(u64::MAX / 2);
        ctrl.record_iteration();
        assert!(ctrl.check().is_ok());
    }

    fn quota_signal(remaining_ratio_ppm: u64, exhausted: bool, stale: bool) -> ExternalQuotaSignal {
        ExternalQuotaSignal {
            remaining_ratio_ppm,
            exhausted,
            stale,
            confidence: QuotaSignalConfidence::Exact,
        }
    }

    #[test]
    fn provider_quota_dimension_name_and_serde() {
        assert_eq!(BudgetDimension::ProviderQuota.as_str(), "provider_quota");
        let json = serde_json::to_string(&BudgetDimension::ProviderQuota).unwrap();
        assert_eq!(json, "\"provider_quota\"");
    }

    #[test]
    fn quota_confidence_matches_canonical_order_and_serde() {
        assert!(QuotaSignalConfidence::Exact > QuotaSignalConfidence::Derived);
        assert!(QuotaSignalConfidence::Derived > QuotaSignalConfidence::Scraped);
        for (confidence, expected) in [
            (QuotaSignalConfidence::Exact, "\"exact\""),
            (QuotaSignalConfidence::Derived, "\"derived\""),
            (QuotaSignalConfidence::Scraped, "\"scraped\""),
        ] {
            let json = serde_json::to_string(&confidence).unwrap();
            assert_eq!(json, expected);
            let back: QuotaSignalConfidence = serde_json::from_str(&json).unwrap();
            assert_eq!(back, confidence);
        }
    }

    #[test]
    fn no_external_quota_signal_leaves_report_unchanged() {
        let ctrl = BudgetController::new(BudgetLimits::default());
        assert!(ctrl.check().is_ok());
        assert!(ctrl.external_quota().is_none());
        assert!(ctrl.quota_signal_note().is_none());
    }

    #[test]
    fn ten_percent_remaining_is_soft_warning() {
        let mut ctrl = BudgetController::new(BudgetLimits::default());
        ctrl.set_external_quota(quota_signal(100_000, false, false));
        let report = ctrl.check();
        assert!(report
            .soft_warnings
            .contains(&BudgetDimension::ProviderQuota));
        assert!(!report
            .hard_exceeded
            .contains(&BudgetDimension::ProviderQuota));
        assert!(!report.must_stop());
        let note = ctrl.quota_signal_note().unwrap();
        assert!(note.contains("10.00%"), "note: {note}");
        assert!(note.contains("soft threshold 20.00%"), "note: {note}");
    }

    #[test]
    fn zero_percent_exhausted_fresh_exact_is_hard_stop() {
        let mut ctrl = BudgetController::new(BudgetLimits::default());
        ctrl.set_external_quota(quota_signal(0, true, false));
        let report = ctrl.check();
        assert!(report
            .hard_exceeded
            .contains(&BudgetDimension::ProviderQuota));
        assert!(report.must_stop());
        assert!(ctrl.quota_signal_note().unwrap().contains("hard stop"));
    }

    #[test]
    fn high_ppm_exhausted_fresh_exact_is_still_hard_stop() {
        let mut ctrl = BudgetController::new(BudgetLimits::default());
        ctrl.set_external_quota(quota_signal(900_000, true, false));
        let report = ctrl.check();
        assert!(report
            .hard_exceeded
            .contains(&BudgetDimension::ProviderQuota));
        assert!(report.must_stop());
        let note = ctrl.quota_signal_note().unwrap();
        assert!(note.contains("90.00%"), "note: {note}");
        assert!(note.contains("hard stop"), "note: {note}");
    }

    #[test]
    fn zero_remaining_without_exhausted_flag_is_soft() {
        let mut ctrl = BudgetController::new(BudgetLimits::default());
        ctrl.set_external_quota(quota_signal(0, false, false));
        let report = ctrl.check();
        assert!(report
            .soft_warnings
            .contains(&BudgetDimension::ProviderQuota));
        assert!(!report.must_stop());
    }

    #[test]
    fn stale_exhausted_signal_never_hard_stops() {
        let mut ctrl = BudgetController::new(BudgetLimits::default());
        ctrl.set_external_quota(quota_signal(0, true, true));
        let report = ctrl.check();
        assert!(report
            .soft_warnings
            .contains(&BudgetDimension::ProviderQuota));
        assert!(!report
            .hard_exceeded
            .contains(&BudgetDimension::ProviderQuota));
        assert!(!report.must_stop());
        let note = ctrl.quota_signal_note().unwrap();
        assert!(note.contains("stale exact signal"), "note: {note}");
        assert!(note.contains("not hard stop"), "note: {note}");
    }

    #[test]
    fn derived_and_scraped_exhausted_signals_never_hard_stop() {
        for (confidence, marker) in [
            (QuotaSignalConfidence::Derived, "derived signal"),
            (QuotaSignalConfidence::Scraped, "scraped signal"),
        ] {
            let mut ctrl = BudgetController::new(BudgetLimits::default());
            ctrl.set_external_quota(ExternalQuotaSignal {
                remaining_ratio_ppm: 0,
                exhausted: true,
                stale: false,
                confidence,
            });
            let report = ctrl.check();
            assert!(report
                .soft_warnings
                .contains(&BudgetDimension::ProviderQuota));
            assert!(!report
                .hard_exceeded
                .contains(&BudgetDimension::ProviderQuota));
            assert!(!report.must_stop());
            let note = ctrl.quota_signal_note().unwrap();
            assert!(note.contains(marker), "note: {note}");
            assert!(note.contains("not hard stop"), "note: {note}");
        }
    }

    #[test]
    fn stale_low_remaining_is_soft_and_explained() {
        let mut ctrl = BudgetController::new(BudgetLimits::default());
        ctrl.set_external_quota(quota_signal(100_000, false, true));
        let report = ctrl.check();
        assert!(report
            .soft_warnings
            .contains(&BudgetDimension::ProviderQuota));
        assert!(!report.must_stop());
        let note = ctrl.quota_signal_note().unwrap();
        assert!(note.contains("stale exact signal"), "note: {note}");
        assert!(note.contains("not hard stop"), "note: {note}");
    }

    #[test]
    fn degraded_above_soft_threshold_still_soft_warns() {
        for (confidence, stale, marker) in [
            (QuotaSignalConfidence::Exact, true, "stale exact signal"),
            (QuotaSignalConfidence::Derived, false, "derived signal"),
            (QuotaSignalConfidence::Scraped, false, "scraped signal"),
            (QuotaSignalConfidence::Derived, true, "stale derived signal"),
        ] {
            let mut ctrl = BudgetController::new(BudgetLimits::default());
            ctrl.set_external_quota(ExternalQuotaSignal {
                remaining_ratio_ppm: 800_000,
                exhausted: false,
                stale,
                confidence,
            });
            let report = ctrl.check();
            assert!(report
                .soft_warnings
                .contains(&BudgetDimension::ProviderQuota));
            assert!(!report
                .hard_exceeded
                .contains(&BudgetDimension::ProviderQuota));
            assert!(!report.must_stop());
            let note = ctrl.quota_signal_note().unwrap();
            assert!(note.contains(marker), "note: {note}");
            assert!(note.contains("above soft threshold"), "note: {note}");
        }
    }

    #[test]
    fn fresh_exact_above_soft_threshold_is_ok() {
        let mut ctrl = BudgetController::new(BudgetLimits::default());
        ctrl.set_external_quota(quota_signal(800_000, false, false));
        let report = ctrl.check();
        assert!(report.is_ok());
        assert!(ctrl.quota_signal_note().unwrap().contains("80.00%"));
    }

    #[test]
    fn clear_external_quota_restores_ok() {
        let mut ctrl = BudgetController::new(BudgetLimits::default());
        ctrl.set_external_quota(quota_signal(0, true, false));
        assert!(ctrl.check().must_stop());
        ctrl.clear_external_quota();
        assert!(ctrl.external_quota().is_none());
        assert!(ctrl.check().is_ok());
        assert!(ctrl.quota_signal_note().is_none());
    }

    #[test]
    fn set_external_quota_clamps_ppm_and_round_trips_serde() {
        let mut ctrl = BudgetController::new(BudgetLimits::default());
        ctrl.set_external_quota(quota_signal(1_500_000, false, false));
        assert_eq!(
            ctrl.external_quota().unwrap().remaining_ratio_ppm,
            1_000_000
        );

        let signal = quota_signal(123_456, true, false);
        let json = serde_json::to_string(&signal).unwrap();
        let back: ExternalQuotaSignal = serde_json::from_str(&json).unwrap();
        assert_eq!(signal, back);
    }

    #[test]
    fn quota_soft_threshold_tracks_custom_soft_ratio() {
        let mut ctrl = BudgetController::new(BudgetLimits::default()).with_soft_ratio(0.5);
        // 剩余 60% > 1 - 0.5 = 50% 阈值 → 不告警。
        ctrl.set_external_quota(quota_signal(600_000, false, false));
        assert!(ctrl.check().is_ok());
        // 剩余 50% 恰好触达阈值 → 软告警。
        ctrl.set_external_quota(quota_signal(500_000, false, false));
        let report = ctrl.check();
        assert!(report
            .soft_warnings
            .contains(&BudgetDimension::ProviderQuota));
        assert!(!report.must_stop());
    }
}
