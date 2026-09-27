//! 共享模型目录、能力证据与协商、定点计价。
//!
//! 宿主和 Provider 适配器共同消费；仅依赖 canonical domain，不依赖任何
//! 具体 Provider、HTTP 客户端、数据库或凭证后端。目录探测通过调用方注入的
//! `ModelProvider` 执行，缓存与能力合并不绑定具体网络实现。

pub mod error;
pub mod negotiate;
pub mod pricing;
pub mod registry;

pub use error::RegistryError;
pub use negotiate::{clamp_reasoning_to_thinking, CapabilityNegotiator};
pub use pricing::{estimate_cost, ModelPricing, BUILTIN_RATE_CARD, BUILTIN_RATE_VERSION};
pub use registry::{
    caps, merge_capabilities, CapabilityEvidence, CapabilitySource, CatalogEntry, ModelRegistry,
    ProbeError, ProviderCapabilitySource, ProviderProbe,
};
