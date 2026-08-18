//! Pawork 的安全可观测性基础设施。
//!
//! S6 波 B 激活：迁移 V1 结构化日志与脱敏能力，并新增全局脱敏 fmt 层
//! （`RedactingFmtLayer`）修复 V1 缺口——V1 `StructuredLogLayer` 只进内存
//! buffer，fmt 输出无脱敏。全局挂载由波 C 宿主装配
//! （`Registry.with(RedactingFmtLayer)`）。
//!
//! 本包当前只保留全局脱敏 fmt 层（`RedactingFmtLayer` / `Redactor`）。
//! 结构化日志缓冲与诊断包已在 R0 波 B 删除，待 R1 迁宿主。

mod logging;

pub use logging::{RedactingFmtLayer, Redactor};
