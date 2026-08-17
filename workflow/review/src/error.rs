//! Review Engine 错误类型。

use pawork_domain::ReviewResolution;
use thiserror::Error;

/// Review Engine 的错误。
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ReviewError {
    /// 状态锁中毒（不可恢复的内部错误）。
    #[error("review state poisoned")]
    StatePoisoned,
    /// 会话不存在。
    #[error("unknown review session: {0}")]
    UnknownSession(String),
    /// finding 不存在。
    #[error("unknown review finding: {0}")]
    UnknownFinding(String),
    /// 重复的 session / finding id（重放序列不合法）。
    #[error("duplicate {kind}: {id}")]
    Duplicate { kind: &'static str, id: String },
    /// 非法 resolution 转移。
    #[error("invalid resolution transition: {from:?} -> {to:?}")]
    InvalidTransition {
        from: ReviewResolution,
        to: ReviewResolution,
    },
    /// 锚点不合法（行越界 / 空路径等）。
    #[error("invalid anchor `{anchor}`: {reason}")]
    InvalidAnchor { anchor: String, reason: String },
    /// 锚点路径逃逸 workspace 根（拒绝，不落盘访问）。
    #[error("anchor file `{0}` escapes workspace root")]
    TraversalDenied(String),
    /// 锚点文件不可读（只读访问；引擎不写任何文件）。
    #[error("anchor file `{0}` unavailable: {1}")]
    FileUnavailable(String, String),
    /// 建议补丁无法通过 dry-run 校验。
    #[error("invalid suggested patch: {0}")]
    InvalidPatch(String),
    /// 内存试应用时 context 行不匹配。
    #[error("patch context mismatch at {position}: expected `{expected}`, found `{found}`")]
    PatchContextMismatch {
        position: String,
        expected: String,
        found: String,
    },
    /// ForgeAdapter 错误。
    #[error("forge adapter error: {0}")]
    Forge(String),
}
