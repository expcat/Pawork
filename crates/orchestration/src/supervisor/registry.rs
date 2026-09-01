//! 注册表：worker 条目、状态查询、事件日志与终态取守卫。

use pawork_control_plane::credential::LeaseGuard;
use pawork_domain::{AgentId, CancellationToken, ModelId};

use crate::budget::WorkerBudgetController;
use crate::identity::AgentInstance;
use crate::lifecycle::{OrchestrationEvent, WorkerState, WorkerStateMachine, WorkerTransition};
use crate::worktree::WorktreeGuard;

use super::budget_gate::FlushTicket;
use super::{now_ms, AgentSupervisor, SupervisorError};

pub struct WorkerEntry {
    /// 不可变身份。
    pub instance: AgentInstance,
    /// 生命周期状态机。
    pub state: WorkerStateMachine,
    /// 持有的 credential lease 守卫（未申请时为 `None`）。
    pub lease: Option<LeaseGuard>,
    /// 分配的 worktree 守卫（未分配时为 `None`）。
    pub worktree: Option<WorktreeGuard>,
    /// spawn 请求携带的模型（用于 ledger 归属）。
    pub model: Option<ModelId>,
}

/// complete / fail 锁内取出的守卫与归属。
pub(crate) struct TerminalTake {
    pub lease: Option<LeaseGuard>,
    pub parent: Option<AgentId>,
    pub instance: AgentInstance,
    pub controller: Option<WorkerBudgetController>,
    pub worktree: Option<WorktreeGuard>,
    pub model: Option<ModelId>,
    pub ticket: Option<FlushTicket>,
}

impl AgentSupervisor {
    /// Starting → Running，发出 `WorkerRunning`。
    pub async fn start_worker(&self, agent_id: &AgentId) -> Result<(), SupervisorError> {
        let mut workers = self
            .workers
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let entry = workers
            .get_mut(agent_id)
            .ok_or_else(|| SupervisorError::UnknownAgent(agent_id.clone()))?;
        entry
            .state
            .apply(WorkerTransition::BeginRunning)
            .map_err(SupervisorError::IllegalLifecycle)?;
        drop(workers);
        self.emit(OrchestrationEvent::WorkerRunning {
            agent_id: agent_id.clone(),
            at_ms: now_ms(),
        });
        Ok(())
    }

    /// 查询 agent 的取消令牌。
    pub fn cancel_token(&self, agent_id: &AgentId) -> Option<CancellationToken> {
        self.cancel_tokens
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .get(agent_id)
            .cloned()
    }

    /// 查询 agent 当前 worker 状态。
    pub fn state(&self, agent_id: &AgentId) -> Option<WorkerState> {
        self.workers
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .get(agent_id)
            .map(|entry| entry.state.state())
    }

    /// 事件快照（供重放 / 恢复）。
    pub fn events(&self) -> Vec<OrchestrationEvent> {
        self.event_log
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .clone()
    }

    /// 追加一条事件。
    pub(crate) fn emit(&self, event: OrchestrationEvent) {
        self.event_log
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .push(event);
    }

    /// 从父的 children 列表中移除（幂等）。
    pub(crate) fn remove_child(&self, parent: Option<&AgentId>, child: &AgentId) {
        let Some(parent) = parent else {
            return;
        };
        if let Some(kids) = self
            .children
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .get_mut(parent)
        {
            kids.retain(|id| id != child);
        }
    }

    /// 测试辅助：活动 worker 与在途并发预约计数（`tenant = None` 时全局）。
    #[cfg(test)]
    pub(crate) fn active_worker_count(&self, tenant: Option<&pawork_domain::TenantId>) -> u64 {
        let reservations = self
            .reservations
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let workers = self
            .workers
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let reserved = reservations
            .values()
            .filter(|tenant_id| tenant.is_none_or(|wanted| *tenant_id == wanted))
            .count() as u64;
        let active = workers
            .values()
            .filter(|entry| {
                entry.state.state().is_active()
                    && tenant.is_none_or(|tenant| entry.instance.tenant_id == *tenant)
            })
            .count() as u64;
        reserved + active
    }

    /// 锁内状态机转换并取出 lease / worktree / budget 守卫（complete / fail 共用）。
    pub(crate) fn apply_terminal_and_take(
        &self,
        agent_id: &AgentId,
        transition: WorkerTransition,
    ) -> Result<TerminalTake, SupervisorError> {
        let mut workers = self
            .workers
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let entry = workers
            .get_mut(agent_id)
            .ok_or_else(|| SupervisorError::UnknownAgent(agent_id.clone()))?;
        entry
            .state
            .apply(transition)
            .map_err(SupervisorError::IllegalLifecycle)?;
        let instance = entry.instance.clone();
        let model = entry.model.clone();
        let controller = self
            .budget
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .remove(agent_id);
        let ticket = controller
            .is_some()
            .then(|| FlushTicket::issue(&self.flush_in_flight, agent_id));
        Ok(TerminalTake {
            lease: entry.lease.take(),
            parent: entry.instance.parent_id.clone(),
            instance,
            controller,
            worktree: entry.worktree.take(),
            model,
            ticket,
        })
    }
}
