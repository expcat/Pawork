//! # Remote Control Adapter（P17-12）
//!
//! Mobile / Remote Control 受限通道：手机等远端设备经受限协议观察与控制 pawork
//! Host。安全模型：
//!
//! - **受限协议**：查询只映射 `SessionGet` / `RunStatus` / 计划状态查询
//!   （[`RemoteQuery::PlanStatus`]）；命令只映射 `RunStart` / `RunCancel` /
//!   `ToolApprove`。文件写、`RunTool`、Provider 直连、终端、会话/工作区
//!   变更、批量内容读取等一律经 [`gate`] 穷举分类 → 显式拒绝 +
//!   [`audit`] 审计，绝不放行到 Core。
//! - **配对/认证**：配对码与设备凭证仅以加盐 SHA-256 摘要存储，可吊销；
//!   明文只在签发时出现一次，不落日志、不进审计、不出现在 Debug。
//! - **通知推送**：有界环形缓冲 + event_id 去重 + 按序 replay；重放窗口被
//!   淘汰或推送背压丢弃时以显式 gap 帧告知客户端。
//! - **Core 单一事实源**：所有读写经 [`app_service::AppService`] canonical
//!   信封；计划状态在 Core 未暴露专用查询时返回显式可用性标记，不伪造状态。
//! - **承载无关**：服务端接受任意 [`transport_api::GuiConnection`] 实现；
//!   transport-remote（TCP + TLS 1.3）承载集成证据见
//!   `tests/transport_remote_carrier.rs`（仅 dev-dependency，不改其源码）。
//!
//! 依赖方向（plan/P17-12）：`core-api` → `remote-control-adapter` → transport
//! 承载抽象。本 crate 不依赖 gui-protocol / agent-engine / provider-* /
//! session-store / app-database。

mod audit;
mod gate;
mod notify;
mod pairing;
mod service;
mod wire;

pub use audit::{AuditEvent, AuditLog, AuditRecord, DEFAULT_AUDIT_CAPACITY};
pub use gate::{
    classify_command, classify_query, command_operation, query_operation, Verdict,
    DENY_CONTENT_READ, DENY_FILE_WRITE, DENY_HOST_MUTATION, DENY_NOT_EXPOSED,
    DENY_PROVIDER_DIRECT_ACCESS, DENY_SESSION_MUTATION, DENY_TOOL_EXECUTION,
    DENY_WORKSPACE_MUTATION,
};
pub use notify::{
    Notification, NotificationLog, NotificationPayload, ReplayGap, DEFAULT_DEDUP_CAPACITY,
    DEFAULT_NOTIFICATION_CAPACITY,
};
pub use pairing::{Activation, IssuedPairing, PairingConfig, PairingError, PairingRegistry};
pub use service::{
    CloseReason, ConnectionSummary, RemoteControlConfig, RemoteControlService, SUBJECT,
};
pub use wire::{ClientFrame, RemoteCommand, RemoteQuery, ServerFrame};

/// 当前 Unix 毫秒时间戳。
pub(crate) fn now_unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| u64::try_from(duration.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}
