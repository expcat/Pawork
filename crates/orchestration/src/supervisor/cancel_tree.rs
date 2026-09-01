//! 取消树：递归取消子孙并以 Cancelled 释放 lease（不惩罚账号健康）。

use pawork_control_plane::credential::LeaseOutcome;
use pawork_domain::{AgentId, ModelId, ProviderId};

use crate::lifecycle::{OrchestrationEvent, WorkerTransition};
use crate::task_graph::TaskId;

use super::budget_gate::FlushTicket;
use super::{now_ms, AgentSupervisor, SupervisorError};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CancelTreeReceipt {
    /// 本次实际被取消（进入终态 Cancelled）的 agent 列表。
    pub cancelled_ids: Vec<AgentId>,
    /// 本次实际释放的 lease 数量。
    pub leases_released: u64,
}

impl AgentSupervisor {
    /// 取消树：取消 `agent_id` 及其全部后代（BFS 遍历 children 图）。
    ///
    /// 每个节点：取消令牌 → `Cancelling`（`WorkerCancelling`）→ `Cancelled`
    /// （`WorkerCancelled`）→ 以 [`LeaseOutcome::Cancelled`] 幂等释放 lease。
    /// worktree 显式释放（best-effort）；TaskGraph 推进为 Cancelled 并发出
    /// `TaskCancelled`。终态节点跳过；重复调用是幂等的（第二次不再取消
    /// 任何节点、不重复释放）。
    ///
    /// 取消总是完成（所有非终态节点进入 `Cancelled`）；若任一节点的终态用量
    /// flush 失败，返回 [`SupervisorError::CancelTreeFlushPending`]——错误携带
    /// 完整 receipt 与待重试的 agent 列表，调用方可经
    /// [`AgentSupervisor::flush_usage`] 逐个重试，不吞掉 pending。
    pub async fn cancel_tree(
        &self,
        agent_id: &AgentId,
    ) -> Result<CancelTreeReceipt, SupervisorError> {
        {
            let workers = self
                .workers
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            if !workers.contains_key(agent_id) {
                return Err(SupervisorError::UnknownAgent(agent_id.clone()));
            }
        }

        let mut queue = vec![agent_id.clone()];
        let mut nodes = Vec::new();
        while let Some(id) = queue.pop() {
            nodes.push(id.clone());
            let kids = self
                .children
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .get(&id)
                .cloned()
                .unwrap_or_default();
            queue.extend(kids);
        }

        let mut cancelled_ids = Vec::new();
        let mut leases_released = 0u64;
        let mut flush_pending = Vec::new();
        for id in nodes {
            if let Some(token) = self.cancel_token(&id) {
                token.cancel();
            }
            let (cancelled, lease, worktree, instance, model, controller, ticket) = {
                let mut workers = self
                    .workers
                    .lock()
                    .unwrap_or_else(|poison| poison.into_inner());
                let Some(entry) = workers.get_mut(&id) else {
                    continue;
                };
                if entry.state.state().is_terminal() {
                    (false, None, None, None, None, None, None)
                } else {
                    let _ = entry.state.apply(WorkerTransition::BeginCancel);
                    let _ = entry.state.apply(WorkerTransition::Cancel);
                    let controller = self
                        .budget
                        .lock()
                        .unwrap_or_else(|poison| poison.into_inner())
                        .remove(&id);
                    // 与 controller 移除同步登记 flush 在途标记（见 flush_usage）。
                    let ticket = controller
                        .is_some()
                        .then(|| FlushTicket::issue(&self.flush_in_flight, &id));
                    (
                        true,
                        entry.lease.take(),
                        entry.worktree.take(),
                        Some(entry.instance.clone()),
                        entry.model.clone(),
                        controller,
                        ticket,
                    )
                }
            };
            if !cancelled {
                continue;
            }
            self.emit(OrchestrationEvent::WorkerCancelling {
                agent_id: id.clone(),
                at_ms: now_ms(),
            });
            self.emit(OrchestrationEvent::WorkerCancelled {
                agent_id: id.clone(),
                at_ms: now_ms(),
            });
            if let Some(guard) = worktree {
                if let Err(error) = guard.release().await {
                    tracing::warn!(%id, %error, "failed to release worktree on cancel");
                }
            }
            if let Some(graph) = &self.task_graph {
                let task_id = TaskId::new(id.as_str());
                let _ = graph.cancel(&task_id);
                self.emit(OrchestrationEvent::TaskCancelled { task_id });
            }
            // 真实归属：account / provider 取自 lease（释放前读取），model 取自 spawn 请求。
            let instance = instance.unwrap();
            let (account_id, provider_id) = lease
                .as_ref()
                .and_then(|guard| guard.lease())
                .map(|l| (l.account_id.as_str().to_string(), l.provider_id.clone()))
                .unwrap_or_else(|| ("local/default".to_string(), ProviderId::new("local")));
            let model_id = model.unwrap_or_else(|| ModelId::new("unknown"));
            if let Some(mut guard) = lease {
                *guard.outcome_mut() = LeaseOutcome::Cancelled;
                // Drop 触发同步幂等释放；Cancelled 只累加取消计数，
                // 不累加连续失败（不惩罚账号健康）。
                drop(guard);
                leases_released += 1;
            }
            // 终态前 flush（与 complete/fail 一致）；失败保留 controller 与归属，
            // 可经 `flush_usage` 重试。取消本身已完成：整体以
            // `CancelTreeFlushPending` 回报（携带 receipt 与待重试 agent 列表），
            // 不再吞掉 pending。
            if self
                .flush_terminal_usage(
                    &id,
                    &instance,
                    account_id,
                    provider_id,
                    model_id,
                    controller,
                )
                .await
                .is_err()
            {
                flush_pending.push(id.clone());
            }
            drop(ticket);
            cancelled_ids.push(id);
        }
        let receipt = CancelTreeReceipt {
            cancelled_ids,
            leases_released,
        };
        if flush_pending.is_empty() {
            Ok(receipt)
        } else {
            Err(SupervisorError::CancelTreeFlushPending {
                receipt,
                pending: flush_pending,
            })
        }
    }
}
