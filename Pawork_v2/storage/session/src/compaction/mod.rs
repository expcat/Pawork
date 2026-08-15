//! Pawork 自动 / 手动 Compaction 引擎（feature `compaction`，默认关闭）。
//!
//! 职责：在上下文超限或手动触发时生成版本化的 [`CompactionSnapshot`]，应用保留策略
//! （最近 N 轮、未解决任务、用户约束、修改文件、待处理 / 失败 tool call），并在压缩前
//! Fork 出可恢复 branch。本模块只产出压缩决策与快照结构，不改写历史、不向事件流
//! 追加事件；真正的摘要文本生成、`CompactionStarted` / `CompactionCompleted`
//! 事件化与上下文重建由调用方（engine 侧）完成。
//!
//! 依赖倒置：token 估算统一经 [`TokenEstimator`] trait 注入，由本 crate 定义、
//! engine 侧实现；本 crate（含默认 feature 集）不依赖 pawork-engine /
//! context-engine 链。
//!
//! 关键类型：
//! - [`CompactionEngine`] / [`CompactionResult`]：自动 / 手动压缩统一入口。
//! - [`CompactionSnapshot`] / [`SnapshotVersion`]：版本化压缩摘要（P5-5，serde 形状冻结）。
//! - [`RetentionPolicy`] / [`apply`] / [`RetentionDecision`]：保留策略（P5-6）。
//! - [`TokenEstimator`]：token 估算注入端口。
//!
//! 由 V1 `compaction-engine` 迁入（archive/M3 关键动作 2）。

mod engine;
mod retention;
mod snapshot;

use pawork_domain::Message;
use thiserror::Error;

pub use engine::{CompactionEngine, CompactionReason, CompactionResult};
pub use retention::{
    apply, ModifiedFile, RetentionConstraint, RetentionDecision, RetentionInputs, RetentionMessage,
    RetentionPolicy, RetentionReasoning, RetentionTask, RetentionToolCall, ToolCallRetentionState,
    DEFAULT_RETAINED_REASONING_ITEMS, DEFAULT_RETAINED_TURNS,
};
pub use snapshot::{CompactionSnapshot, SnapshotVersion, CURRENT_SNAPSHOT_VERSION};

/// Token 估算端口：由 session 定义、调用方实现并显式注入。
///
/// 这是 V1 `context-engine::TokenEstimator` 的依赖倒置收窄——compaction 只需要
/// 文本与消息两级估算；启发式 / tokenizer 实现留在 engine 侧，避免 session
/// 反向依赖引擎链。实现必须无 IO、线程安全（`Send + Sync`）且确定性。
pub trait TokenEstimator: Send + Sync {
    /// 估算纯文本 token 数。
    fn count_text(&self, text: &str) -> u64;

    /// 估算单条消息（角色 + 内容 + 结构开销）。
    fn count_message(&self, message: &Message) -> u64;
}

/// Compaction 引擎错误。
#[derive(Debug, Error)]
pub enum CompactionError {
    /// Event Store 调用失败。
    #[error(transparent)]
    Store(#[from] crate::SessionStoreError),
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
