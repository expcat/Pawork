//! Usage / stop reason 归一（薄转发）。
//!
//! 实现已迁至 `pawork-provider-core::usage`（S5 波 A）；本模块保留原路径
//! re-export，流解析调用点无需改动。相关测试随迁 provider-core。

pub use pawork_provider_core::usage::{map_stop_reason, normalize_usage};
