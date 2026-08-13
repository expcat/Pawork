//! Codex App Server client adapter（P18-11）。
//!
//! 本文件为任务收尾时的**可编译骨架**：crate 已落地 `Cargo.toml`（独立 workspace
//! 根，临时 `[workspace]` 表，待主代理接线至根 workspace）与协议基线常量，但
//! adapter / wire / map / host / contract 的实现尚未开始（见任务报告「明确未完成」）。
//!
//! 协议基线（2026-08 官方稳定 schema，已核对 developers.openai.com/codex/app-server）：
//!
//! - 传输为 JSON-RPC *风格*，但线上**省略 `jsonrpc` 字段**；默认 stdio JSONL，亦可本地 socket。
//! - 握手顺序固定：`initialize`（request）→ `initialized`（notification）；未完成握手前禁止其它方法，
//!   重复 `initialize` 返回 *Already initialized*。
//! - 生命周期：`thread/start` `thread/resume` `thread/fork`（`parentThreadId` / `forkedFromId` 保留
//!   subagent 血缘）；`turn/start` `turn/steer` `turn/interrupt`；item 通知与 `turn/completed`；
//!   显式压缩 `thread/compact/start` + `contextCompaction` 通知。
//! - 审批为 server→client JSON-RPC **请求** `item/commandExecution/requestApproval`，携带
//!   `threadId` / `turnId` / `itemId`；客户端以 `{decision}` 响应。
//! - 有界 ingress：饱和时返回 `-32001 Server overloaded; retry later.`。
//! - 未协商能力（tool namespace / compaction / experimental api）在使用点显式 fail-closed，
//!   绝不「收到 200 + JSON 即视为兼容」。
//!
//! 边界红线（与 ACP/IDE adapter 一致）：本 crate 只做协议翻译，不读取 Provider 凭证、
//! 不构造第二个 Core、不绕过 app-service / policy。

/// 线协议名（线上不出现在 message，仅用于 capability negotiation 与 registry）。
pub const PROTOCOL_NAME: &str = "codex-app-server";

/// 目标线协议 schema 版本字符串（官方 schema 为生成产物，按版本固定）。
pub const PROTOCOL_VERSION: &str = "2026-08";

/// 握手期间 host 自报的 `ClientInfo.name`（写进 canonical `ActorIdentity::Automation`）。
pub const HOST_AGENT_NAME: &str = "pawork-codex-app-server";

/// 握手期间 host 自报的 `ClientInfo.version`。
pub const HOST_AGENT_VERSION: &str = "0.0.0";

/// 生成当前 Unix 毫秒时间戳（agent-domain 的 `Timestamp` 无 `now()` 构造器）。
///
/// 当前骨架未引用；预留给未实现的 adapter/wire/map/host 模块复用。
#[allow(dead_code)]
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

    /// 骨架烟雾测试：常量与时间戳构造不依赖未实现的模块。
    #[test]
    fn protocol_baseline_constants_are_non_empty() {
        assert!(!PROTOCOL_NAME.is_empty());
        assert!(!PROTOCOL_VERSION.is_empty());
        assert!(!HOST_AGENT_NAME.is_empty());
        // 仅证明时间戳构造器在未引入模块依赖时可用（Unix epoch 至今恒正）。
        assert!(now_timestamp().as_unix_millis() > 0);
    }
}
