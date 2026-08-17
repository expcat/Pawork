//! ACP Host：Agent Client Protocol 稳定 v1（`protocolVersion = 1`）适配宿主。
//!
//! 职责边界：本模块只做 ACP ↔ canonical 协议翻译，承载
//! [`pawork_protocol::adapter::ClientAdapter`] 实现与宿主胶水（[`host::AcpHost`]）。
//! 不持有 Provider 凭证、不构造第二个 Core、不消费 GUI Connection Protocol
//! frame；session ownership 一律复用
//! [`pawork_protocol::adapter::SessionRegistry`] 的 authoritative 记录，Core
//! 执行统一走 [`AcpCommandHost`]。
//!
//! 协议基线：wire `protocolVersion = 1`（整数），能力协商决定可选消息；
//! 实验 v2 不混入。未知方法显式拒绝（-32601），未知参数显式拒绝（-32602）。

pub mod adapter;
pub mod command_host;
pub mod host;
pub(crate) mod map;
pub mod wire;

pub use adapter::{
    AcpClientAdapter, AcpClientAdapterFactory, CancelTarget, CwdResolver, NegotiatedAcpAdapter,
    PermissionDecision, SessionResolver,
};
pub use command_host::{AcpCommandHost, AcpHostError};
pub use host::{AcpHost, OutboxItem, PromptResolution};
pub use wire::{JsonRpcError, JsonRpcId, JsonRpcMessage, PROTOCOL_VERSION};

/// 生成当前 Unix 毫秒时间戳（`pawork-domain` 无 `now()` 构造器）。
pub(crate) fn now_timestamp() -> pawork_domain::Timestamp {
    use std::time::{SystemTime, UNIX_EPOCH};
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0);
    pawork_domain::Timestamp::from_unix_millis(millis)
}
