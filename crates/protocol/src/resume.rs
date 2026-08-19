//! 重连 disposition 计算：给定服务端可重放历史与客户端已确认序列，决定
//! Replay / SnapshotRequired / UpToDate。

use crate::app::GlobalSequence;

use crate::ResumeDisposition;

/// 服务端当前可重放的历史范围（含两端点）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResumeContext {
    pub earliest_available: GlobalSequence,
    pub current: GlobalSequence,
}

impl ResumeContext {
    pub fn new(earliest_available: GlobalSequence, current: GlobalSequence) -> Self {
        Self {
            earliest_available,
            current,
        }
    }
}

/// 计算重连 disposition：
///
/// - `last_global_sequence == current`：客户端已追上，返回 `UpToDate`；
/// - `last_global_sequence + 1 >= earliest_available` 且 `last_global_sequence <
///   current`：缺失事件仍在可重放窗口内，返回 `Replay { from: last + 1,
///   through: current }`；
/// - 其他（客户端落后于窗口、领先于服务端，或历史为空）：返回 `SnapshotRequired`。
///
/// 函数是全函数：任何输入组合都有确定结果，不产生错误。
pub fn compute_resume_disposition(
    earliest_available: GlobalSequence,
    current: GlobalSequence,
    last_global_sequence: GlobalSequence,
) -> ResumeDisposition {
    let last = last_global_sequence.0;
    let current_sequence = current.0;

    if last == current_sequence {
        return ResumeDisposition::UpToDate {
            current_sequence: current,
        };
    }
    if last < current_sequence && last + 1 >= earliest_available.0 {
        return ResumeDisposition::Replay {
            from_sequence: GlobalSequence(last + 1),
            through_sequence: current,
        };
    }
    ResumeDisposition::SnapshotRequired {
        earliest_available_sequence: earliest_available,
    }
}
