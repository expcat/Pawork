//! S11 波 D：多 Agent demo（Supervisor spawn / cancel-tree / budget-gate）。

use std::sync::Arc;

use pawork_control_plane::{default_principal, default_tenant};
use pawork_domain::{AgentId, ModelId, ProviderId, SessionId};
use pawork_orchestration::{
    AgentSupervisor, OrchestrationEvent, SpawnRequest, SupervisorConfig, WorkerBudgetLimits,
};
use pawork_control_plane::credential::AcquireRequest;
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
            if let Err(error) = self.tasks_start_agent(None) {
                tracing::warn!(%error, "failed to start orchestration worker agent task");
            }
            if let Some(limit) = options.budget_input_tokens {
                supervisor
                    .record_usage(worker, limit.saturating_add(1), 0, 0)
                    .await
                    .map_err(|error| AppError::Orchestration(error.to_string()))?;
            }
        }

        let mut cancelled = Vec::new();
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
