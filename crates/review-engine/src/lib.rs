//! Pawork P16-8 Review Engine：行锚点评审、re-anchor 与 resolution 生命周期（Phase 16）。
//!
//! 职责：
//! - 行锚点评审意见（`file:line`）：编辑后按邻近行内容指纹 re-anchor，
//!   漂移时标 `stale` 而非静默失效；
//! - resolution 生命周期 `open → addressed → resolved | wontfix`，以
//!   `agent_domain::ReviewEvent` 持久化（经 `agent_events::AgentEvent::Review`
//!   wrapping），可关联修复引用（commit / patch / Run），形成
//!   「finding → suggestion → fix → resolution」链；
//! - SuggestedPatch 只做 dry-run（校验 / 解析 / 内存试应用），不写文件，
//!   实际应用交既有工具 + policy（checkpoint / sandbox）；
//! - 平台无关 ForgeAdapter（GitHub / GitLab / Generic 枚举 + trait）：拉取 PR
//!   context、映射平台字段为 `PRContext`；`publish_comment` 仅在用户显式调用时
//!   产生 `CommentPublished`；Review core 不含平台名称 match 分支（有测试断言）；
//! - 按 file / severity / status 聚合；导入 diff 范围生成待发布评论。
//!
//! 设计要点（event-sourcing，进程内内存实现）：
//! - [`engine::ReviewState::apply`] 纯函数折叠 canonical 事件，是重放 / 恢复的
//!   唯一入口；命令方法校验状态机后「先 apply 再返回事件」给调用方持久化；
//! - 评审引擎对工作区只读：锚点解析仅 `fs::read_to_string`，补丁仅内存 dry-run。
//!
//! 富快照字段说明：canonical 事件只承载生命周期事实（session / finding / anchor /
//! severity / body / resolution / fix_ref / forge）。`evidence` / `assignee` /
//! `suggested_patch` / `anchor_fingerprint` 属命令时富快照字段，由命令写入内存态
//! （快照持久化由 session-store 负责）；事件流保证生命周期转移的一致性。

pub mod aggregate;
pub mod anchor;
pub mod engine;
pub mod error;
pub mod forge;
pub mod model;
pub mod patch;

pub use aggregate::AggregateBy;
pub use anchor::{AnchorResolver, ReanchorOutcome, ResolvedAnchor, StaleReason};
pub use engine::{OpenFindingInput, ReviewEngine, ReviewState};
pub use error::ReviewError;
pub use forge::{ForgeAdapter, ForgeKind, GenericForgeAdapter, PrReference};
pub use model::{
    AggregateSnapshot, FindingSnapshot, GroupCount, PRComment, PRContext, PendingComment,
    PublishedCommentRecord, Resolution, ReviewFinding, ReviewSession, ReviewSessionSnapshot,
    SuggestedPatch,
};
pub use patch::{PatchReport, PatchValidator};

/// canonical 评审类型重导出（来自 `agent-domain`，只读消费）。
pub use agent_domain::{
    ReviewAnchor, ReviewEvent, ReviewFindingId, ReviewResolution, ReviewSessionId, ReviewSeverity,
};
