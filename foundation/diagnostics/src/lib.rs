//! Pawork 的安全可观测性基础设施。
//!
//! S6 波 B 激活：迁移 V1 结构化日志与脱敏能力，并新增全局脱敏 fmt 层
//! （`RedactingFmtLayer`）修复 V1 缺口——V1 `StructuredLogLayer` 只进内存
//! buffer，fmt 输出无脱敏。全局挂载由波 C 宿主装配
//! （`Registry.with(RedactingFmtLayer)`）。
//!
//! metrics 与诊断包（bundle）以 `experimental` feature 门控随迁；default
//! 不启用该 feature。

mod logging;

#[cfg(feature = "experimental")]
mod bundle;
#[cfg(feature = "experimental")]
mod metrics;

pub use logging::{
    LogBuffer, LogRecord, RedactingFmtLayer, Redactor, Sampling, StructuredLogLayer,
};

#[cfg(feature = "experimental")]
pub use bundle::{
    CrashDiagnostic, DiagnosticBundle, DiagnosticError, DiagnosticInput, DiagnosticLimits,
    DiagnosticLog, McpDiagnostic, PluginDiagnostic, ProviderDiagnostic,
};
#[cfg(feature = "experimental")]
pub use metrics::{HistogramSnapshot, MetricName, MetricSnapshot, Metrics, MetricsTimer};
