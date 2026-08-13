//! ACP Host（P17-7）：Agent Client Protocol 稳定 v1（protocolVersion = 1）适配宿主。
//!
//! 职责边界：本 crate 只做 ACP ↔ canonical 协议翻译，承载 [`client_adapter_api::ClientAdapter`]
//! 实现与宿主胶水（[`host::AcpHost`]）。不持有 Provider 凭证、不构造第二个 Core、
//! 不消费 GUI Connection Protocol frame；session ownership 一律复用
//! [`client_adapter_api::SessionRegistry`] 的 authoritative 记录，Core 执行统一走
//! `app-service` 的 [`app_service::ClientAdapterHost`]。
//!
//! 协议基线（2026-08 官方稳定版）：wire `protocolVersion = 1`（整数），能力协商决定
//! 可选消息；实验 v2 不混入。未知方法显式拒绝（-32601），未知参数显式拒绝（-32602），
//! 未协商能力在使用点显式降级（错误/记录），不静默丢字段。

pub mod adapter;
pub mod host;
pub mod map;
pub mod wire;

pub use adapter::{
    AcpClientAdapter, AcpClientAdapterFactory, CancelTarget, CwdResolver, NegotiatedAcpAdapter,
    PermissionDecision, SessionResolver,
};
pub use host::{AcpHost, OutboxItem, PromptResolution};
pub use wire::{JsonRpcError, JsonRpcId, JsonRpcMessage, PROTOCOL_VERSION};

/// 生成当前 Unix 毫秒时间戳（agent-domain 无 `now()` 构造器）。
pub(crate) fn now_timestamp() -> agent_domain::Timestamp {
    use std::time::{SystemTime, UNIX_EPOCH};
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0);
    agent_domain::Timestamp::from_unix_millis(millis)
}
