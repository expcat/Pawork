//! 崩溃恢复：重放事件后把无存活运行时的活动 worker 标为 Failed。

use std::collections::BTreeMap;

use pawork_domain::AgentId;

use crate::lifecycle::{replay_workers, OrchestrationEvent, WorkerState, WorkerStateMachine, WorkerTransition};

use super::AgentSupervisor;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecoveryReport {
    /// 重放后处于活动态、已被标记 Failed 的孤儿 worker。
    pub orphaned: Vec<AgentId>,
    /// 每个已知 worker 恢复后的最终状态（全部为终态）。
    pub recovered_states: BTreeMap<AgentId, WorkerState>,
}


impl AgentSupervisor {
    /// 崩溃恢复：重放事件重建状态；任何重放后仍处于活动态且无存活运行时的
    /// worker 一律标记 `Failed`（不留悬挂 worker）。终态 worker 原样保留。
    pub async fn recover(&self, events: &[OrchestrationEvent]) -> RecoveryReport {
        let states = replay_workers(events);
        let mut orphaned = Vec::new();
        let mut recovered_states = BTreeMap::new();
        for (agent_id, state) in states {
            if state.is_terminal() {
                recovered_states.insert(agent_id.clone(), state);
                continue;
            }
            // 无存活运行时：活动态 → Failed。
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

}
