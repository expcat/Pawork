//! 进程内内存 [`PlanService`]：命令面（状态机校验 + 产出 canonical 事件）与查询面。
//!
//! 命令方法在锁内完成校验、构造 [`PlanEvent`]、`apply` 到内部 state 后返回事件
//! 给调用方（由 session-store 封装为 `pawork_domain::AgentEvent::Plan` 持久化）。
//! 本服务**只读**：不暴露任何 spawn / exec / write / 文件 / 网络 API。

// 毒锁策略：panic 不屏蔽毒化传播的前提下取回内部数据继续（不吞错误状态）。
use std::sync::{Mutex, PoisonError};

use pawork_domain::{
    CheckpointId, PlanCommentAnchor, PlanEvent, PlanId, PlanReviewStatus, PlanStepId,
    PlanStepSnapshot, PlanStepStatus, PlanVersionId,
};

use super::error::PlanError;
use super::snapshot::{PlanSnapshot, PlanVersionInfo};
use super::state::{apply, is_legal_step_transition, replay, PlanState};

/// 进程内内存 Plan service（只读聚合）。
pub struct PlanService {
    inner: Mutex<Inner>,
}

struct Inner {
    state: PlanState,
    next_plan: u64,
    next_version: u64,
    next_step: u64,
}

impl Default for PlanService {
    fn default() -> Self {
        Self {
            inner: Mutex::new(Inner {
                state: PlanState::default(),
                next_plan: 1,
                next_version: 1,
                next_step: 1,
            }),
        }
    }
}

impl PlanService {
    /// 创建一个空 service（无 Plan）。
    pub fn new() -> Self {
        Self::default()
    }

    /// 从 canonical 事件序列重放重建 service（恢复入口）。
    pub fn from_events<'a>(events: impl IntoIterator<Item = &'a PlanEvent>) -> Self {
        let events: Vec<&PlanEvent> = events.into_iter().collect();
        let state = replay(events.iter().copied());
        let (next_plan, next_version, next_step) = seed_counters(&events);
        Self {
            inner: Mutex::new(Inner {
                state,
                next_plan: next_plan + 1,
                next_version: next_version + 1,
                next_step: next_step + 1,
            }),
        }
    }

    /// 创建首版 Plan；返回已 apply 的 [`PlanEvent::Created`]。
    pub fn create_plan(
        &self,
        title: &str,
        step_texts: Vec<String>,
    ) -> Result<PlanEvent, PlanError> {
        if step_texts.is_empty() {
            return Err(PlanError::EmptyPlan);
        }
        if step_texts.iter().any(|t| t.trim().is_empty()) {
            return Err(PlanError::EmptyStepText);
        }

        let mut inner = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(plan_id) = inner.state.plan_id().cloned() {
            return Err(PlanError::AlreadyExists(plan_id));
        }

        let plan_id = PlanId::new(format!("plan_{}", inner.next_plan));
        let version = PlanVersionId::new(format!("planver_{}", inner.next_version));
        let steps = build_steps(&mut inner, step_texts);

        inner.next_plan += 1;
        inner.next_version += 1;

        let event = PlanEvent::Created {
            plan_id,
            version,
            title: title.to_owned(),
            steps,
        };
        apply(&mut inner.state, &event);
        Ok(event)
    }

    /// 整体替换 Plan（新版本，`parent_version` 指向旧版本）；返回 [`PlanEvent::Replaced`]。
    pub fn replace_plan(
        &self,
        title: &str,
        step_texts: Vec<String>,
    ) -> Result<PlanEvent, PlanError> {
        if step_texts.is_empty() {
            return Err(PlanError::EmptyPlan);
        }
        if step_texts.iter().any(|t| t.trim().is_empty()) {
            return Err(PlanError::EmptyStepText);
        }

        let mut inner = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        let plan_id = inner
            .state
            .plan_id()
            .cloned()
            .ok_or(PlanError::NotCreated)?;
        let parent_version = inner
            .state
            .current_version()
            .cloned()
            .ok_or(PlanError::NotCreated)?;

        let version = PlanVersionId::new(format!("planver_{}", inner.next_version));
        let steps = build_steps(&mut inner, step_texts);
        inner.next_version += 1;

        let event = PlanEvent::Replaced {
            plan_id,
            version,
            parent_version,
            title: title.to_owned(),
            steps,
        };
        apply(&mut inner.state, &event);
        Ok(event)
    }

    /// 单步状态转移（须经合法状态机）；返回 [`PlanEvent::StepUpdated`]。
    pub fn update_step(
        &self,
        step_id: &PlanStepId,
        new_status: PlanStepStatus,
        note: Option<String>,
    ) -> Result<PlanEvent, PlanError> {
        let mut inner = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        let plan_id = inner
            .state
            .plan_id()
            .cloned()
            .ok_or(PlanError::NotCreated)?;
        let from = inner
            .state
            .steps()
            .iter()
            .find(|s| &s.step_id == step_id)
            .map(|s| s.status)
            .ok_or_else(|| PlanError::StepNotFound(step_id.clone()))?;
        if !is_legal_step_transition(from, new_status) {
            return Err(PlanError::IllegalStepTransition {
                from,
                to: new_status,
            });
        }
        let event = PlanEvent::StepUpdated {
            plan_id,
            step_id: step_id.clone(),
            status: new_status,
            note,
        };
        apply(&mut inner.state, &event);
        Ok(event)
    }

    /// 查询面：当前 Plan 只读快照；尚未创建时返回 `None`。
    pub fn plan_snapshot(&self) -> Option<PlanSnapshot> {
        self.inner
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .state
            .snapshot()
    }

    /// 查询面：版本修订链（含当前版本，按创建顺序）。
    pub fn version_history(&self) -> Vec<PlanVersionInfo> {
        self.inner
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .state
            .history()
            .to_vec()
    }

    /// 提交评审：`Draft → InReview`；返回 [`PlanEvent::ReviewRequested`]。
    pub fn request_review(&self, version: &PlanVersionId) -> Result<PlanEvent, PlanError> {
        let mut inner = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        let state = &mut inner.state;
        let plan_id = state.plan_id().cloned().ok_or(PlanError::NotCreated)?;
        check_current_version(state, version)?;
        let from = state.review_status();
        if from != PlanReviewStatus::Draft {
            return Err(PlanError::IllegalReviewTransition {
                from,
                to: PlanReviewStatus::InReview,
            });
        }
        let event = PlanEvent::ReviewRequested {
            plan_id,
            version: version.clone(),
        };
        apply(state, &event);
        Ok(event)
    }

    /// 评审方请求修改：`InReview → ChangesRequested`；同样发出
    /// [`PlanEvent::ReviewRequested`]（推进「评审回合」，apply 按当前状态折叠）。
    pub fn request_changes(&self, version: &PlanVersionId) -> Result<PlanEvent, PlanError> {
        let mut inner = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        let state = &mut inner.state;
        let plan_id = state.plan_id().cloned().ok_or(PlanError::NotCreated)?;
        check_current_version(state, version)?;
        let from = state.review_status();
        if from != PlanReviewStatus::InReview {
            return Err(PlanError::IllegalReviewTransition {
                from,
                to: PlanReviewStatus::ChangesRequested,
            });
        }
        let event = PlanEvent::ReviewRequested {
            plan_id,
            version: version.clone(),
        };
        apply(state, &event);
        Ok(event)
    }

    /// 修订：`ChangesRequested → Draft`（新版本，`parent_version` 指向被修订版本）。
    /// 校验 `parent_version` 必须等于当前版本、新版本不同于 parent、且 version
    /// 不与历史冲突。`title`/`steps` 写入 `PlanEvent::Revised`（ADR-037）。
    pub fn revise(
        &self,
        version: &PlanVersionId,
        parent_version: &PlanVersionId,
        title: impl Into<String>,
        steps: Vec<PlanStepSnapshot>,
    ) -> Result<PlanEvent, PlanError> {
        let mut inner = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        let state = &mut inner.state;
        let plan_id = state.plan_id().cloned().ok_or(PlanError::NotCreated)?;
        let current = state
            .current_version()
            .cloned()
            .ok_or(PlanError::NotCreated)?;
        if &current != parent_version {
            return Err(PlanError::VersionMismatch {
                expected: current,
                actual: parent_version.clone(),
            });
        }
        if version == parent_version {
            return Err(PlanError::SameVersion(version.clone()));
        }
        if state.history().iter().any(|h| &h.version == version) {
            return Err(PlanError::DuplicateVersion(version.clone()));
        }
        let from = state.review_status();
        if from != PlanReviewStatus::ChangesRequested {
            return Err(PlanError::NotChangesRequested { current: from });
        }
        let event = PlanEvent::Revised {
            plan_id,
            version: version.clone(),
            parent_version: parent_version.clone(),
            title: title.into(),
            steps,
        };
        apply(state, &event);
        Ok(event)
    }

    /// 审批通过：`InReview | ChangesRequested → Approved`；`checkpoint_id` 标记
    /// 批准点（可回滚）。审批仅作为执行 gate 放行，不赋予任何写 / 执行能力。
    pub fn approve(
        &self,
        plan_id: &PlanId,
        version: &PlanVersionId,
        checkpoint_id: Option<CheckpointId>,
    ) -> Result<PlanEvent, PlanError> {
        let mut inner = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        let state = &mut inner.state;
        check_plan_version(state, plan_id, version)?;
        let from = state.review_status();
        if !matches!(
            from,
            PlanReviewStatus::InReview | PlanReviewStatus::ChangesRequested
        ) {
            return Err(PlanError::IllegalReviewTransition {
                from,
                to: PlanReviewStatus::Approved,
            });
        }
        let event = PlanEvent::Approved {
            plan_id: plan_id.clone(),
            version: version.clone(),
            checkpoint_id,
        };
        apply(state, &event);
        Ok(event)
    }

    /// 审批拒绝：`InReview | ChangesRequested → Rejected`；`reason` 必填。
    pub fn reject(
        &self,
        plan_id: &PlanId,
        version: &PlanVersionId,
        reason: &str,
    ) -> Result<PlanEvent, PlanError> {
        if reason.trim().is_empty() {
            return Err(PlanError::EmptyReason);
        }
        let mut inner = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        let state = &mut inner.state;
        check_plan_version(state, plan_id, version)?;
        let from = state.review_status();
        if !matches!(
            from,
            PlanReviewStatus::InReview | PlanReviewStatus::ChangesRequested
        ) {
            return Err(PlanError::IllegalReviewTransition {
                from,
                to: PlanReviewStatus::Rejected,
            });
        }
        let event = PlanEvent::Rejected {
            plan_id: plan_id.clone(),
            version: version.clone(),
            reason: reason.to_owned(),
        };
        apply(state, &event);
        Ok(event)
    }

    /// 追加行锚点评审意见（锚点 `step_id` 必须是当前版本的既有步骤）；返回
    /// [`PlanEvent::CommentAdded`]。
    pub fn add_comment(
        &self,
        plan_id: &PlanId,
        version: &PlanVersionId,
        anchor: PlanCommentAnchor,
        body: &str,
    ) -> Result<PlanEvent, PlanError> {
        if body.trim().is_empty() {
            return Err(PlanError::EmptyComment);
        }
        let mut inner = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        let state = &mut inner.state;
        check_plan_version(state, plan_id, version)?;
        if !state.steps().iter().any(|s| s.step_id == anchor.step_id) {
            return Err(PlanError::StepNotFound(anchor.step_id.clone()));
        }
        let event = PlanEvent::CommentAdded {
            plan_id: plan_id.clone(),
            version: version.clone(),
            anchor,
            body: body.to_owned(),
        };
        apply(state, &event);
        Ok(event)
    }

    /// 执行 gate：仅当该 Plan 版本已 [`PlanReviewStatus::Approved`] 才放行；
    /// 未创建 / plan_id 或 version 不匹配 / 任何未批准状态一律返回 `false`。
    /// 本 gate 只做只读判定，不授予任何写 / 执行能力。
    pub fn is_approved_for_execution(&self, plan_id: &PlanId, version: &PlanVersionId) -> bool {
        let inner = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        inner.state.plan_id().is_some_and(|p| p == plan_id)
            && inner.state.current_version().is_some_and(|v| v == version)
            && inner.state.review_status() == PlanReviewStatus::Approved
    }
}

/// 校验 `plan_id` / `version` 与当前聚合一致（版本化命令共用前置）。
fn check_plan_version(
    state: &PlanState,
    plan_id: &PlanId,
    version: &PlanVersionId,
) -> Result<(), PlanError> {
    let expected = state.plan_id().cloned().ok_or(PlanError::NotCreated)?;
    if &expected != plan_id {
        return Err(PlanError::PlanIdMismatch {
            expected,
            actual: plan_id.clone(),
        });
    }
    check_current_version(state, version)
}

/// 校验 `version` 等于当前版本。
fn check_current_version(state: &PlanState, version: &PlanVersionId) -> Result<(), PlanError> {
    let current = state
        .current_version()
        .cloned()
        .ok_or(PlanError::NotCreated)?;
    if &current != version {
        return Err(PlanError::VersionMismatch {
            expected: current,
            actual: version.clone(),
        });
    }
    Ok(())
}

fn build_steps(inner: &mut Inner, step_texts: Vec<String>) -> Vec<PlanStepSnapshot> {
    let mut steps = Vec::with_capacity(step_texts.len());
    for text in step_texts {
        let step_id = PlanStepId::new(format!("step_{}", inner.next_step));
        inner.next_step += 1;
        steps.push(PlanStepSnapshot {
            step_id,
            text,
            status: PlanStepStatus::Pending,
        });
    }
    steps
}

/// 从事件中已出现的 ID 推断计数器上限，避免重放后新发事件 ID 与历史碰撞。
fn seed_counters(events: &[&PlanEvent]) -> (u64, u64, u64) {
    let (mut plan, mut version, mut step) = (0u64, 0u64, 0u64);
    for event in events.iter().copied() {
        match event {
            PlanEvent::Created {
                plan_id,
                version: v,
                steps,
                ..
            } => {
                plan = plan.max(suffix(plan_id.as_str()));
                version = version.max(suffix(v.as_str()));
                for s in steps {
                    step = step.max(suffix(s.step_id.as_str()));
                }
            }
            PlanEvent::Replaced {
                version: v, steps, ..
            } => {
                version = version.max(suffix(v.as_str()));
                for s in steps {
                    step = step.max(suffix(s.step_id.as_str()));
                }
            }
            PlanEvent::Revised {
                version: v, steps, ..
            } => {
                version = version.max(suffix(v.as_str()));
                for s in steps {
                    step = step.max(suffix(s.step_id.as_str()));
                }
            }
            PlanEvent::ReviewRequested { version: v, .. }
            | PlanEvent::Approved { version: v, .. }
            | PlanEvent::Rejected { version: v, .. }
            | PlanEvent::CommentAdded { version: v, .. } => {
                version = version.max(suffix(v.as_str()));
            }
            PlanEvent::StepUpdated { .. } => {}
        }
    }
    (plan, version, step)
}

/// 解析字符串末尾的十进制数字（如 `plan_3` → 3）；无数字时返回 0。
fn suffix(value: &str) -> u64 {
    let mut acc: u64 = 0;
    let mut mult: u64 = 1;
    for c in value.chars().rev() {
        if let Some(d) = c.to_digit(10) {
            acc = acc.saturating_add((d as u64).saturating_mul(mult));
            mult = mult.saturating_mul(10);
        } else {
            break;
        }
    }
    acc
}
