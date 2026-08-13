//! 共享任务板命令校验（pure）。
//!
//! 复用 P12 [`orchestration::TaskId`] / [`orchestration::TaskState`] 图结构与
//! 依赖语义，但 owner 由「团队认领」决定（[`crate::event::BoardTask::owner`]）。
//! 本模块只做命令面校验并构造事件，事件落地与状态折叠由 [`crate::service`] /
//! [`crate::state::apply`] 负责——不写 run loop、不执行任务。

use agent_domain::AgentId;
use orchestration::{TaskId, TaskState};

use crate::error::TeamError;
use crate::event::BoardTask;
use crate::state::{is_legal_task_transition, TeamAggregate};

/// 构造一个新的（未认领）任务条目；校验依赖全部在板上。
pub fn build_task(
    state: &TeamAggregate,
    task_id: TaskId,
    poster: AgentId,
    description: String,
    depends_on: Vec<TaskId>,
    max_retries: u32,
) -> Result<BoardTask, TeamError> {
    if description.trim().is_empty() {
        return Err(TeamError::EmptyText);
    }
    for dep in &depends_on {
        if !state.board.contains_key(dep) {
            return Err(TeamError::UnknownDependency {
                task_id: task_id.clone(),
                dependency: dep.clone(),
            });
        }
    }
    let ready = depends_on.iter().all(|dep| {
        state
            .board
            .get(dep)
            .is_some_and(|d| d.state == TaskState::Completed)
    });
    Ok(BoardTask {
        task_id,
        poster,
        owner: None,
        description,
        depends_on,
        state: if ready {
            TaskState::Ready
        } else {
            TaskState::Created
        },
        retry_count: 0,
        max_retries,
    })
}

/// 校验认领：任务存在、未认领、依赖满足；返回就绪的任务条目引用。
pub fn validate_claim<'a>(
    state: &'a TeamAggregate,
    task_id: &TaskId,
) -> Result<&'a BoardTask, TeamError> {
    let task = state
        .board
        .get(task_id)
        .ok_or_else(|| TeamError::TaskNotFound(task_id.clone()))?;
    if task.owner.is_some() {
        return Err(TeamError::TaskAlreadyClaimed {
            task_id: task_id.clone(),
            owner: task.owner.clone().expect("checked Some"),
        });
    }
    if !state.dependencies_satisfied(task) {
        let missing: Vec<TaskId> = task
            .depends_on
            .iter()
            .filter(|dep| {
                !state
                    .board
                    .get(dep)
                    .is_some_and(|d| d.state == TaskState::Completed)
            })
            .cloned()
            .collect();
        return Err(TeamError::UnmetDependencies {
            task_id: task_id.clone(),
            missing,
        });
    }
    Ok(task)
}

/// 校验状态推进：任务存在、推进者**必须**是 owner、转移合法。
///
/// 严格 owner 语义：任何状态（含终态）只有认领者能推进；需要绕过的场景
/// （owner 失联、任务搁浅）走显式 Supervisor override
/// （[`validate_supervisor_advance`]，由 service 层校验 supervisor 角色）。
pub fn validate_advance(
    state: &TeamAggregate,
    task_id: &TaskId,
    by: &AgentId,
    to: TaskState,
) -> Result<TaskState, TeamError> {
    let task = state
        .board
        .get(task_id)
        .ok_or_else(|| TeamError::TaskNotFound(task_id.clone()))?;
    if task.owner.as_ref() != Some(by) {
        return Err(TeamError::NotTaskOwner {
            task_id: task_id.clone(),
            agent_id: (*by).clone(),
        });
    }
    validate_transition(state, task_id, to)
}

/// 显式 Supervisor override 的状态推进校验：跳过 owner 校验，仅检查转移合法。
///
/// 调用方（service 层）必须先确认 `by` 是 supervisor；本函数不重复角色校验，
/// 只保证任务存在且状态转移合法。
pub fn validate_supervisor_advance(
    state: &TeamAggregate,
    task_id: &TaskId,
    to: TaskState,
) -> Result<TaskState, TeamError> {
    validate_transition(state, task_id, to)
}

fn validate_transition(
    state: &TeamAggregate,
    task_id: &TaskId,
    to: TaskState,
) -> Result<TaskState, TeamError> {
    let task = state
        .board
        .get(task_id)
        .ok_or_else(|| TeamError::TaskNotFound(task_id.clone()))?;
    if !is_legal_task_transition(task.state, to) {
        return Err(TeamError::IllegalTaskTransition {
            task_id: task_id.clone(),
            from: task.state,
            to,
        });
    }
    Ok(to)
}

/// 校验释放：任务存在、释放者是 owner。
pub fn validate_release(
    state: &TeamAggregate,
    task_id: &TaskId,
    by: &AgentId,
) -> Result<(), TeamError> {
    let task = state
        .board
        .get(task_id)
        .ok_or_else(|| TeamError::TaskNotFound(task_id.clone()))?;
    if task.owner.as_ref() != Some(by) {
        return Err(TeamError::NotTaskOwner {
            task_id: task_id.clone(),
            agent_id: (*by).clone(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::TeamEvent;
    use crate::ids::TeamId;
    use crate::state::apply;
    use agent_domain::TenantId;

    fn agg() -> TeamAggregate {
        let mut s = TeamAggregate::default();
        apply(
            &mut s,
            TeamEvent::TeamCreated {
                team_id: TeamId::from("t1"),
                tenant_id: TenantId::from("ten"),
                supervisor: AgentId::from("sup"),
                name: "T".into(),
            },
        );
        s
    }

    #[test]
    fn build_task_rejects_unknown_dependency() {
        let s = agg();
        let err = build_task(
            &s,
            TaskId::new("tk1"),
            AgentId::from("sup"),
            "do".into(),
            vec![TaskId::new("ghost")],
            0,
        )
        .unwrap_err();
        assert!(matches!(err, TeamError::UnknownDependency { .. }));
    }

    #[test]
    fn claim_blocked_until_dependency_completed() {
        let mut s = agg();
        let dep = build_task(
            &s,
            TaskId::new("dep"),
            AgentId::from("sup"),
            "dep".into(),
            vec![],
            0,
        )
        .unwrap();
        apply(
            &mut s,
            TeamEvent::TaskPosted {
                team_id: TeamId::from("t1"),
                task: dep.clone(),
            },
        );
        let child = build_task(
            &s,
            TaskId::new("child"),
            AgentId::from("sup"),
            "child".into(),
            vec![TaskId::new("dep")],
            0,
        )
        .unwrap();
        // 依赖未完成：child 处于 Created。
        assert_eq!(child.state, TaskState::Created);
        // 张贴到板上后认领被拒（依赖未完成）。
        apply(
            &mut s,
            TeamEvent::TaskPosted {
                team_id: TeamId::from("t1"),
                task: child.clone(),
            },
        );
        let err = validate_claim(&s, &TaskId::new("child")).unwrap_err();
        assert!(matches!(err, TeamError::UnmetDependencies { .. }));

        // 完成依赖后，依赖满足的同类任务直接 Ready。
        apply(
            &mut s,
            TeamEvent::TaskClaimed {
                team_id: TeamId::from("t1"),
                task_id: TaskId::new("dep"),
                claimer: AgentId::from("w1"),
            },
        );
        apply(
            &mut s,
            TeamEvent::TaskAdvanced {
                team_id: TeamId::from("t1"),
                task_id: TaskId::new("dep"),
                state: TaskState::Completed,
            },
        );
        let rebuilt = build_task(
            &s,
            TaskId::new("child2"),
            AgentId::from("sup"),
            "child2".into(),
            vec![TaskId::new("dep")],
            0,
        )
        .unwrap();
        assert_eq!(rebuilt.state, TaskState::Ready);
    }
}
