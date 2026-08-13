//! Codex App Server client adapter（P18-11）。
//!
//! 官方 Codex App Server 线协议（stdio JSONL，JSON-RPC *风格*但省略 `jsonrpc`
//! 字段）↔ Pawork canonical [`client_adapter_api`] / [`core_api`]。
//!
//! 协议基线（2026-08，已核对 developers.openai.com/codex/app-server 与
//! openai/codex `codex-rs/app-server/README.md`）：
//!
//! - 握手：`initialize` 请求 → `initialized` 通知；握手前拒绝 *Not initialized*；
//!   重复 `initialize` 返回 *Already initialized*。
//! - 生命周期：`thread/start` `thread/resume` `thread/fork`（`parentThreadId` /
//!   `forkedFromId` 保留 subagent 血缘）；`turn/start` `turn/steer` `turn/interrupt`；
//!   item 通知与 `turn/completed`；压缩为 `thread/compact/start` + `contextCompaction`
//!   item（legacy `thread/compacted` 已废弃，不得视为等价）。
//! - 审批为 server→client JSON-RPC **请求** `item/commandExecution/requestApproval`。
//! - 有界 ingress：饱和时 `-32001 Server overloaded; retry later.`。
//! - 未协商能力（tool namespace / compaction / experimental api）在使用点显式
//!   fail-closed，绝不「收到 200 + JSON 即视为兼容」。
//!
//! 边界红线：本 crate 只做协议翻译，不读取 Provider 凭证、不构造第二个 Core、
//! 不绕过 app-service / policy、不混入 GUI Connection Protocol frame。

pub mod adapter;
pub mod host;
pub mod map;
pub mod wire;

pub use adapter::{
    CodexAppServerAdapter, CodexAppServerAdapterFactory, CwdResolver, NegotiatedCodexAdapter,
    SessionResolver, CAP_COMPACTION, CAP_EXPERIMENTAL_API, CAP_TOOL_NAMESPACE,
    DEFAULT_SUPPORTED_CAPABILITIES,
};
pub use host::{CodexAppServerHost, CoreDispatcher, HandshakeState, RuntimeIdentity};
pub use map::ThreadLineage;
pub use wire::{
    JsonRpcError, JsonRpcMessage, ERROR_ALREADY_INITIALIZED, ERROR_NOT_INITIALIZED,
    ERROR_OVERLOADED, ERROR_OVERLOADED_MESSAGE,
};

/// 线协议名（线上不出现在 message，仅用于 capability negotiation 与 registry）。
pub const PROTOCOL_NAME: &str = "codex-app-server";

/// 目标线协议 schema 版本字符串（官方 schema 为生成产物，按版本固定）。
pub const PROTOCOL_VERSION: &str = "2026-08";

/// 握手期间 host 自报的 `ClientInfo.name`（写进 canonical `ActorIdentity::Automation`）。
pub const HOST_AGENT_NAME: &str = "pawork-codex-app-server";

/// 握手期间 host 自报的 `ClientInfo.version`。
pub const HOST_AGENT_VERSION: &str = "0.0.0";

/// 生成当前 Unix 毫秒时间戳（agent-domain 的 `Timestamp` 无 `now()` 构造器）。
pub(crate) fn now_timestamp() -> agent_domain::Timestamp {
    use std::time::{SystemTime, UNIX_EPOCH};
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0);
    agent_domain::Timestamp::from_unix_millis(millis)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_baseline_constants_are_non_empty() {
        assert!(!PROTOCOL_NAME.is_empty());
        assert_eq!(PROTOCOL_VERSION, "2026-08");
        assert!(!HOST_AGENT_NAME.is_empty());
        assert!(now_timestamp().as_unix_millis() > 0);
    }
}
