//! 预算控制（P3-6）。
//!
//! 多维预算（迭代/工具/时间/输入 token/输出 token/费用/输出字节/artifact 字节/
//! 并发工具上限）。达到预算时产生明确决策（软警告 / 硬超限）而非静默停止，
//! 由 Agent Loop 翻译为持久化事件（如 [`AgentEvent::Diagnostic`] 或终态事件）。

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
        }
    }
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
}

impl BudgetController {
    pub fn new(limits: BudgetLimits) -> Self {
        Self {
            limits,
            usage: BudgetUsage::default(),
            soft_ratio: DEFAULT_SOFT_RATIO,
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
        report
    }

    /// 记录一次迭代并立即检查（循环每轮常用入口）。
    pub fn tick_iteration(&mut self) -> BudgetReport {
        self.record_iteration();
        self.check()
    }
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
}
