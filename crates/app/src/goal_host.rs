//! Durable continuous goal projection. Execution is owned by the Host, never the window.
use crate::{AppCore, AppError};
use pawork_domain::{
    AgentEvent, GoalEvent, GoalId, GoalStatus, RunId, SessionId, SuccessCriterionSnapshot,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GoalSnapshot {
    pub goal_id: GoalId,
    pub title: String,
    pub criteria: Vec<SuccessCriterionSnapshot>,
    pub status: GoalStatus,
    pub budget_tokens: Option<u64>,
    pub max_runs: Option<u32>,
    pub used_tokens: u64,
    pub used_runs: u32,
    pub current_run: Option<RunId>,
    pub steering: Vec<String>,
    pub pause_reason: Option<String>,
}
impl GoalSnapshot {
    pub fn remaining_tokens(&self) -> u64 {
        self.budget_tokens
            .unwrap_or(0)
            .saturating_sub(self.used_tokens)
    }
    pub fn remaining_runs(&self) -> u32 {
        self.max_runs.unwrap_or(0).saturating_sub(self.used_runs)
    }
}
impl AppCore {
    /// Rebuild from canonical events, including canceled/failed run usage.
    pub async fn goal_snapshot(
        &self,
        session: &SessionId,
    ) -> Result<Option<GoalSnapshot>, AppError> {
        self.get_session(session).await?;
        let events = self.store()?.replay_events(session, 1, usize::MAX).await?;
        let mut snapshot: Option<GoalSnapshot> = None;
        let mut in_flight_usage = 0u64;
        let mut prior_request_usage = 0u64;
        for event in events {
            if let AgentEvent::Goal(GoalEvent::Created {
                goal_id,
                title,
                criteria,
                budget_tokens,
                max_runs,
            }) = event.payload
            {
                snapshot = Some(GoalSnapshot {
                    goal_id,
                    title,
                    criteria,
                    status: GoalStatus::Active,
                    budget_tokens,
                    max_runs,
                    used_tokens: 0,
                    used_runs: 0,
                    current_run: None,
                    steering: Vec::new(),
                    pause_reason: None,
                });
                in_flight_usage = 0;
                prior_request_usage = 0;
                continue;
            }
            let Some(goal) = snapshot.as_mut() else {
                continue;
            };
            match event.payload {
                AgentEvent::Goal(GoalEvent::Paused { goal_id }) if goal_id == goal.goal_id => {
                    goal.status = GoalStatus::Paused
                }
                AgentEvent::Goal(GoalEvent::Resumed {
                    goal_id,
                    remaining_budget_tokens,
                    max_runs,
                }) if goal_id == goal.goal_id => {
                    goal.budget_tokens =
                        Some(goal.used_tokens.saturating_add(remaining_budget_tokens));
                    goal.max_runs = max_runs.map(|n| goal.used_runs.saturating_add(n));
                    goal.status = GoalStatus::Active;
                    goal.pause_reason = None;
                }
                AgentEvent::Goal(GoalEvent::Steered { goal_id, input })
                    if goal_id == goal.goal_id =>
                {
                    goal.steering.push(input);
                    if goal.steering.len() > 16 {
                        goal.steering.remove(0);
                    }
                }
                AgentEvent::Goal(GoalEvent::Achieved { goal_id }) if goal_id == goal.goal_id => {
                    goal.status = GoalStatus::Achieved;
                    goal.pause_reason = None;
                }
                AgentEvent::Goal(GoalEvent::Abandoned { goal_id, reason })
                    if goal_id == goal.goal_id =>
                {
                    goal.status = GoalStatus::Abandoned;
                    goal.pause_reason = Some(reason);
                }
                AgentEvent::Goal(GoalEvent::CriterionSatisfied {
                    goal_id,
                    criterion_id,
                }) if goal_id == goal.goal_id => {
                    if let Some(criterion) = goal
                        .criteria
                        .iter_mut()
                        .find(|c| c.criterion_id == criterion_id)
                    {
                        criterion.satisfied = true;
                    }
                }
                AgentEvent::RunStarted { .. } if goal.status == GoalStatus::Active => {
                    goal.current_run = Some(event.run_id.clone());
                    goal.used_runs = goal.used_runs.saturating_add(1);
                    in_flight_usage = 0;
                    prior_request_usage = 0;
                }
                AgentEvent::RunCompleted { usage, .. }
                    if goal.current_run.as_ref() == Some(&event.run_id) =>
                {
                    goal.used_tokens = goal
                        .used_tokens
                        .saturating_add(usage.input_tokens.saturating_add(usage.output_tokens));
                    goal.current_run = None;
                    in_flight_usage = 0;
                    prior_request_usage = 0;
                }
                AgentEvent::RunCancelled { usage, .. } | AgentEvent::RunFailed { usage, .. }
                    if goal.current_run.as_ref() == Some(&event.run_id) =>
                {
                    let used = usage
                        .map(|u| u.input_tokens.saturating_add(u.output_tokens))
                        .unwrap_or(prior_request_usage.saturating_add(in_flight_usage));
                    goal.used_tokens = goal.used_tokens.saturating_add(used);
                    goal.current_run = None;
                    in_flight_usage = 0;
                    prior_request_usage = 0;
                    goal.pause_reason = Some("run_stopped".into());
                }
                AgentEvent::ProviderRequestStarted { .. }
                    if goal.current_run.as_ref() == Some(&event.run_id) =>
                {
                    prior_request_usage = prior_request_usage.saturating_add(in_flight_usage);
                    in_flight_usage = 0;
                }
                AgentEvent::UsageUpdated { usage }
                    if goal.current_run.as_ref() == Some(&event.run_id) =>
                {
                    in_flight_usage = usage.input_tokens.saturating_add(usage.output_tokens)
                }
                AgentEvent::Diagnostic { code, details } if code == "goal.paused" => {
                    if details["goal_id"].as_str() == Some(goal.goal_id.as_str()) {
                        goal.pause_reason = details["reason"].as_str().map(str::to_owned);
                    }
                }
                _ => {}
            }
        }
        if let Some(goal) = &mut snapshot {
            goal.used_tokens = goal
                .used_tokens
                .saturating_add(prior_request_usage)
                .saturating_add(in_flight_usage);
        }
        Ok(snapshot)
    }

    pub(crate) async fn persist_goal_event(
        &self,
        session: &SessionId,
        event: GoalEvent,
    ) -> Result<(), AppError> {
        let mut sequence = self.next_sequence(session).await?;
        let run = RunId::from(format!("run-goal-{sequence}"));
        self.append_payload(session, &run, &mut sequence, AgentEvent::Goal(event))
            .await?;
        Ok(())
    }
    pub(crate) async fn pause_goal(
        &self,
        session: &SessionId,
        goal_id: &GoalId,
        reason: &str,
    ) -> Result<(), AppError> {
        let mut sequence = self.next_sequence(session).await?;
        let run = RunId::from(format!("run-goal-{sequence}"));
        self.append_payload(
            session,
            &run,
            &mut sequence,
            AgentEvent::Diagnostic {
                code: "goal.paused".into(),
                details: serde_json::json!({"goal_id":goal_id,"reason":reason}),
            },
        )
        .await?;
        self.append_payload(
            session,
            &run,
            &mut sequence,
            AgentEvent::Goal(GoalEvent::Paused {
                goal_id: goal_id.clone(),
            }),
        )
        .await?;
        Ok(())
    }
}
