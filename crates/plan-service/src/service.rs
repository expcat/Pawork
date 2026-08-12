//! 进程内内存 [`PlanService`]：命令面（状态机校验 + 产出 canonical 事件）与查询面。
//!
//! 命令方法在锁内完成校验、构造 [`PlanEvent`]、`apply` 到内部 state 后返回事件
//! 给调用方（由 session-store 封装为 `agent_events::AgentEvent::Plan` 持久化）。
//! 本服务**只读**：不暴露任何 spawn / exec / write / 文件 / 网络 API。

use parking_lot::Mutex;

use agent_domain::{
    PlanEvent, PlanId, PlanStepId, PlanStepSnapshot, PlanStepStatus, PlanVersionId,
};

use crate::error::PlanError;
use crate::snapshot::{PlanSnapshot, PlanVersionInfo};
use crate::state::{apply, is_legal_step_transition, replay, PlanState};

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

        let mut inner = self.inner.lock();
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

        let mut inner = self.inner.lock();
        let plan_id = inner.state.plan_id().cloned().ok_or(PlanError::NotCreated)?;
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
        let mut inner = self.inner.lock();
        let plan_id = inner.state.plan_id().cloned().ok_or(PlanError::NotCreated)?;
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
        self.inner.lock().state.snapshot()
    }

    /// 查询面：版本修订链（含当前版本，按创建顺序）。
    pub fn version_history(&self) -> Vec<PlanVersionInfo> {
        self.inner.lock().state.history().to_vec()
    }
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
                version: v,
                steps,
                ..
            } => {
                version = version.max(suffix(v.as_str()));
                for s in steps {
                    step = step.max(suffix(s.step_id.as_str()));
                }
            }
            PlanEvent::ReviewRequested { version: v, .. }
            | PlanEvent::Revised { version: v, .. }
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
