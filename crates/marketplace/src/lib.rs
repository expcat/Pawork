//! Pawork Plugin Marketplace（P17-3）。
//!
//! 多 source（registry / git / local）发现 → semver 范围与 exact / hash pin 解析 →
//! Ed25519 签名校验（绑定已校验 manifest + 全部条目）→ trust / team policy 闸门
//! （组织策略优先且 fail-closed）→ 经宿主注入的事务接口安装 / 更新 / 卸载六类子资源
//! （Skills / Agents / Hooks / MCP / LSP / Monitors）。
//!
//! Marketplace 绝不执行任何子资源：只提交声明并保证事务语义——任一失败整体回滚；
//! 卸载 Monitor 先 stop 再 unregister。安装集与 pin 持久化在可重放 state store。

pub mod error;
pub mod host;
pub mod manager;
pub mod pin;
pub mod policy;
pub mod signature;
pub mod source;
pub mod store;
pub mod trust;

pub use error::MarketplaceError;
pub use host::{RecordingHost, ResourceHost};
pub use manager::{InstallOutcome, Marketplace, UninstallOutcome, UpdateOutcome};
pub use pin::{Pin, PinMap};
pub use policy::{PolicyInput, TeamPolicy};
pub use signature::{
    canonical_payload, content_digest, content_digest_hex, sign_archive, Keyring, PackageSignature,
};
pub use source::{
    discover, Candidate, Discovery, InMemorySourceIo, IndexEntry, SourceIndex, SourceIo,
    SourceKind, SourceSpec,
};
pub use store::{AtomicFileStore, InstalledPackage, MarketplaceState, MemoryStore, StateStore};
pub use trust::{TrustConfig, TrustLevel};
