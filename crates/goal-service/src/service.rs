//! 进程内内存 [`GoalService`]：命令面（状态机校验 + 产出 canonical 事件）与查询面。
//!
//! 命令方法在锁内完成校验、构造 [`GoalEvent`]、`apply` 到内部 state 后返回事件
//! 给调用方（由 session-store 封装为 `agent_events::AgentEvent::Goal` 持久化）。
//! 成功标准区分 `Auto`（可机检，Agent 可自行满足）与 `Human`（需人确认：
//! [`GoalService::satisfy_criterion`] 拒绝、必须走
//! [`GoalService::mark_human_satisfied`] 显式人审入口）。

use std::collections::HashMap;

use parking_lot::Mutex;

use agent_domain::{CriterionKind, GoalEvent, GoalId, GoalStatus, SuccessCriterionSnapshot};

use crate::error::GoalError;
use crate::snapshot::GoalSnapshot;
use crate::state::{apply, recompute_progress, GoalState};

/// 创建 Goal 时的成功标准草稿（criterion id 由服务确定性生成）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CriterionDraft {
    pub description: String,
    pub kind: CriterionKind,
}

impl CriterionDraft {
    pub fn new(description: impl Into<String>, kind: CriterionKind) -> Self {
        Self {
            description: description.into(),
            kind,
        }
    }
}

/// 进程内内存 Goal service。
pub struct GoalService {
    inner: Mutex<Inner>,
}

struct Inner {
    goals: HashMap<GoalId, GoalState>,
    next_goal: u64,
    next_criterion: u64,
}

impl Default for GoalService {
    fn default() -> Self {
        Self {
            inner: Mutex::new(Inner {
                goals: HashMap::new(),
                next_goal: 1,
                next_criterion: 1,
            }),
        }
    }
}

impl GoalService {
    /// 创建一个空 service（无 Goal）。
    pub fn new() -> Self {
        Self::default()
    }

    /// 从 canonical 事件序列重放重建 service（崩溃恢复入口）。
    pub fn from_events<'a>(events: impl IntoIterator<Item = &'a GoalEvent>) -> Self {
        let mut goals = HashMap::new();
        let (mut next_goal, mut next_criterion) = (0u64, 0u64);
        for event in events {
            let goal_id = goal_id_of(event).clone();
            next_goal = next_goal.max(suffix(goal_id.as_str()));
            if let GoalEvent::Created { criteria, .. } = event {
                for criterion in criteria {
                    next_criterion = next_criterion.max(suffix(&criterion.criterion_id));
                }
            }
            let state = goals.entry(goal_id).or_default();
            apply(state, event);
        }
        Self {
            inner: Mutex::new(Inner {
                goals,
                next_goal: next_goal + 1,
                next_criterion: next_criterion + 1,
            }),
        }
    }

    /// 创建 Goal（长期锚点）；返回已 apply 的 [`GoalEvent::Created`]。
    ///
    /// criterion id 由服务生成（`criterion_<n>`），满足位初始为 `false`。
    pub fn create_goal(
        &self,
        title: &str,
        criteria: Vec<CriterionDraft>,
    ) -> Result<GoalEvent, GoalError> {
        if title.trim().is_empty() {
            return Err(GoalError::EmptyTitle);
        }
        if criteria.is_empty() {
            return Err(GoalError::EmptyCriteria);
        }
        if criteria.iter().any(|c| c.description.trim().is_empty()) {
            return Err(GoalError::EmptyCriterionDescription);
        }

        let mut inner = self.inner.lock();
        let goal_id = GoalId::new(format!("goal_{}", inner.next_goal));
        let mut snapshots = Vec::with_capacity(criteria.len());
        for draft in criteria {
            let criterion_id = format!("criterion_{}", inner.next_criterion);
            inner.next_criterion += 1;
            snapshots.push(SuccessCriterionSnapshot {
                criterion_id,
                description: draft.description,
                kind: draft.kind,
                satisfied: false,
            });
        }
        inner.next_goal += 1;

        let event = GoalEvent::Created {
            goal_id,
            title: title.to_owned(),
            criteria: snapshots,
        };
        apply_event(&mut inner, &event);
        Ok(event)
    }

    /// Agent 满足一个 `Auto` 成功标准；`Human` 项返回错误（Agent 不得自行
    /// 宣布人审项达成）。命中率变化时产出 [`GoalEvent::ProgressUpdated`]；
    /// 已满足的幂等调用返回空 Vec（无状态变化）。
    pub fn satisfy_criterion(
        &self,
        goal_id: &GoalId,
        criterion_id: &str,
    ) -> Result<Vec<GoalEvent>, GoalError> {
        let mut inner = self.inner.lock();
        let state = require_active(inner.goals.get_mut(goal_id), goal_id)?;
        let criterion_id = criterion_id.to_owned();
        {
            let criterion =
                state
                    .criterion_mut(&criterion_id)
                    .ok_or_else(|| GoalError::CriterionNotFound {
                        goal_id: goal_id.clone(),
                        criterion_id: criterion_id.clone(),
                    })?;
            if criterion.kind == CriterionKind::Human {
                return Err(GoalError::HumanCriterionNotAutoSatisfiable(criterion_id));
            }
            if criterion.satisfied {
                return Ok(vec![]);
            }
        }
        // 单项满足位事件化（可重放，ADR-016），再刷新命中率进度。
        let criterion_satisfied = GoalEvent::CriterionSatisfied {
            goal_id: goal_id.clone(),
            criterion_id,
        };
        apply(state, &criterion_satisfied);
        let mut events = vec![criterion_satisfied];
        events.extend(progress_events(goal_id, state));
        Ok(events)
    }

    /// 显式人审入口：人确认后满足任意 kind 的成功标准（`Human` 项只能经此
    /// 路径达成）。已满足的幂等调用返回空 Vec。
    pub fn mark_human_satisfied(
        &self,
        goal_id: &GoalId,
        criterion_id: &str,
    ) -> Result<Vec<GoalEvent>, GoalError> {
        let mut inner = self.inner.lock();
        let state = require_active(inner.goals.get_mut(goal_id), goal_id)?;
        let criterion_id = criterion_id.to_owned();
        {
            let criterion =
                state
                    .criterion_mut(&criterion_id)
                    .ok_or_else(|| GoalError::CriterionNotFound {
                        goal_id: goal_id.clone(),
                        criterion_id: criterion_id.clone(),
                    })?;
            if criterion.satisfied {
                return Ok(vec![]);
            }
        }
        // 单项满足位事件化（可重放，ADR-016），再刷新命中率进度。
        let criterion_satisfied = GoalEvent::CriterionSatisfied {
            goal_id: goal_id.clone(),
            criterion_id,
        };
        apply(state, &criterion_satisfied);
        let mut events = vec![criterion_satisfied];
        events.extend(progress_events(goal_id, state));
        Ok(events)
    }

    /// 暂停 Goal（`Active → Paused`）。
    pub fn pause(&self, goal_id: &GoalId) -> Result<GoalEvent, GoalError> {
        let mut inner = self.inner.lock();
        let state = require_exists(inner.goals.get_mut(goal_id), goal_id)?;
        let from = state.status();
        if from != GoalStatus::Active {
            return Err(GoalError::IllegalStatusTransition {
                from,
                to: GoalStatus::Paused,
            });
        }
        let event = GoalEvent::Paused {
            goal_id: goal_id.clone(),
        };
        apply(state, &event);
        Ok(event)
    }

    /// 恢复 Goal（`Paused → Active`）。`remaining_budget_tokens` 必须是调用方
    /// **复算后**的剩余预算，事件与状态保存新值而非沿用旧值。
    pub fn resume(
        &self,
        goal_id: &GoalId,
        remaining_budget_tokens: u64,
    ) -> Result<GoalEvent, GoalError> {
        let mut inner = self.inner.lock();
        let state = require_exists(inner.goals.get_mut(goal_id), goal_id)?;
        let from = state.status();
        if from != GoalStatus::Paused {
            return Err(GoalError::IllegalStatusTransition {
                from,
                to: GoalStatus::Active,
            });
        }
        let event = GoalEvent::Resumed {
            goal_id: goal_id.clone(),
            remaining_budget_tokens,
        };
        apply(state, &event);
        Ok(event)
    }

    /// 运行中转向：注入修正方向 / 约束 / 新优先级，存入可回溯的 steering history。
    pub fn steer(&self, goal_id: &GoalId, input: &str) -> Result<GoalEvent, GoalError> {
        if input.trim().is_empty() {
            return Err(GoalError::EmptySteerInput);
        }
        let mut inner = self.inner.lock();
        let state = require_active(inner.goals.get_mut(goal_id), goal_id)?;
        let event = GoalEvent::Steered {
            goal_id: goal_id.clone(),
            input: input.to_owned(),
        };
        apply(state, &event);
        Ok(event)
    }

    /// 宣布 Goal 达成（`Active → Achieved`）。
    pub fn achieve(&self, goal_id: &GoalId) -> Result<GoalEvent, GoalError> {
        let mut inner = self.inner.lock();
        let state = require_exists(inner.goals.get_mut(goal_id), goal_id)?;
        let from = state.status();
        if from != GoalStatus::Active {
            return Err(GoalError::IllegalStatusTransition {
                from,
                to: GoalStatus::Achieved,
            });
        }
        let event = GoalEvent::Achieved {
            goal_id: goal_id.clone(),
        };
        apply(state, &event);
        Ok(event)
    }

    /// 放弃 Goal（`Active | Paused → Abandoned`）。
    pub fn abandon(&self, goal_id: &GoalId, reason: &str) -> Result<GoalEvent, GoalError> {
        let mut inner = self.inner.lock();
        let state = require_exists(inner.goals.get_mut(goal_id), goal_id)?;
        let from = state.status();
        if !matches!(from, GoalStatus::Active | GoalStatus::Paused) {
            return Err(GoalError::IllegalStatusTransition {
                from,
                to: GoalStatus::Abandoned,
            });
        }
        let event = GoalEvent::Abandoned {
            goal_id: goal_id.clone(),
            reason: reason.to_owned(),
        };
        apply(state, &event);
        Ok(event)
    }

    /// 查询面：Goal 只读快照；不存在时返回 `None`。
    pub fn goal_snapshot(&self, goal_id: &GoalId) -> Option<GoalSnapshot> {
        self.inner
            .lock()
            .goals
            .get(goal_id)
            .and_then(GoalState::snapshot)
    }

    /// 查询面：全部 Goal 快照（按 goal_id 排序，保证确定性）。
    pub fn goals(&self) -> Vec<GoalSnapshot> {
        let inner = self.inner.lock();
        let mut snapshots: Vec<GoalSnapshot> = inner
            .goals
            .values()
            .filter_map(GoalState::snapshot)
            .collect();
        snapshots.sort_by(|a, b| a.goal_id.cmp(&b.goal_id));
        snapshots
    }
}

fn require_exists<'a>(
    state: Option<&'a mut GoalState>,
    goal_id: &GoalId,
) -> Result<&'a mut GoalState, GoalError> {
    state.ok_or_else(|| GoalError::GoalNotFound(goal_id.clone()))
}

fn require_active<'a>(
    state: Option<&'a mut GoalState>,
    goal_id: &GoalId,
) -> Result<&'a mut GoalState, GoalError> {
    let state = require_exists(state, goal_id)?;
    if state.status() != GoalStatus::Active {
        return Err(GoalError::GoalNotActive(goal_id.clone()));
    }
    Ok(state)
}

/// 把事件折叠进对应 Goal（事件视为已校验事实，缺失 Goal 时防御性忽略）。
fn apply_event(inner: &mut Inner, event: &GoalEvent) {
    let goal_id = goal_id_of(event).clone();
    apply(inner.goals.entry(goal_id).or_default(), event);
}

/// 满足位变化后复算进度；命中率变化时产出并应用 [`GoalEvent::ProgressUpdated`]。
fn progress_events(goal_id: &GoalId, state: &mut GoalState) -> Vec<GoalEvent> {
    let progress = recompute_progress(state.criteria());
    if (progress - state.progress()).abs() > f64::EPSILON {
        let event = GoalEvent::ProgressUpdated {
            goal_id: goal_id.clone(),
            progress,
        };
        apply(state, &event);
        vec![event]
    } else {
        vec![]
    }
}

fn goal_id_of(event: &GoalEvent) -> &GoalId {
    match event {
        GoalEvent::Created { goal_id, .. }
        | GoalEvent::ProgressUpdated { goal_id, .. }
        | GoalEvent::CriterionSatisfied { goal_id, .. }
        | GoalEvent::Paused { goal_id }
        | GoalEvent::Resumed { goal_id, .. }
        | GoalEvent::Steered { goal_id, .. }
        | GoalEvent::Achieved { goal_id }
        | GoalEvent::Abandoned { goal_id, .. } => goal_id,
    }
}

/// 解析字符串末尾的十进制数字（如 `goal_3` → 3）；无数字时返回 0。
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::replay;
    use agent_domain::GoalStatus;
    use agent_events::AgentEvent;

    fn auto(description: &str) -> CriterionDraft {
        CriterionDraft::new(description, CriterionKind::Auto)
    }

    fn human(description: &str) -> CriterionDraft {
        CriterionDraft::new(description, CriterionKind::Human)
    }

    fn sample_service() -> (GoalService, GoalId, Vec<String>) {
        let service = GoalService::new();
        let created = service
            .create_goal(
                "完成 P16-3",
                vec![auto("代码通过测试"), auto("文档已更新"), human("用户验收")],
            )
            .unwrap();
        let GoalEvent::Created {
            goal_id, criteria, ..
        } = created
        else {
            panic!("expected Created");
        };
        let ids: Vec<String> = criteria.into_iter().map(|c| c.criterion_id).collect();
        (service, goal_id, ids)
    }

    fn criterion_id(snapshot: &GoalSnapshot, index: usize) -> String {
        snapshot.criteria[index].criterion_id.clone()
    }

    #[test]
    fn create_goal_produces_active_snapshot() {
        let (service, goal_id, ids) = sample_service();
        let snapshot = service.goal_snapshot(&goal_id).unwrap();
        assert_eq!(snapshot.title, "完成 P16-3");
        assert_eq!(snapshot.status, GoalStatus::Active);
        assert_eq!(snapshot.progress, 0.0);
        assert_eq!(snapshot.criteria.len(), 3);
        assert_eq!(ids, vec!["criterion_1", "criterion_2", "criterion_3"]);
        assert!(snapshot.steering_history.is_empty());
        assert_eq!(snapshot.remaining_budget_tokens, None);
        assert!(snapshot.criteria.iter().all(|c| !c.satisfied));
    }

    #[test]
    fn auto_criterion_satisfiable_updates_progress() {
        let (service, goal_id, ids) = sample_service();
        let events = service.satisfy_criterion(&goal_id, &ids[0]).unwrap();
        assert_eq!(
            events,
            vec![
                GoalEvent::CriterionSatisfied {
                    goal_id: goal_id.clone(),
                    criterion_id: ids[0].clone(),
                },
                GoalEvent::ProgressUpdated {
                    goal_id: goal_id.clone(),
                    progress: 1.0 / 3.0,
                },
            ]
        );
        let snapshot = service.goal_snapshot(&goal_id).unwrap();
        assert!(snapshot.criteria[0].satisfied);
        assert!((snapshot.progress - 1.0 / 3.0).abs() < f64::EPSILON);

        // 已满足的幂等调用不再产生事件。
        assert!(service
            .satisfy_criterion(&goal_id, &ids[0])
            .unwrap()
            .is_empty());
    }

    #[test]
    fn human_criterion_cannot_be_satisfied_by_agent() {
        let (service, goal_id, ids) = sample_service();
        let err = service.satisfy_criterion(&goal_id, &ids[2]).unwrap_err();
        assert!(matches!(
            err,
            GoalError::HumanCriterionNotAutoSatisfiable(id) if id == ids[2]
        ));
        // 状态未被污染：Human 项保持未满足，进度不变。
        let snapshot = service.goal_snapshot(&goal_id).unwrap();
        assert!(!snapshot.criteria[2].satisfied);
        assert_eq!(snapshot.progress, 0.0);
    }

    #[test]
    fn human_criterion_satisfied_via_explicit_human_entry() {
        let (service, goal_id, ids) = sample_service();
        let events = service.mark_human_satisfied(&goal_id, &ids[2]).unwrap();
        assert_eq!(events.len(), 2);
        let GoalEvent::CriterionSatisfied { criterion_id, .. } = &events[0] else {
            panic!("expected CriterionSatisfied");
        };
        assert_eq!(criterion_id, &ids[2]);
        let GoalEvent::ProgressUpdated { progress, .. } = &events[1] else {
            panic!("expected ProgressUpdated");
        };
        assert!((*progress - 1.0 / 3.0).abs() < f64::EPSILON);
        let snapshot = service.goal_snapshot(&goal_id).unwrap();
        assert!(snapshot.criteria[2].satisfied);
        // 人审入口幂等。
        assert!(service
            .mark_human_satisfied(&goal_id, &ids[2])
            .unwrap()
            .is_empty());
    }

    #[test]
    fn progress_is_hit_rate_of_criteria() {
        let (service, goal_id, ids) = sample_service();
        service.satisfy_criterion(&goal_id, &ids[0]).unwrap();
        service.satisfy_criterion(&goal_id, &ids[1]).unwrap();
        let snapshot = service.goal_snapshot(&goal_id).unwrap();
        assert!((snapshot.progress - 2.0 / 3.0).abs() < f64::EPSILON);
        service.mark_human_satisfied(&goal_id, &ids[2]).unwrap();
        let snapshot = service.goal_snapshot(&goal_id).unwrap();
        assert!((snapshot.progress - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn pause_resume_preserves_state_and_resume_recomputes_budget() {
        let (service, goal_id, ids) = sample_service();
        service.satisfy_criterion(&goal_id, &ids[0]).unwrap();

        let paused = service.pause(&goal_id).unwrap();
        assert_eq!(
            paused,
            GoalEvent::Paused {
                goal_id: goal_id.clone()
            }
        );
        let snapshot = service.goal_snapshot(&goal_id).unwrap();
        assert_eq!(snapshot.status, GoalStatus::Paused);
        // 暂停保留进度等状态。
        assert!((snapshot.progress - 1.0 / 3.0).abs() < f64::EPSILON);

        // 重复 pause 非法。
        assert!(matches!(
            service.pause(&goal_id).unwrap_err(),
            GoalError::IllegalStatusTransition {
                from: GoalStatus::Paused,
                to: GoalStatus::Paused
            }
        ));
        // 暂停期间不可满足标准 / 转向。
        assert!(matches!(
            service.satisfy_criterion(&goal_id, &ids[1]).unwrap_err(),
            GoalError::GoalNotActive(_)
        ));
        assert!(matches!(
            service.steer(&goal_id, "新方向").unwrap_err(),
            GoalError::GoalNotActive(_)
        ));

        // resume 注入复算后的新预算，覆盖旧值。
        let resumed = service.resume(&goal_id, 42_000).unwrap();
        assert_eq!(
            resumed,
            GoalEvent::Resumed {
                goal_id: goal_id.clone(),
                remaining_budget_tokens: 42_000
            }
        );
        let snapshot = service.goal_snapshot(&goal_id).unwrap();
        assert_eq!(snapshot.status, GoalStatus::Active);
        assert_eq!(snapshot.remaining_budget_tokens, Some(42_000));
        assert!((snapshot.progress - 1.0 / 3.0).abs() < f64::EPSILON);

        // 再次暂停后以新的复算值 resume：断言保存的是新值而非旧值。
        service.pause(&goal_id).unwrap();
        service.resume(&goal_id, 7_777).unwrap();
        let snapshot = service.goal_snapshot(&goal_id).unwrap();
        assert_eq!(snapshot.remaining_budget_tokens, Some(7_777));

        // 非 Paused 状态 resume 非法。
        assert!(matches!(
            service.resume(&goal_id, 1).unwrap_err(),
            GoalError::IllegalStatusTransition {
                from: GoalStatus::Active,
                to: GoalStatus::Active
            }
        ));
    }

    #[test]
    fn steering_is_recorded_and_replayable() {
        let (service, goal_id, _ids) = sample_service();
        let first = service.steer(&goal_id, "优先保证正确性").unwrap();
        assert_eq!(
            first,
            GoalEvent::Steered {
                goal_id: goal_id.clone(),
                input: "优先保证正确性".to_owned()
            }
        );
        service.steer(&goal_id, "取消 UI 部分").unwrap();
        let snapshot = service.goal_snapshot(&goal_id).unwrap();
        assert_eq!(
            snapshot.steering_history,
            vec!["优先保证正确性", "取消 UI 部分"]
        );
        assert!(matches!(
            service.steer(&goal_id, "  ").unwrap_err(),
            GoalError::EmptySteerInput
        ));
    }

    #[test]
    fn terminal_statuses_reject_all_commands() {
        let (service, goal_id, ids) = sample_service();
        service.achieve(&goal_id).unwrap();
        assert_eq!(
            service.goal_snapshot(&goal_id).unwrap().status,
            GoalStatus::Achieved
        );
        assert!(matches!(
            service.achieve(&goal_id).unwrap_err(),
            GoalError::IllegalStatusTransition {
                from: GoalStatus::Achieved,
                to: GoalStatus::Achieved
            }
        ));
        assert!(matches!(
            service.satisfy_criterion(&goal_id, &ids[0]).unwrap_err(),
            GoalError::GoalNotActive(_)
        ));
        assert!(matches!(
            service.mark_human_satisfied(&goal_id, &ids[2]).unwrap_err(),
            GoalError::GoalNotActive(_)
        ));
        assert!(matches!(
            service.pause(&goal_id).unwrap_err(),
            GoalError::IllegalStatusTransition {
                from: GoalStatus::Achieved,
                to: GoalStatus::Paused
            }
        ));
        assert!(matches!(
            service.resume(&goal_id, 1).unwrap_err(),
            GoalError::IllegalStatusTransition {
                from: GoalStatus::Achieved,
                to: GoalStatus::Active
            }
        ));

        let (service, goal_id, _ids) = sample_service();
        service.abandon(&goal_id, "范围变更").unwrap();
        assert_eq!(
            service.goal_snapshot(&goal_id).unwrap().status,
            GoalStatus::Abandoned
        );
        // 终态不可再 achieve / abandon。
        assert!(matches!(
            service.achieve(&goal_id).unwrap_err(),
            GoalError::IllegalStatusTransition {
                from: GoalStatus::Abandoned,
                to: GoalStatus::Achieved
            }
        ));
        assert!(matches!(
            service.abandon(&goal_id, "再放弃一次").unwrap_err(),
            GoalError::IllegalStatusTransition {
                from: GoalStatus::Abandoned,
                to: GoalStatus::Abandoned
            }
        ));
        // Paused 状态可以 abandon。
        let (service, goal_id, _ids) = sample_service();
        service.pause(&goal_id).unwrap();
        service.abandon(&goal_id, "暂停后放弃").unwrap();
        assert_eq!(
            service.goal_snapshot(&goal_id).unwrap().status,
            GoalStatus::Abandoned
        );
    }

    #[test]
    fn command_validation_errors() {
        let service = GoalService::new();
        assert!(matches!(
            service.create_goal("  ", vec![auto("x")]).unwrap_err(),
            GoalError::EmptyTitle
        ));
        assert!(matches!(
            service.create_goal("t", vec![]).unwrap_err(),
            GoalError::EmptyCriteria
        ));
        assert!(matches!(
            service.create_goal("t", vec![auto("  ")]).unwrap_err(),
            GoalError::EmptyCriterionDescription
        ));

        let (service, goal_id, ids) = sample_service();
        assert!(service.goal_snapshot(&GoalId::new("goal_999")).is_none());
        assert!(matches!(
            service.pause(&GoalId::new("goal_999")).unwrap_err(),
            GoalError::GoalNotFound(_)
        ));
        assert!(matches!(
            service
                .satisfy_criterion(&goal_id, "criterion_999")
                .unwrap_err(),
            GoalError::CriterionNotFound { .. }
        ));
        assert!(matches!(
            service
                .mark_human_satisfied(&goal_id, "criterion_999")
                .unwrap_err(),
            GoalError::CriterionNotFound { .. }
        ));
        assert_eq!(ids.len(), 3);
    }

    #[test]
    fn multiple_goals_are_isolated() {
        let service = GoalService::new();
        let created_a = service.create_goal("目标 A", vec![auto("a")]).unwrap();
        let created_b = service
            .create_goal("目标 B", vec![auto("b"), human("b 审")])
            .unwrap();
        let GoalEvent::Created { goal_id: a, .. } = created_a else {
            panic!()
        };
        let GoalEvent::Created { goal_id: b, .. } = created_b else {
            panic!()
        };
        assert_ne!(a, b);
        let snapshot_b = service.goal_snapshot(&b).unwrap();
        let criterion_b = criterion_id(&snapshot_b, 0);
        service.satisfy_criterion(&a, "criterion_1").unwrap();
        service.satisfy_criterion(&b, &criterion_b).unwrap();
        // 两个 Goal 独立推进，id 互不串扰。
        assert!((service.goal_snapshot(&a).unwrap().progress - 1.0).abs() < f64::EPSILON);
        assert!((service.goal_snapshot(&b).unwrap().progress - 0.5).abs() < f64::EPSILON);
        assert_eq!(service.goals().len(), 2);
    }

    #[test]
    fn replay_rebuilds_reconstructible_state_identical_to_stepwise_apply() {
        let (service, _goal_id, _ids) = sample_service();
        let mut events = vec![service
            .create_goal("重放目标", vec![auto("x"), human("h")])
            .unwrap()];
        let GoalEvent::Created {
            goal_id: g,
            criteria,
            ..
        } = &events[0]
        else {
            panic!()
        };
        let x = criteria[0].criterion_id.clone();
        let h = criteria[1].criterion_id.clone();
        let g = g.clone();
        events.extend(service.satisfy_criterion(&g, &x).unwrap());
        events.extend(service.mark_human_satisfied(&g, &h).unwrap());
        events.push(service.steer(&g, "转向：缩小范围").unwrap());
        events.push(service.pause(&g).unwrap());
        events.push(service.resume(&g, 3_000).unwrap());

        // 逐步 apply 与一次 replay 一致。
        let mut stepwise = GoalState::default();
        for event in &events {
            apply(&mut stepwise, event);
        }
        let rebuilt = replay(events.iter());
        assert_eq!(stepwise, rebuilt);

        // service 级重放：criteria 满足位 / 状态机 / 进度 / 转向 / 预算全部从事件流
        // 完整恢复（ADR-016：live→fresh snapshot 必须完整相等）。
        let replayed = GoalService::from_events(events.iter());
        let live = service.goal_snapshot(&g).unwrap();
        let restored = replayed.goal_snapshot(&g).unwrap();
        assert_eq!(restored, live, "live→fresh replay 必须完整 snapshot 相等");

        // 重放后新事件 id 不与历史碰撞。
        let new_event = replayed.create_goal("新目标", vec![auto("n")]).unwrap();
        let GoalEvent::Created { goal_id: new_g, .. } = new_event else {
            panic!()
        };
        assert_ne!(new_g, g);
        assert_eq!(new_g, GoalId::new("goal_3"));
    }

    #[test]
    fn progress_events_are_clamped_and_traceable() {
        let mut state = GoalState::default();
        // 防御性折叠：越界 progress 收敛到 [0,1]。
        apply(
            &mut state,
            &GoalEvent::ProgressUpdated {
                goal_id: GoalId::new("goal_1"),
                progress: 1.5,
            },
        );
        assert!((state.progress() - 1.0).abs() < f64::EPSILON);
        apply(
            &mut state,
            &GoalEvent::ProgressUpdated {
                goal_id: GoalId::new("goal_1"),
                progress: -0.2,
            },
        );
        assert!((state.progress() - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn goal_events_wrap_into_agent_event_and_round_trip() {
        let (service, goal_id, ids) = sample_service();
        let event = service.satisfy_criterion(&goal_id, &ids[0]).unwrap()[0].clone();
        let envelope = AgentEvent::Goal(event);
        let json = serde_json::to_string(&envelope).unwrap();
        let decoded: AgentEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, envelope);
        // snake_case 序列化契约。
        assert!(json.contains("\"kind\":\"criterion_satisfied\""));

        let snapshot = service.goal_snapshot(&goal_id).unwrap();
        let json = serde_json::to_string(&snapshot).unwrap();
        let decoded: GoalSnapshot = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, snapshot);
        assert!(json.contains("\"status\":\"active\""));
    }
}

/// 旧流 serde 兼容：历史持久化的 GoalEvent 不含 `criterion_satisfied` 变体，
/// 新增变体不破坏反序列化；SuccessCriterionSnapshot.satisfied 已有 serde default。
#[cfg(test)]
mod serde_compat_tests {
    use super::*;

    #[test]
    fn legacy_created_event_round_trips_without_new_variant() {
        // 历史 Created 事件（无 criterion 满足事件语义）仍可反序列化。
        let legacy = r#"{"kind":"created","goal_id":"goal_1","title":"t","criteria":[{"criterion_id":"c","description":"d","kind":"auto","satisfied":false}]}"#;
        let event: GoalEvent = serde_json::from_str(legacy).unwrap();
        let GoalEvent::Created {
            goal_id, criteria, ..
        } = event
        else {
            panic!("expected Created");
        };
        assert_eq!(goal_id, GoalId::new("goal_1"));
        assert_eq!(criteria.len(), 1);
        // 旧事件缺 satisfied 字段时默认 false（serde 兼容）。
        let legacy_no_sat = r#"{"kind":"created","goal_id":"goal_1","title":"t","criteria":[{"criterion_id":"c","description":"d","kind":"auto"}]}"#;
        let event: GoalEvent = serde_json::from_str(legacy_no_sat).unwrap();
        let GoalEvent::Created { criteria, .. } = event else {
            panic!("expected Created");
        };
        assert!(!criteria[0].satisfied);
    }

    #[test]
    fn criterion_satisfied_serializes_snake_case_and_round_trips() {
        let sat = GoalEvent::CriterionSatisfied {
            goal_id: GoalId::new("goal_7"),
            criterion_id: "criterion_2".to_string(),
        };
        let json = serde_json::to_string(&sat).unwrap();
        assert!(json.contains("\"kind\":\"criterion_satisfied\""));
        assert_eq!(serde_json::from_str::<GoalEvent>(&json).unwrap(), sat);
    }
}
