//! 崩溃诊断（report-only）：重放事件后报告孤儿与终态，不重建可操作状态。

use std::collections::BTreeMap;

use pawork_domain::AgentId;

use crate::lifecycle::{
    replay_workers, OrchestrationEvent, WorkerState, WorkerStateMachine, WorkerTransition,
};

use super::AgentSupervisor;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecoveryReport {
    /// 重放后处于活动态、在报告中标记为 Failed 的孤儿 worker。
    ///
    /// 这只是诊断结论，**不会**写回 Supervisor 或 emit `WorkerFailed`。
    pub orphaned: Vec<AgentId>,
    /// 每个已知 worker 按报告口径推演的最终状态（全部为终态）。
    pub recovered_states: BTreeMap<AgentId, WorkerState>,
}

impl AgentSupervisor {
    /// Report-only 崩溃诊断：重放事件计算孤儿与终态。
    ///
    /// **不**重建 `WorkerEntry` / `children` / cancel token，也**不** emit
    /// `WorkerFailed`。返回的报告不能作为继续 cancel / assign / flush 的恢复态。
    pub async fn recover_report(&self, events: &[OrchestrationEvent]) -> RecoveryReport {
        let states = replay_workers(events);
        let mut orphaned = Vec::new();
        let mut recovered_states = BTreeMap::new();
        for (agent_id, state) in states {
            if state.is_terminal() {
                recovered_states.insert(agent_id.clone(), state);
                continue;
            }
            // 无存活运行时：活动态在报告中记为 Failed（不写回 Supervisor）。
            let mut machine = WorkerStateMachine::from_state(state);
            let _ = machine.apply(WorkerTransition::Fail);
            recovered_states.insert(agent_id.clone(), machine.state());
            orphaned.push(agent_id);
        }
        RecoveryReport {
            orphaned,
            recovered_states,
        }
    }

    /// 已弃用：等价于 [`Self::recover_report`]。只生成报告，不重建可操作状态。
    #[deprecated(note = "report-only; use recover_report — does not rebuild operable supervisor state")]
    pub async fn recover(&self, events: &[OrchestrationEvent]) -> RecoveryReport {
        self.recover_report(events).await
    }
}
