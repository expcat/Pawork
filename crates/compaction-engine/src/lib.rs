//! Pawork 自动 / 手动 Compaction 引擎。
//!
//! 职责：在上下文超限或手动触发时生成版本化的 [`CompactionSnapshot`]，应用保留策略
//! （最近 N 轮、未解决任务、用户约束、修改文件、待处理 / 失败 tool call），并在压缩前
//! Fork 出可恢复 branch。本 crate 只产出压缩决策与快照结构；真正的摘要文本生成与
//! 上下文重建由调用方（`agent-engine` / `context-engine`）完成。
//!
//! 关键类型：
//! - [`CompactionEngine`] / [`CompactionResult`]：自动 / 手动压缩统一入口。
//! - [`CompactionSnapshot`] / [`SnapshotVersion`]：版本化压缩摘要（P5-5）。
//! - [`RetentionPolicy`] / [`apply`] / [`RetentionDecision`]：保留策略（P5-6）。
//!
//! 详见 `docs/features/context.md`。

mod engine;
mod retention;
mod snapshot;

use thiserror::Error;

pub use engine::{CompactionEngine, CompactionReason, CompactionResult};
pub use retention::{
    apply, ModifiedFile, RetentionConstraint, RetentionDecision, RetentionInputs, RetentionMessage,
    RetentionPolicy, RetentionReasoning, RetentionTask, RetentionToolCall, ToolCallRetentionState,
    DEFAULT_RETAINED_REASONING_ITEMS, DEFAULT_RETAINED_TURNS,
};
pub use snapshot::{CompactionSnapshot, SnapshotVersion, CURRENT_SNAPSHOT_VERSION};

/// Compaction 引擎错误。
#[derive(Debug, Error)]
pub enum CompactionError {
    /// Event Store 调用失败。
    #[error(transparent)]
    Store(#[from] session_store::SessionStoreError),
    /// 待压缩分支没有任何事件，无法压缩。
    #[error("nothing to compact: session {session_id} branch {branch_id} has no events")]
    NothingToCompact {
        session_id: String,
        branch_id: String,
    },
    /// 快照版本不被支持。
    #[error("unsupported snapshot version: found {found}, supported {supported}")]
    UnsupportedSnapshotVersion { found: u32, supported: u32 },
}
