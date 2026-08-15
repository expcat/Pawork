//! Provider 核心机制（S5 波 A，迁自 V1 provider-runtime 剩余模块 + model-registry）。
//!
//! - [`usage`]：token usage / stop reason 归一 +「请求内最新快照、跨请求累加」聚合器；
//! - [`pricing`]：micros 整数定价与费用估算（`BUILTIN_RATE_VERSION` 机制保留）；
//! - [`registry`]：模型目录、别名、三源能力证据（static / probe / override）与
//!   Provider 探测缓存（迁自 V1 `model-registry` 整包机制）；
//! - [`negotiate`]：能力协商纯函数（证据快照 × 请求要求，迁自 V1
//!   `provider-runtime::negotiate`）；
//! - [`reasoning`]：reasoning continuation 保护 trait（无 blob store 依赖；
//!   持久实现属宿主组装层，激活推迟）。
//!
//! 不迁移：`stream_assembly`（组装归 `pawork-providers` 适配器）、
//! `ModelPricingRef`（避免与 [`pricing::ModelPricing`] 双轨）、capability 死函数
//! （`structured_output_supported` 等零消费者助手）。

pub mod negotiate;
pub mod error;
pub mod pricing;
pub mod reasoning;
pub mod registry;
pub mod usage;

pub use negotiate::{clamp_reasoning_to_thinking, CapabilityNegotiator};
pub use error::RegistryError;
pub use pricing::{estimate_cost, ModelPricing, BUILTIN_RATE_CARD, BUILTIN_RATE_VERSION};
pub use reasoning::{ReasoningProtectError, ReasoningProtector};
pub use registry::{
    caps, merge_capabilities, CapabilityEvidence, CapabilitySource, CatalogEntry, ModelRegistry,
    ProbeError, ProviderCapabilitySource, ProviderProbe,
};
pub use usage::{map_stop_reason, normalize_usage, UsageAccumulator};
