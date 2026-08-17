//! 成员 presence（在线 / 忙 / 空闲 / 离线）派生。
//!
//! presence **派生自** P12 worker 生命周期（[`crate::WorkerState`]），
//! 不由成员自行声明；这与 P3-1 run 状态机对齐——supervisor 是 worker 活动状态
//! 的唯一事实源，team 只把它翻译成协作层可消费的 presence 信号，供 task board
//! 分配与调度决策。

use crate::WorkerState;
use serde::{Deserialize, Serialize};

/// 成员 presence。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Presence {
    /// 在线：已就绪但当前不忙（Created / Admitted / Starting）。
    #[default]
    Online,
    /// 忙：正在运行（Running / Cancelling）。
    Busy,
    /// 空闲：等待外部输入（Waiting）。
    Idle,
    /// 离线：终态（Completed / Cancelled / Failed）。
    Offline,
}

/// 由 P12 worker 生命周期状态派生 presence。
///
/// 映射规则：
/// - `Created | Admitted | Starting` → [`Presence::Online`]（活动但未占用）
/// - `Running | Cancelling` → [`Presence::Busy`]
/// - `Waiting` → [`Presence::Idle`]
/// - `Completed | Cancelled | Failed` → [`Presence::Offline`]（终态）
pub fn derive_from_worker_state(state: WorkerState) -> Presence {
    match state {
        WorkerState::Created | WorkerState::Admitted | WorkerState::Starting => Presence::Online,
        WorkerState::Running | WorkerState::Cancelling => Presence::Busy,
        WorkerState::Waiting => Presence::Idle,
        WorkerState::Completed | WorkerState::Cancelled | WorkerState::Failed => Presence::Offline,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_each_worker_state() {
        let cases = [
            (WorkerState::Created, Presence::Online),
            (WorkerState::Admitted, Presence::Online),
            (WorkerState::Starting, Presence::Online),
            (WorkerState::Running, Presence::Busy),
            (WorkerState::Waiting, Presence::Idle),
            (WorkerState::Cancelling, Presence::Busy),
            (WorkerState::Completed, Presence::Offline),
            (WorkerState::Cancelled, Presence::Offline),
            (WorkerState::Failed, Presence::Offline),
        ];
        for (state, expected) in cases {
            assert_eq!(
                derive_from_worker_state(state),
                expected,
                "worker state {state:?}"
            );
        }
    }
}
