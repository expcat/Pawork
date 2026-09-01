//! Supervisor 侧用量闸门：record / flush / 终态 flush。

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use pawork_domain::{AgentId, ModelId, ProviderId};

use crate::budget::{
    LedgerContext, WorkerBudgetController, DIM_COST_MICROS, DIM_INPUT_TOKENS, DIM_OUTPUT_TOKENS,
};
use crate::identity::AgentInstance;
use crate::lifecycle::OrchestrationEvent;

use super::{AgentSupervisor, SupervisorError};

/// 用量 flush 在途标记（RAII）：进入 flush 前登记，结束时（含 future 被
/// drop / 取消）自动清除。仅在同步段操作，不跨 await 持有任何锁。
pub(crate) struct FlushTicket {
    inflight: Arc<Mutex<BTreeSet<AgentId>>>,
    agent_id: AgentId,
}

impl FlushTicket {
    /// 登记 `agent_id` 的在途标记并返回票据。
    pub(crate) fn issue(inflight: &Arc<Mutex<BTreeSet<AgentId>>>, agent_id: &AgentId) -> Self {
        inflight
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .insert(agent_id.clone());
        Self {
            inflight: Arc::clone(inflight),
            agent_id: agent_id.clone(),
        }
    }
}

impl Drop for FlushTicket {
    fn drop(&mut self) {
        self.inflight
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .remove(&self.agent_id);
    }
}

impl AgentSupervisor {
    /// 记录一次用量并检查预算（B1）：对硬超限维度发出 `BudgetExceeded`。
    ///
    /// 用量经该 worker 的 [`WorkerBudgetController`] 累加；`check()` 报告中
    /// 「新进入硬超限且尚未发出过事件」的维度经 `diff_hard_exceeded` 去重后
    /// 以当前用量与对应上限发出一个 `BudgetExceeded` 事件（同一维度持续
    /// 超限只告警一次；用量回落到上限以下后该维度被「忘记」，恢复后可再告警）。
    ///
    /// worker 进入终态（Completed / Cancelled / Failed）后拒绝再记录用量：
    /// 终态用量已由 `complete` / `fail` / `cancel_tree` flush 到 ledger，
    /// 此后新增 record 会破坏「终态后不再变更用量」的不变式，因此返回
    /// [`SupervisorError::WorkerTerminal`]。终态 flush 若失败保留了 controller，
    /// 调用方应经 [`AgentSupervisor::flush_usage`] 重试。
    pub async fn record_usage(
        &self,
        agent_id: &AgentId,
        input: u64,
        output: u64,
        cost_micros: u64,
    ) -> Result<(), SupervisorError> {
        // 终态拒绝：worker 已终态时不得再累加用量（避免与终态 flush 竞争/重复）。
        let terminal = self
            .workers
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .get(agent_id)
            .map(|entry| entry.state.state().is_terminal())
            .unwrap_or(false);
        if terminal {
            return Err(SupervisorError::WorkerTerminal(agent_id.clone()));
        }
        // 直接累加进注册表内的控制器（克隆会丢失写入）。
        let mut controllers = self
            .budget
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let controller = controllers
            .get_mut(agent_id)
            .ok_or_else(|| SupervisorError::UnknownAgent(agent_id.clone()))?;
        controller.record_tokens(input, output);
        controller.record_cost(cost_micros);
        let report = controller.check();
        // 持续超限去重：仅对「新进入硬超限」的维度发事件；恢复后可再告警。
        let newly_exceeded = controller.diff_hard_exceeded(&report);
        let (used_input, used_output, used_cost) = controller.usage();
        let limits = controller.limits().clone();
        for dimension in &newly_exceeded {
            let (used, limit) = match dimension.as_str() {
                DIM_INPUT_TOKENS => (used_input, limits.max_input_tokens.unwrap_or(0)),
                DIM_OUTPUT_TOKENS => (used_output, limits.max_output_tokens.unwrap_or(0)),
                DIM_COST_MICROS => (used_cost, limits.max_cost_micros.unwrap_or(0)),
                _ => continue,
            };
            self.emit(OrchestrationEvent::BudgetExceeded {
                agent_id: agent_id.clone(),
                dimension: dimension.clone(),
                used,
                limit,
            });
        }
        Ok(())
    }

    /// 显式重试终态 worker 的用量 flush。
    ///
    /// 仅允许终态 worker：活动 worker 误调用返回 [`SupervisorError::FlushNotTerminal`]，
    /// 且不会移除 / 丢弃其 controller。controller 与归属 ctx 必须成对存在：
    /// 不一致（controller 在而 ctx 缺失）时保留 controller 并返回
    /// [`SupervisorError::FlushContextMissing`]，不吞 pending。
    ///
    /// 并发安全：认领（校验在途标记、移除 controller / ctx）在同一临界区完成，
    /// 认领成功后登记在途标记；flush 在途期间其他调用方收到
    /// [`SupervisorError::UsageFlushPending`] 而非假成功。提交成功后才丢弃
    /// controller / ctx；失败时原样放回（放回先于在途标记清除），可重试。
    /// 账本写入由 controller 内部提交游标串行化，重试按相同 record 幂等重放，
    /// 不重复计账。controller 不存在时为空操作（无用量或已 flush）。
    pub async fn flush_usage(&self, agent_id: &AgentId) -> Result<(), SupervisorError> {
        // 仅允许终态：活动 worker 误调用直接拒绝，不触碰 budget / flush_ctx。
        let terminal = self
            .workers
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .get(agent_id)
            .map(|entry| entry.state.state().is_terminal())
            .ok_or_else(|| SupervisorError::UnknownAgent(agent_id.clone()))?;
        if !terminal {
            return Err(SupervisorError::FlushNotTerminal(agent_id.clone()));
        }
        // 原子认领：controller 与 ctx 必须成对存在；认领成功即登记在途标记，
        // 并发 flush（终态路径或其他 flush_usage）期间本调用返回
        // `UsageFlushPending`，避免假成功。
        let (controller, ctx, _ticket) = {
            let mut budget = self
                .budget
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            let mut flush_ctx = self
                .flush_ctx
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            let mut inflight = self
                .flush_in_flight
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            if inflight.contains(agent_id) {
                return Err(SupervisorError::UsageFlushPending(agent_id.clone()));
            }
            let controller = budget.remove(agent_id);
            let ctx = flush_ctx.remove(agent_id);
            match (controller, ctx) {
                (None, None) => (None, None, None),
                (Some(controller), Some(ctx)) => {
                    inflight.insert(agent_id.clone());
                    (
                        Some(controller),
                        Some(ctx),
                        Some(FlushTicket {
                            inflight: Arc::clone(&self.flush_in_flight),
                            agent_id: agent_id.clone(),
                        }),
                    )
                }
                (Some(controller), None) => {
                    // 不一致：controller 在而 ctx 缺失 → 保留 controller，不吞 pending。
                    budget.insert(agent_id.clone(), controller);
                    return Err(SupervisorError::FlushContextMissing(agent_id.clone()));
                }
                (None, Some(_ctx)) => (None, None, None),
            }
        };
        let Some(controller) = controller else {
            // 无可 flush：已提交或从未有 pending（残留 ctx 已随认领丢弃）。
            return Ok(());
        };
        let ctx = ctx.expect("controller 与 ctx 成对认领");
        match controller.flush_to_ledger(self.ledger.as_ref(), &ctx).await {
            Ok(()) => Ok(()),
            Err(error) => {
                // 失败：controller 与 ctx 放回表内（在途标记由票据 Drop 清除，
                // 且放回先于票据清除完成），等待下一次重试。
                self.budget
                    .lock()
                    .unwrap_or_else(|poison| poison.into_inner())
                    .insert(agent_id.clone(), controller);
                self.flush_ctx
                    .lock()
                    .unwrap_or_else(|poison| poison.into_inner())
                    .insert(agent_id.clone(), ctx);
                tracing::warn!(
                    %agent_id, %error,
                    "retry flush failed; controller and ctx retained"
                );
                Err(SupervisorError::UsageFlushPending(agent_id.clone()))
            }
        }
    }

    /// 终态 flush：把 controller 的累计用量写入 ledger。成功返回 `Ok`
    /// （controller 被消费）；失败时把 controller 与归属 ctx 放回 budget /
    /// flush_ctx 表，返回 [`SupervisorError::UsageFlushPending`]，调用方可经
    /// [`AgentSupervisor::flush_usage`] 重试。终态转换本身已完成，flush 失败
    /// 不回滚生命周期，仅保留用量可重试状态。
    ///
    /// 注：`std::sync::Mutex` 仅在构造 ctx 时短暂持有并立即 drop，不跨 await；
    /// ledger 调用期间不持有任何 `std::sync::Mutex`（保持无锁跨 await）。
    pub(crate) async fn flush_terminal_usage(
        &self,
        agent_id: &AgentId,
        instance: &AgentInstance,
        account_id: String,
        provider_id: ProviderId,
        model_id: ModelId,
        controller: Option<WorkerBudgetController>,
    ) -> Result<(), SupervisorError> {
        let Some(controller) = controller else {
            return Ok(());
        };
        let ctx = LedgerContext {
            credential_id: None,
            tenant_id: instance.tenant_id.clone(),
            principal_id: instance.principal_id.clone(),
            account_id,
            session_id: instance.session_id.clone(),
            agent_id: instance.agent_id.clone(),
            run_id: None,
            provider_id,
            model_id,
        };
        match controller.flush_to_ledger(self.ledger.as_ref(), &ctx).await {
            Ok(()) => Ok(()),
            Err(error) => {
                tracing::warn!(
                    %agent_id, %error,
                    "terminal usage flush failed; controller and ctx retained for retry"
                );
                self.budget
                    .lock()
                    .unwrap_or_else(|poison| poison.into_inner())
                    .insert(agent_id.clone(), controller);
                self.flush_ctx
                    .lock()
                    .unwrap_or_else(|poison| poison.into_inner())
                    .insert(agent_id.clone(), ctx);
                Err(SupervisorError::UsageFlushPending(agent_id.clone()))
            }
        }
    }
}
