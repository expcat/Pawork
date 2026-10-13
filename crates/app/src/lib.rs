//! 应用装配门面：配置发现与凭证链 → provider 装配 → 内建 / MCP 工具 →
//! 事件化 `run_session`（persist-first），并把 `AppCore` 适配为 GUI 业务
//! 宿主（gui_host）与本机模型网关后端（gateway_backend）。
//!
//! 不按 Provider 名称分支；协议来自 `extra.provider_protocols` 与默认表。
//! 落库 persist-first，再推渲染 sink。

mod app_core;
mod approval;
mod auth;
mod channels;
mod checkpoint;
mod control;
mod data_dir;
#[cfg(any(test, feature = "ui-fixture"))]
#[doc(hidden)]
pub mod devfixture;
mod diff;
mod extensions;
pub mod gateway_backend;
#[cfg(test)]
mod gateway_tests;
mod goal_host;
mod gui_host;
mod hub;
mod idempotency;
mod ids;
mod import_host;
mod instance_lock;
mod loop_ctx;
mod orchestration_host;
mod persist;
mod plan_host;
mod protected;
mod protocol;
mod provider_assembly;
mod provider_quota;
mod services;
mod subagents;
mod tasks_host;
#[cfg(test)]
mod testsupport;

pub use app_core::{
    session_title_from_text, AppCore, AppError, AppLoadOptions, ComputerNativeMode,
};
pub(crate) use app_core::{unbound_workspace, RETAINED_MESSAGES};

pub use approval::{
    parse_approval_mode, ApprovalAsk, ApprovalPromptHost, DenyAllApprovals, GuiApprovalHost,
};
pub(crate) use approval::{ApprovalResolve, PendingToolApproval};
pub use auth::{AuthChannelStatus, AuthSource, OAuthLogin};
pub use channels::{ChannelKind, FirstPartyChannel, FIRST_PARTY_CHANNELS};
pub use checkpoint::{CheckpointSummary, RollbackOutcome};
pub use control::{LedgerTotals, QuotaWindowLine, SessionUsageLine, UsageOverview};
pub use data_dir::{
    consume_data_dir_outcome, default_data_dir, default_data_dir_outcome, instance_dir,
    normalize_instance, session_db_path_for, DataDirOutcome, DEFAULT_INSTANCE,
};
pub use diff::{paginate_diff, render_session_diff, GitDiffHeader, SessionDiff};
pub use extensions::McpServerStatus;
pub(crate) use goal_host::GoalSnapshot;
pub use gui_host::{GuiHostAdapter, GuiRunRegistry};
pub(crate) use hub::{EventHub, HubError, DEFAULT_HUB_CAPACITY};
pub(crate) use idempotency::{
    should_cache, IdempotencyCheck, IdempotencyError, IdempotencyStore,
    DEFAULT_IDEMPOTENCY_CAPACITY,
};
pub use import_host::{
    parse_session_source, CompatImportItemView, CompatImportPreview, CompatImportReport,
    CompatTool, SessionImportFormat, SessionImportOutcome,
};
pub use instance_lock::{active_instance_host_pids, InstanceRole};
pub use orchestration_host::{MultiAgentDemoOptions, MultiAgentDemoReport};
pub use pawork_git::{DiffFile, DiffPage};
pub use pawork_policy::{ApprovalMode, RiskLevel};
pub use pawork_storage::session::{SessionExport, SessionRecord};
pub use pawork_workflow::plan::PlanSnapshot;
pub use pawork_workflow::task::TaskSnapshot;
pub use pawork_workspace::import::{LocalSessionFile, LocalSessionSource};
pub use plan_host::review_status_label;
pub use protocol::{AdapterProtocol, ProtocolError};
pub use services::extension::{base64_encode, image_media_type};
pub use tasks_host::parse_task_kind;
