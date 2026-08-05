//! 模型目录与能力管理（P2-7）。
//!
//! 维护内置模型目录、别名、能力过滤、上下文窗口校验与费用估算，让 Agent 与
//! UI 能正确选择模型。支持 Provider 动态发现与用户自定义模型覆盖。
//!
//! 复用既有领域类型（[`agent_domain::ModelId`] / [`agent_domain::ProviderId`] /
//! [`agent_domain::TokenUsage`] / [`agent_domain::Cost`] 与
//! [`provider_api::ModelCapabilities`]），不重定义。

pub mod error;
pub mod pricing;
pub mod registry;

pub use error::RegistryError;
pub use pricing::{estimate_cost, ModelPricing};
pub use registry::{caps, CatalogEntry, ModelRegistry};
