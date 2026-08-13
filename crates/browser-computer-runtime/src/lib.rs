//! # Pawork Browser / Computer Runtime（P17-10）
//!
//! 为 Agent 提供统一的 Browser / Computer 使用运行时。以 [`BrowserComputerCapability`]
//! facade 收敛，背后是可替换的执行后端（Local / Playwright / MCP / ProviderHosted），
//! 每个 backend 映射到 P15-1 的三执行位点之一：
//!
//! ```text
//! BrowserComputerCapability
//! ├── Local / Playwright          → ClientFunction（Core Tool Scheduler）
//! ├── MCP（按所有权）              → ClientFunction 或 ProviderExtension
//! └── ProviderHosted              → ProviderHosted（ServerToolEvent，不入本地 execute）
//! ```
//!
//! 关键不变量：
//! - 路由只按 `execution_site()`，**不**按 Provider 名分支（ADR-002）；
//! - ProviderHosted / ProviderExtension **不**进入本地 `AgentTool::execute()`，生命周期走
//!   P15-5 `ServerToolEvent`（运行期硬门 [`backend::reject_non_client_function_for_local`]）；
//! - 所有操作经 `policy-engine` 审批与审计；Core-owned 子进程经注入的 `SandboxBackend`
//!   隔离执行（未注入 fail closed；in-process / preconnected 明确不 spawn）；
//!   ProviderHosted/Extension 不进入本地 sandbox；
//! - 审计记录经 versioned durable sink 落盘（[`audit::FileAuditSink`]），可跨重启 replay；
//! - 跨 trust boundary 的降级显式、可观测、需符合 Policy，不允许隐式切换；
//! - 截图 / DOM / 大输出经 artifact 引用（ADR-018）。
//!
//! 仅定向 / Mock smoke 测试；不要求 workspace 全量门禁。

pub mod action;
pub mod artifact;
pub mod audit;
pub mod backend;
pub mod backends;
pub mod capability;
pub mod error;
pub mod policy;
pub mod process;
pub mod selector;
pub mod tool;

pub use action::{BrowserComputerAction, BrowserComputerSnapshot};
pub use artifact::{
    artifact_reference, normalize_snapshot, store_payload, DEFAULT_LARGE_PAYLOAD_BYTES,
    DOM_MEDIA_TYPE, SCREENSHOT_MEDIA_TYPE,
};
pub use audit::{AuditRecord, AuditSink, AuditSinkError, FileAuditSink, AUDIT_FORMAT_VERSION};
pub use backend::{
    reject_hosted_for_local, reject_non_client_function_for_local, BackendKind, BackendProbe,
    BackendRoute, BrowserComputerBackend, ExecutionSite, TrustBoundary,
};
pub use backends::{screenshot_event, CanonicalHostedEmitter, HostedComputerEventEmitter};
pub use backends::{
    LocalBackend, LocalDriver, McpBackend, McpDriver, McpOwnership, PlaywrightBackend,
    PlaywrightDriver, ProviderHostedBackend, StubLocalDriver, StubPlaywrightDriver,
};
pub use capability::{BrowserComputerCapability, BrowserComputerCapabilityBuilder};
pub use error::BrowserComputerError;
pub use policy::{action_capability, enforce_decision, policy_input_for, BrowserComputerAudit};
pub use process::{ProcessMode, SandboxAuthorization, SandboxGate};
pub use selector::{
    find_hosted, select_for_local, BackendSelection, ProbeAttempt, SelectionPolicy,
};
pub use tool::BrowserComputerTool;
