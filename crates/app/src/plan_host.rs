//! S11 波 D：Plan 审批 gate。无 Plan 时放行；有未批准版本则拦截 `run_session`。

use pawork_control_plane::{AuditAction, AuditDecision, AuditTargetKind};
use pawork_domain::{AgentEvent, PlanEvent, PlanReviewStatus, RunId, SessionId};
use pawork_workflow::plan::{PlanError, PlanService, PlanSnapshot};

use crate::control::append_audit;
use crate::{AppCore, AppError};

impl AppCore {
    pub async fn plan_snapshot(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<PlanSnapshot>, AppError> {
        Ok(self.plan_service(session_id).await?.plan_snapshot())
    }

    pub async fn plan_create(
        &self,
        session_id: &SessionId,
        title: &str,
        steps: Vec<String>,
    ) -> Result<PlanSnapshot, AppError> {
        let service = self.plan_service(session_id).await?;
        let event = service.create_plan(title, steps).map_err(plan_error)?;
        self.persist_plan_event(session_id, event).await?;
        self.plan_snapshot(session_id)
            .await?
            .ok_or_else(|| AppError::Plan("plan missing after create".into()))
    }

    pub async fn plan_replace(
        &self,
        session_id: &SessionId,
        title: &str,
        steps: Vec<String>,
    ) -> Result<PlanSnapshot, AppError> {
        let service = self.plan_service(session_id).await?;
        let event = service.replace_plan(title, steps).map_err(plan_error)?;
        self.persist_plan_event(session_id, event).await?;
        self.plan_snapshot(session_id)
            .await?
            .ok_or_else(|| AppError::Plan("plan missing after replace".into()))
    }

    pub async fn plan_submit(&self, session_id: &SessionId) -> Result<PlanSnapshot, AppError> {
        let service = self.plan_service(session_id).await?;
        let snapshot = service
            .plan_snapshot()
            .ok_or_else(|| AppError::Plan(PlanError::NotCreated.to_string()))?;
        let event = service
            .request_review(&snapshot.version)
            .map_err(plan_error)?;
        self.persist_plan_event(session_id, event).await?;
        self.plan_snapshot(session_id)
            .await?
            .ok_or_else(|| AppError::Plan("plan missing after submit".into()))
    }

    pub async fn plan_approve(&self, session_id: &SessionId) -> Result<PlanSnapshot, AppError> {
        let service = self.plan_service(session_id).await?;
        let snapshot = service
            .plan_snapshot()
            .ok_or_else(|| AppError::Plan(PlanError::NotCreated.to_string()))?;
        if snapshot.review_status == PlanReviewStatus::Draft {
            let event = service
                .request_review(&snapshot.version)
                .map_err(plan_error)?;
            self.persist_plan_event(session_id, event).await?;
        }
        let service = self.plan_service(session_id).await?;
        let snapshot = service
            .plan_snapshot()
            .ok_or_else(|| AppError::Plan(PlanError::NotCreated.to_string()))?;
        let event = service
            .approve(&snapshot.plan_id, &snapshot.version, None)
            .map_err(plan_error)?;
        self.persist_plan_event(session_id, event).await?;
        append_audit(
            self.usage.control.audit.as_ref(),
            AuditAction::ApprovalEvaluated,
            AuditTargetKind::Approval,
            AuditDecision::Allow,
            "plan_approved",
        );
        self.plan_snapshot(session_id)
            .await?
            .ok_or_else(|| AppError::Plan("plan missing after approve".into()))
    }

    pub async fn plan_reject(
        &self,
        session_id: &SessionId,
        reason: &str,
    ) -> Result<PlanSnapshot, AppError> {
        let service = self.plan_service(session_id).await?;
        let snapshot = service
            .plan_snapshot()
            .ok_or_else(|| AppError::Plan(PlanError::NotCreated.to_string()))?;
        if snapshot.review_status == PlanReviewStatus::Draft {
            let event = service
                .request_review(&snapshot.version)
                .map_err(plan_error)?;
            self.persist_plan_event(session_id, event).await?;
        }
        let service = self.plan_service(session_id).await?;
        let snapshot = service
            .plan_snapshot()
            .ok_or_else(|| AppError::Plan(PlanError::NotCreated.to_string()))?;
        let event = service
            .reject(&snapshot.plan_id, &snapshot.version, reason)
            .map_err(plan_error)?;
        self.persist_plan_event(session_id, event).await?;
        append_audit(
            self.usage.control.audit.as_ref(),
            AuditAction::ApprovalEvaluated,
            AuditTargetKind::Approval,
            AuditDecision::Deny,
            "plan_rejected",
        );
        self.plan_snapshot(session_id)
            .await?
            .ok_or_else(|| AppError::Plan("plan missing after reject".into()))
    }

    pub(crate) async fn ensure_plan_allows_execution(
        &self,
        session_id: &SessionId,
    ) -> Result<(), AppError> {
        let Ok(service) = self.plan_service(session_id).await else {
            return Ok(());
        };
        let Some(snapshot) = service.plan_snapshot() else {
            return Ok(());
        };
        if service.is_approved_for_execution(&snapshot.plan_id, &snapshot.version) {
            return Ok(());
        }
        append_audit(
            self.usage.control.audit.as_ref(),
            AuditAction::ApprovalEvaluated,
            AuditTargetKind::Approval,
            AuditDecision::Deny,
            "plan_blocked",
        );
        Err(AppError::PlanNotApproved {
            plan_id: snapshot.plan_id.as_str().to_string(),
            version: snapshot.version.as_str().to_string(),
            status: review_status_label(snapshot.review_status).into(),
        })
    }

    async fn plan_service(&self, session_id: &SessionId) -> Result<PlanService, AppError> {
        let events = self
            .store()?
            .replay_events(session_id, 1, usize::MAX)
            .await?;
        let plan_events: Vec<&PlanEvent> = events
            .iter()
            .filter_map(|envelope| match &envelope.payload {
                AgentEvent::Plan(event) => Some(event),
                _ => None,
            })
            .collect();
        Ok(PlanService::from_events(plan_events))
    }

    async fn persist_plan_event(
        &self,
        session_id: &SessionId,
        event: PlanEvent,
    ) -> Result<(), AppError> {
        let mut sequence = self.next_sequence(session_id).await?;
        let run_id = RunId::from(format!("run-plan-{sequence}"));
        self.append_payload(session_id, &run_id, &mut sequence, AgentEvent::Plan(event))
            .await
            .map(|_| ())
    }
}

fn plan_error(error: PlanError) -> AppError {
    AppError::Plan(error.to_string())
}

pub fn review_status_label(status: PlanReviewStatus) -> &'static str {
    match status {
        PlanReviewStatus::Draft => "draft",
        PlanReviewStatus::InReview => "in_review",
        PlanReviewStatus::ChangesRequested => "changes_requested",
        PlanReviewStatus::Approved => "approved",
        PlanReviewStatus::Rejected => "rejected",
    }
}
