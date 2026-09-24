//! S11 波 D：多 Agent demo（Supervisor spawn / cancel-tree / budget-gate）。

use std::sync::Arc;

use pawork_control_plane::credential::AcquireRequest;
use pawork_control_plane::{default_principal, default_tenant};
use pawork_domain::{AgentId, ModelId, ProviderId, SessionId};
use pawork_orchestration::{
    AgentSupervisor, OrchestrationEvent, SpawnRequest, SupervisorConfig, WorkerBudgetLimits,
};
use serde::Serialize;

use crate::AppError;

#[derive(Clone, Debug, Default)]
pub struct MultiAgentDemoOptions {
    pub cancel: bool,
    pub budget_input_tokens: Option<u64>,
}

#[derive(Clone, Debug, Serialize)]
pub struct MultiAgentDemoReport {
    pub parent_id: String,
    pub workers: Vec<String>,
    pub cancelled: Vec<String>,
    pub budget_exceeded: bool,
    pub event_kinds: Vec<String>,
}

impl crate::AppCore {
    pub async fn run_multi_agent_demo(
        &self,
        options: MultiAgentDemoOptions,
    ) -> Result<MultiAgentDemoReport, AppError> {
        let supervisor = AgentSupervisor::new(
            Arc::clone(&self.usage.control.pool),
            Arc::clone(&self.usage.control.policy),
            Arc::clone(&self.usage.control.ledger),
            SupervisorConfig::default(),
        );
        let session_id = SessionId::new("ses-s11-demo");
        let tenant_id = default_tenant();
        let principal_id = default_principal();
        let parent = supervisor
            .spawn(SpawnRequest {
                tenant_id: tenant_id.clone(),
                principal_id: principal_id.clone(),
                parent_id: None,
                session_id: session_id.clone(),
                worktree_path: None,
                budget: None,
                model: None,
                acquire: None,
                task_deps: Vec::new(),
                task_description: Some("s11-demo-parent".into()),
                task_max_retries: None,
            })
            .await
            .map_err(|error| AppError::Orchestration(error.to_string()))?;

        let left = spawn_worker(
            &supervisor,
            &parent,
            &session_id,
            &tenant_id,
            &principal_id,
            "glm-coding",
            "glm-4.7",
            "s11-demo-glm",
            options.budget_input_tokens,
        )
        .await?;
        let right = spawn_worker(
            &supervisor,
            &parent,
            &session_id,
            &tenant_id,
            &principal_id,
            "opencode-go",
            "deepseek-v4-flash",
            "s11-demo-opencode",
            options.budget_input_tokens,
        )
        .await?;

        for worker in [&left, &right] {
            if let Some(limit) = options.budget_input_tokens {
                supervisor
                    .record_usage(worker, limit.saturating_add(1), 0, 0)
                    .await
                    .map_err(|error| AppError::Orchestration(error.to_string()))?;
            }
        }

        // R-16：demo 登记的 Agent 任务持有 ID 并在终态配对收口，
        // 不留无执行体的悬空任务（任务无真实执行体令牌，挂独立令牌）。
        let mut worker_tasks = Vec::new();
        for _worker in [&left, &right] {
            match self.tasks_start_agent(None, &pawork_domain::CancellationToken::new()) {
                Ok(task_id) => worker_tasks.push(task_id),
                Err(error) => {
                    tracing::warn!(%error, "failed to start orchestration worker agent task")
                }
            }
        }

        let mut cancelled = Vec::new();
        let outcome: Result<(), AppError> = async {
            if options.cancel {
                let receipt = supervisor
                    .cancel_tree(&parent)
                    .await
                    .map_err(|error| AppError::Orchestration(error.to_string()))?;
                cancelled = receipt
                    .cancelled_ids
                    .iter()
                    .map(|id| id.as_str().to_string())
                    .collect();
            } else {
                supervisor
                    .complete(&left)
                    .await
                    .map_err(|error| AppError::Orchestration(error.to_string()))?;
                supervisor
                    .complete(&right)
                    .await
                    .map_err(|error| AppError::Orchestration(error.to_string()))?;
                supervisor
                    .complete(&parent)
                    .await
                    .map_err(|error| AppError::Orchestration(error.to_string()))?;
            }
            Ok(())
        }
        .await;
        // 任务终态与 supervisor 真实终态配对：取消 → Canceled；全部完成
        // → Completed；中途失败 → Failed（带原因）。
        let (task_status, task_detail) = match &outcome {
            Err(error) => (
                pawork_domain::TaskStatus::Failed,
                Some(format!("demo aborted: {error}")),
            ),
            Ok(()) if options.cancel => (pawork_domain::TaskStatus::Canceled, None),
            Ok(()) => (pawork_domain::TaskStatus::Completed, None),
        };
        for task_id in &worker_tasks {
            if let Err(error) =
                self.tasks_finish_from_run(task_id, task_status, task_detail.clone())
            {
                tracing::warn!(%error, "failed to finish orchestration worker agent task");
            }
        }
        outcome?;

        let event_kinds: Vec<String> = supervisor
            .events()
            .iter()
            .map(orchestration_event_kind)
            .collect();
        let budget_exceeded = event_kinds.iter().any(|kind| kind == "BudgetExceeded");
        Ok(MultiAgentDemoReport {
            parent_id: parent.as_str().to_string(),
            workers: vec![left.as_str().to_string(), right.as_str().to_string()],
            cancelled,
            budget_exceeded,
            event_kinds,
        })
    }
}

async fn spawn_worker(
    supervisor: &AgentSupervisor,
    parent: &AgentId,
    session_id: &SessionId,
    tenant_id: &pawork_domain::TenantId,
    principal_id: &pawork_domain::PrincipalId,
    provider: &str,
    model: &str,
    description: &str,
    budget_input_tokens: Option<u64>,
) -> Result<AgentId, AppError> {
    let provider_id = ProviderId::new(provider);
    supervisor
        .spawn(SpawnRequest {
            tenant_id: tenant_id.clone(),
            principal_id: principal_id.clone(),
            parent_id: Some(parent.clone()),
            session_id: session_id.clone(),
            worktree_path: None,
            budget: budget_input_tokens.map(|max_input_tokens| WorkerBudgetLimits {
                max_input_tokens: Some(max_input_tokens),
                ..WorkerBudgetLimits::default()
            }),
            model: Some(ModelId::new(model)),
            acquire: Some(AcquireRequest {
                tenant_id: tenant_id.clone(),
                principal_id: principal_id.clone(),
                session_id: session_id.clone(),
                agent_id: AgentId::new("pending"),
                provider_id: Some(provider_id),
                account_id: None,
                trace_id: Some(description.into()),
            }),
            task_deps: Vec::new(),
            task_description: Some(description.into()),
            task_max_retries: None,
        })
        .await
        .map_err(|error| AppError::Orchestration(error.to_string()))
}

fn orchestration_event_kind(event: &OrchestrationEvent) -> String {
    match event {
        OrchestrationEvent::WorkerCreated { .. } => "WorkerCreated",
        OrchestrationEvent::WorkerAdmitted { .. } => "WorkerAdmitted",
        OrchestrationEvent::WorkerStarted { .. } => "WorkerStarted",
        OrchestrationEvent::WorkerRunning { .. } => "WorkerRunning",
        OrchestrationEvent::WorkerWaiting { .. } => "WorkerWaiting",
        OrchestrationEvent::WorkerCompleted { .. } => "WorkerCompleted",
        OrchestrationEvent::WorkerCancelling { .. } => "WorkerCancelling",
        OrchestrationEvent::WorkerCancelled { .. } => "WorkerCancelled",
        OrchestrationEvent::WorkerFailed { .. } => "WorkerFailed",
        OrchestrationEvent::TaskCreated { .. } => "TaskCreated",
        OrchestrationEvent::TaskReady { .. } => "TaskReady",
        OrchestrationEvent::TaskAssigned { .. } => "TaskAssigned",
        OrchestrationEvent::TaskCompleted { .. } => "TaskCompleted",
        OrchestrationEvent::TaskFailed { .. } => "TaskFailed",
        OrchestrationEvent::TaskRetried { .. } => "TaskRetried",
        OrchestrationEvent::TaskCancelled { .. } => "TaskCancelled",
        OrchestrationEvent::BudgetExceeded { .. } => "BudgetExceeded",
        OrchestrationEvent::ConcurrencyDenied { .. } => "ConcurrencyDenied",
        OrchestrationEvent::PatchProposed { .. } => "PatchProposed",
        OrchestrationEvent::PatchMerged { .. } => "PatchMerged",
        OrchestrationEvent::PatchConflict { .. } => "PatchConflict",
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use pawork_domain::{TaskKind, TaskStatus};
    use pawork_workflow::task::is_terminal_status;

    use super::*;
    use crate::testsupport::mock_core;

    /// R-16：demo 执行与取消两臂都不留悬空 Agent 任务——任务终态与
    /// supervisor 真实终态配对，持久快照重放后无活跃残留。
    #[tokio::test]
    async fn demo_pairs_worker_task_terminals() {
        for cancel in [false, true] {
            let (mut core, dir) = mock_core(Vec::new()).await;
            core.open_control_plane(dir.path()).expect("control plane");
            let report = core
                .run_multi_agent_demo(MultiAgentDemoOptions {
                    cancel,
                    budget_input_tokens: None,
                })
                .await
                .expect("demo");
            assert_eq!(report.workers.len(), 2);

            let expected = if cancel {
                TaskStatus::Canceled
            } else {
                TaskStatus::Completed
            };
            let agent_tasks: Vec<_> = core
                .tasks_list()
                .into_iter()
                .filter(|task| task.task_kind == TaskKind::Agent)
                .collect();
            assert_eq!(agent_tasks.len(), 2, "demo registers two worker tasks");
            for task in &agent_tasks {
                assert_eq!(
                    task.status, expected,
                    "worker task terminal must pair with demo outcome"
                );
            }

            let reloaded = crate::tasks_host::load_task_manager(&dir.path().join("tasks.json"))
                .expect("reload tasks snapshot");
            assert!(
                reloaded
                    .tasks()
                    .iter()
                    .all(|task| is_terminal_status(task.status)),
                "persisted snapshot must not claim any task still active"
            );
            core.shutdown().await.expect("shutdown");
        }
    }
}
