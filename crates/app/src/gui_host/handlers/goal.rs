//! Continuous goals reuse RunStart, its session reservation, approvals and durable terminal path.
use super::super::{GuiBroadcastSink, GuiHostAdapter};
use super::run_start::SessionSlotGuard;
use crate::{gui_server::GuiHostError, GoalSnapshot};
use pawork_domain::{
    AgentEvent, AgentEventEnvelope, CancellationToken, CriterionKind, GoalEvent, GoalId,
    GoalStatus, SessionId, SuccessCriterionSnapshot,
};
use pawork_engine::{AgentEventSink, EngineError};
use pawork_protocol::{AppCommand, AppCommandEnvelope, AppQuery, AppResponse};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};

#[derive(Clone)]
pub(in crate::gui_host) struct GoalFlight {
    stop: CancellationToken,
    done: tokio::sync::watch::Receiver<bool>,
}
pub(in crate::gui_host) type GoalFlights = Arc<Mutex<HashMap<String, GoalFlight>>>;
fn error(message: impl Into<String>) -> GuiHostError {
    GuiHostAdapter::host_error("goal_error", message)
}
fn data(goal: Option<GoalSnapshot>) -> Result<AppResponse, GuiHostError> {
    serde_json::to_value(goal)
        .map(AppResponse::Data)
        .map_err(|e| error(e.to_string()))
}
fn budget(tokens: u64, runs: u32) -> Result<(), GuiHostError> {
    if tokens == 0 || runs == 0 || runs > 100 {
        return Err(error("Specify a positive token budget and 1–100 runs"));
    }
    Ok(())
}
pub(crate) async fn get(
    adapter: &GuiHostAdapter,
    query: &AppQuery,
) -> Result<AppResponse, GuiHostError> {
    let AppQuery::GoalGet { session_id } = query else {
        unreachable!()
    };
    let mut goal = adapter
        .core
        .read()
        .await
        .goal_snapshot(session_id)
        .await
        .map_err(GuiHostAdapter::app_error)?;
    if let Some(goal) = &mut goal {
        if goal.status == GoalStatus::Active
            && !adapter
                .goals
                .lock()
                .unwrap()
                .contains_key(session_id.as_str())
        {
            goal.status = GoalStatus::Paused;
            goal.current_run = None;
            goal.pause_reason = Some("host_restarted; explicit resume required".into());
        }
    }
    data(goal)
}
async fn stop(adapter: &GuiHostAdapter, session: &SessionId) {
    let flight = adapter.goals.lock().unwrap().get(session.as_str()).cloned();
    if let Some(mut flight) = flight {
        flight.stop.cancel();
        for run in adapter
            .runs
            .active()
            .into_iter()
            .filter(|r| &r.session_id == session)
        {
            adapter.runs.cancel(&run.run_id);
        }
        while !*flight.done.borrow() {
            if flight.done.changed().await.is_err() {
                break;
            }
        }
    }
}

pub(crate) async fn change(
    adapter: &GuiHostAdapter,
    envelope: &AppCommandEnvelope,
    command: &AppCommand,
) -> Result<AppResponse, GuiHostError> {
    // Serialize state changes, including pause/steer waiting for the current durable terminal.
    let _commands = adapter.goal_commands.lock().await;
    let session = match command {
        AppCommand::GoalStart { session_id, .. }
        | AppCommand::GoalPause { session_id, .. }
        | AppCommand::GoalResume { session_id, .. }
        | AppCommand::GoalSteer { session_id, .. }
        | AppCommand::GoalFinish { session_id, .. } => session_id,
        _ => unreachable!(),
    };
    if adapter.runs.is_stopping() {
        return Err(error("Host is stopping"));
    }
    let previous = adapter
        .core
        .read()
        .await
        .goal_snapshot(session)
        .await
        .map_err(GuiHostAdapter::app_error)?;
    let mut launch = false;
    let mut resume_after_steer = false;
    match command {
        AppCommand::GoalStart {
            title,
            criteria,
            budget_tokens,
            max_runs,
            ..
        } => {
            budget(*budget_tokens, *max_runs)?;
            if title.trim().is_empty()
                || title.len() > 256
                || criteria.is_empty()
                || criteria.len() > 16
                || criteria
                    .iter()
                    .any(|c| c.trim().is_empty() || c.len() > 2048)
            {
                return Err(error(
                    "A goal requires a title (≤256 bytes) and 1–16 criteria (≤2048 bytes each)",
                ));
            }
            if previous
                .as_ref()
                .is_some_and(|g| matches!(g.status, GoalStatus::Active | GoalStatus::Paused))
            {
                return Err(error("Finish the current goal before starting another"));
            }
            launch = true;
        }
        AppCommand::GoalPause { goal_id, .. }
        | AppCommand::GoalResume { goal_id, .. }
        | AppCommand::GoalSteer { goal_id, .. }
        | AppCommand::GoalFinish { goal_id, .. } => {
            let goal = previous
                .as_ref()
                .filter(|g| &g.goal_id == goal_id)
                .ok_or_else(|| error("Goal changed; refresh the panel"))?;
            if !matches!(goal.status, GoalStatus::Active | GoalStatus::Paused) {
                return Err(error("Goal is already finished"));
            }
            match command {
                AppCommand::GoalResume {
                    budget_tokens,
                    max_runs,
                    ..
                } => {
                    budget(*budget_tokens, *max_runs)?;
                    if adapter.goals.lock().unwrap().contains_key(session.as_str()) {
                        return Err(error("Pause the running goal before adding budget"));
                    }
                    launch = true;
                }
                AppCommand::GoalSteer { input, .. } => {
                    if input.trim().is_empty() || input.len() > 4096 {
                        return Err(error("Direction must be nonempty and ≤4096 bytes"));
                    }
                    resume_after_steer =
                        adapter.goals.lock().unwrap().contains_key(session.as_str());
                }
                AppCommand::GoalFinish {
                    outcome, reason, ..
                } => {
                    if !matches!(outcome.as_str(), "achieved" | "abandoned")
                        || reason.as_ref().is_some_and(|r| r.len() > 2048)
                    {
                        return Err(error(
                            "Choose achieved or abandoned; reason is limited to 2048 bytes",
                        ));
                    }
                }
                _ => {}
            }
        }
        _ => unreachable!(),
    }
    if !matches!(command, AppCommand::GoalStart { .. }) {
        stop(adapter, session).await;
    }
    let slot = SessionSlotGuard::acquire(adapter, session)?;
    let core = adapter.core.read().await;
    if launch || resume_after_steer {
        core.ensure_plan_allows_execution(session)
            .await
            .map_err(GuiHostAdapter::app_error)?;
    }
    let latest = core
        .goal_snapshot(session)
        .await
        .map_err(GuiHostAdapter::app_error)?;
    let event = match command {
        AppCommand::GoalStart {
            title,
            criteria,
            budget_tokens,
            max_runs,
            ..
        } => GoalEvent::Created {
            goal_id: GoalId::from(format!("goal-{}", envelope.command_id.as_str())),
            title: title.trim().into(),
            criteria: criteria
                .iter()
                .enumerate()
                .map(|(i, c)| SuccessCriterionSnapshot {
                    criterion_id: format!("criterion-{}", i + 1),
                    description: c.trim().into(),
                    kind: CriterionKind::Human,
                    satisfied: false,
                })
                .collect(),
            budget_tokens: Some(*budget_tokens),
            max_runs: Some(*max_runs),
        },
        AppCommand::GoalPause { goal_id, .. } => GoalEvent::Paused {
            goal_id: goal_id.clone(),
        },
        AppCommand::GoalResume {
            goal_id,
            budget_tokens,
            max_runs,
            ..
        } => GoalEvent::Resumed {
            goal_id: goal_id.clone(),
            remaining_budget_tokens: latest
                .as_ref()
                .expect("existing goal")
                .remaining_tokens()
                .saturating_add(*budget_tokens),
            max_runs: Some(
                latest
                    .as_ref()
                    .expect("existing goal")
                    .remaining_runs()
                    .saturating_add(*max_runs),
            ),
        },
        AppCommand::GoalSteer { goal_id, input, .. } => GoalEvent::Steered {
            goal_id: goal_id.clone(),
            input: input.trim().into(),
        },
        AppCommand::GoalFinish {
            goal_id,
            outcome,
            reason,
            ..
        } => {
            if outcome == "achieved" {
                for criterion in &latest.as_ref().expect("existing goal").criteria {
                    if !criterion.satisfied {
                        core.persist_goal_event(
                            session,
                            GoalEvent::CriterionSatisfied {
                                goal_id: goal_id.clone(),
                                criterion_id: criterion.criterion_id.clone(),
                            },
                        )
                        .await
                        .map_err(GuiHostAdapter::app_error)?;
                    }
                }
                GoalEvent::Achieved {
                    goal_id: goal_id.clone(),
                }
            } else {
                GoalEvent::Abandoned {
                    goal_id: goal_id.clone(),
                    reason: reason.clone().unwrap_or_else(|| "Stopped by user".into()),
                }
            }
        }
        _ => unreachable!(),
    };
    if let GoalEvent::Paused { goal_id } = &event {
        core.pause_goal(session, goal_id, "user_paused")
            .await
            .map_err(GuiHostAdapter::app_error)?;
    } else {
        core.persist_goal_event(session, event)
            .await
            .map_err(GuiHostAdapter::app_error)?;
    }
    if resume_after_steer {
        if let Some(goal) = latest.filter(|g| g.remaining_runs() > 0 && g.remaining_tokens() > 0) {
            core.persist_goal_event(
                session,
                GoalEvent::Resumed {
                    goal_id: goal.goal_id.clone(),
                    remaining_budget_tokens: goal.remaining_tokens(),
                    max_runs: Some(goal.remaining_runs()),
                },
            )
            .await
            .map_err(GuiHostAdapter::app_error)?;
            launch = true;
        }
    }
    let goal = core
        .goal_snapshot(session)
        .await
        .map_err(GuiHostAdapter::app_error)?;
    let provider = core.provider_id().clone();
    let model = core.model().clone();
    drop(core);
    if launch {
        let stop = CancellationToken::new();
        let (done, receiver) = tokio::sync::watch::channel(false);
        adapter.goals.lock().unwrap().insert(
            session.as_str().into(),
            GoalFlight {
                stop: stop.clone(),
                done: receiver,
            },
        );
        let worker = adapter.clone();
        let session = session.clone();
        let envelope = envelope.clone();
        drop(slot);
        adapter.runs.spawn(async move {
            run_goal(&worker, &envelope, &session, provider, model, stop).await;
            worker.goals.lock().unwrap().remove(session.as_str());
            let _ = done.send(true);
        });
    }
    data(goal)
}

async fn run_goal(
    adapter: &GuiHostAdapter,
    envelope: &AppCommandEnvelope,
    session: &SessionId,
    provider: pawork_domain::ProviderId,
    model: pawork_domain::ModelId,
    stop: CancellationToken,
) {
    let reason = loop {
        if adapter.runs.is_stopping() {
            break "host_stopped".to_string();
        }
        if stop.is_cancelled() {
            break "user_paused".to_string();
        }
        let goal = match adapter.core.read().await.goal_snapshot(session).await {
            Ok(Some(goal)) => goal,
            _ => break "goal_unavailable".into(),
        };
        if goal.remaining_tokens() == 0 {
            break "token_budget_exhausted".into();
        }
        if goal.remaining_runs() == 0 {
            break "run_limit_reached".into();
        }
        let command = AppCommand::RunStart {
            session_id: session.clone(),
            user_message: format!(
                "Continue this explicit goal: {}\nSuccess criteria:\n{}\nDirection:\n{}\nPerform the next useful step and report evidence. Do not declare user-reviewed criteria achieved yourself.",
                goal.title,
                goal.criteria
                    .iter()
                    .map(|c| c.description.as_str())
                    .collect::<Vec<_>>()
                    .join("\n"),
                goal.steering.join("\n")
            ),
            model: Some(model.clone()),
            provider: Some(provider.clone()),
            profile: None,
            effort: None,
            attachment_ids: Vec::new(),
            web_search: None,
            video_urls: Vec::new(),
        };
        let response =
            super::run_start::start_for_goal(adapter, envelope, &command, goal.remaining_tokens())
                .await;
        let run = match response {
            Ok(AppResponse::Accepted {
                run_id: Some(run), ..
            }) => run,
            Ok(_) => break "unexpected_run_response".into(),
            Err(error) => break error.code,
        };
        while adapter.runs.session_busy(session) {
            if stop.is_cancelled() || adapter.runs.is_stopping() {
                adapter.runs.cancel(&run);
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        if adapter.runs.is_stopping() {
            break "host_stopped".into();
        }
        if stop.is_cancelled() {
            break "user_paused".into();
        }
        match adapter.core.read().await.goal_snapshot(session).await {
            Ok(Some(goal)) if goal.remaining_tokens() == 0 => {
                break "token_budget_exhausted".into();
            }
            Ok(Some(goal)) if goal.pause_reason.is_some() => break goal.pause_reason.unwrap(),
            Err(_) => break "goal_replay_failed".into(),
            _ => {}
        }
        // Yield to user commands between runs; no independent scheduler/runtime.
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    if let Ok(_slot) = SessionSlotGuard::acquire(adapter, session) {
        let core = adapter.core.read().await;
        if let Ok(Some(goal)) = core.goal_snapshot(session).await {
            if let Err(error) = core.pause_goal(session, &goal.goal_id, &reason).await {
                tracing::error!(%error, "goal pause persistence failed");
            }
        }
    }
}

/// Usage is reported by the Provider; stop on the first observed exhausted budget.
/// Track per-request snapshots so tool rounds cannot reset the accumulated count.
pub(super) struct BudgetSink {
    pub inner: GuiBroadcastSink,
    pub limit: Option<u64>,
    pub cancel: CancellationToken,
    pub usage: Mutex<(u64, u64)>,
}
#[async_trait::async_trait]
impl AgentEventSink for BudgetSink {
    async fn emit(&self, envelope: AgentEventEnvelope) -> Result<(), EngineError> {
        if let Some(limit) = self.limit {
            let mut usage = self.usage.lock().unwrap();
            match &envelope.payload {
                AgentEvent::ProviderRequestStarted { .. } => {
                    usage.0 = usage.0.saturating_add(usage.1);
                    usage.1 = 0;
                }
                AgentEvent::UsageUpdated { usage: current } => {
                    usage.1 = current.input_tokens.saturating_add(current.output_tokens)
                }
                AgentEvent::ContextPrepared {
                    estimated_input_tokens,
                    ..
                } if usage
                    .0
                    .saturating_add(usage.1)
                    .saturating_add(*estimated_input_tokens)
                    >= limit =>
                {
                    self.cancel.cancel()
                }
                _ => {}
            }
            if usage.0.saturating_add(usage.1) >= limit {
                self.cancel.cancel();
            }
        }
        self.inner.emit(envelope).await
    }
}
