//! 模型目录与能力管理（P2-7 / P15-8）。
//!
//! 维护内置模型目录、别名、能力过滤、上下文窗口校验与费用估算，让 Agent 与
//! UI 能正确选择模型。支持 Provider 动态发现与用户自定义模型覆盖。
//!
//! P15-8 能力证据：模型能力来自三处——静态声明（目录）、Provider 探测
//! （[`registry::ModelRegistry::probe_provider`]，同一 provider 只发现一次、
//! 线程安全、不持锁跨 await）与配置覆盖（[`registry::ModelRegistry::set_override`]）。
//! 三源以 provider-neutral 的 [`provider_api::ModelCapabilities`] 表达并合并取
//! 交集（覆盖不能放大静态/探测未支持的能力）；「请求 × 支持」的最终交集由
//! provider-runtime 的 CapabilityNegotiator 消费
//! [`registry::ModelRegistry::capability_snapshot`] 快照完成。
//!
//! 复用既有领域类型（[`agent_domain::ModelId`] / [`agent_domain::ProviderId`] /
//! [`agent_domain::TokenUsage`] / [`agent_domain::Cost`] 与
//! [`provider_api::ModelCapabilities`]），不重定义。

pub mod error;
pub mod pricing;
pub mod registry;

pub use error::RegistryError;
pub use pricing::{estimate_cost, ModelPricing, BUILTIN_RATE_CARD, BUILTIN_RATE_VERSION};
pub use registry::{
    caps, merge_capabilities, CapabilityEvidence, CapabilitySource, CatalogEntry, ModelRegistry,
    ProbeError, ProviderCapabilitySource, ProviderProbe,
};
