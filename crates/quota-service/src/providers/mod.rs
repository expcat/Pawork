//! 六家 Provider 的真实配额适配器与能力矩阵。
//!
//! 每家 Provider 提供一个构造函数，返回组装好的 [`crate::QuotaAdapter`]（基于
//! [`crate::adapters`] 中的可复用骨架）。Provider 名 / 端点 / 解析规则只存在于
//! 这里，[`crate::adapters`] 与 Agent Engine 都不按 Provider 名分支。

pub mod anthropic;
pub mod capability;
pub mod moonshot;
pub mod openai;
pub mod qwen;
pub mod xai;
pub mod zhipu;
