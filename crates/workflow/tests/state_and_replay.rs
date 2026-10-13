//! 状态机 / 事件日志 / 快照 / 重放 / 取消传播 的定向测试。

use pawork_domain::{BackgroundTaskId, CancellationToken, TaskEvent, TaskKind, TaskStatus};
use pawork_workflow::task::{is_terminal_status, TaskManager, TaskManagerError, TaskManagerState};

fn manager() -> TaskManager {
    TaskManager::new()
}

#[test]
fn four_kinds_register_and_query() {
    let mgr = manager();
    let process = mgr.register(TaskKind::Process, None).unwrap();
    let agent = mgr.register(TaskKind::Agent, None).unwrap();
    let monitor = mgr
        .register(TaskKind::Monitor, Some(process.clone()))
        .unwrap();
    let automation = mgr.register(TaskKind::Automation, None).unwrap();

    assert_eq!(mgr.tasks().len(), 4);
    assert_eq!(mgr.task(&process).unwrap().task_kind, TaskKind::Process);
    assert_eq!(mgr.task(&agent).unwrap().task_kind, TaskKind::Agent);
    assert_eq!(mgr.task(&monitor).unwrap().task_kind, TaskKind::Monitor);
    assert_eq!(
        mgr.task(&monitor).unwrap().parent_task_id.as_ref(),
        Some(&process)
    );
    assert_eq!(
        mgr.task(&automation).unwrap().task_kind,
        TaskKind::Automation
    );
    for id in [&process, &agent, &monitor, &automation] {
        assert_eq!(mgr.task(id).unwrap().status, TaskStatus::Queued);
    }
}

#[test]
fn legal_lifecycle_emits_events() {
    let mgr = manager();
    let id = mgr.register(TaskKind::Agent, None).unwrap();
    assert!(mgr.event_log().is_empty(), "queued 注册不发事件");

    let started = mgr.start(&id).unwrap();
    assert!(matches!(
        started,
        TaskEvent::Started {
            task_id,
            task_kind: TaskKind::Agent,
            parent_task_id: None,
        } if task_id == id
    ));
    assert_eq!(mgr.task(&id).unwrap().status, TaskStatus::Running);

    let finished = mgr
        .finish(&id, TaskStatus::Completed, Some("done".into()))
        .unwrap();
    assert!(matches!(
        finished,
        TaskEvent::Finished {
            task_id,
            status: TaskStatus::Completed,
            detail: Some(_),
        } if task_id == id
    ));
    assert_eq!(mgr.task(&id).unwrap().status, TaskStatus::Completed);
    assert_eq!(mgr.event_log().len(), 2);
}

#[test]
fn illegal_transitions_rejected() {
    let mgr = manager();
    let id = mgr.register(TaskKind::Process, None).unwrap();

    assert!(matches!(
        mgr.finish(&id, TaskStatus::Completed, None),
        Err(TaskManagerError::InvalidTransition { .. })
    ));

    mgr.start(&id).unwrap();
    assert!(matches!(
        mgr.start(&id),
        Err(TaskManagerError::InvalidTransition { .. })
    ));

    assert!(matches!(
        mgr.finish(&id, TaskStatus::Canceled, None),
        Err(TaskManagerError::InvalidFinishedStatus(_))
    ));
    mgr.finish(&id, TaskStatus::Completed, None).unwrap();
    assert!(matches!(
        mgr.finish(&id, TaskStatus::Completed, None),
        Err(TaskManagerError::InvalidTransition { .. })
    ));

    let ghost = BackgroundTaskId::new("task_999");
    assert!(matches!(
        mgr.start(&ghost),
        Err(TaskManagerError::UnknownTask(_))
    ));
    assert!(matches!(
        mgr.cancel(&ghost),
        Err(TaskManagerError::UnknownTask(_))
    ));
    assert!(mgr.task(&ghost).is_none());
    assert!(matches!(
        mgr.register(TaskKind::Agent, Some(ghost)),
        Err(TaskManagerError::UnknownParent(_))
    ));
}

#[test]
fn snapshot_and_replay_rebuild_view() {
    let mgr = manager();
    let parent = mgr.register(TaskKind::Process, None).unwrap();
    let child = mgr.register(TaskKind::Agent, Some(parent.clone())).unwrap();
    mgr.start(&parent).unwrap();
    mgr.start(&child).unwrap();
    mgr.finish(&parent, TaskStatus::Completed, Some("ok".into()))
        .unwrap();

    let snapshot = mgr.snapshot();
    assert_eq!(snapshot.tasks.len(), 2);
    assert_eq!(snapshot.events.len(), 3);

    // 事件日志可 JSON 序列化 / 反序列化（持久化 + 重放输入）。
    let json = serde_json::to_string(&snapshot).unwrap();
    let restored: pawork_workflow::task::TaskManagerSnapshot = serde_json::from_str(&json).unwrap();
    assert_eq!(restored, snapshot);

    // 全新 manager 重放后重建出相同视图。
    let mgr2 = manager();
    let count = mgr2.replay(restored.events).unwrap();
    assert_eq!(count, 3);
    assert_eq!(mgr2.snapshot(), snapshot);
    assert_eq!(mgr2.task(&parent).unwrap().status, TaskStatus::Completed);
    assert_eq!(mgr2.task(&child).unwrap().status, TaskStatus::Running);
    assert_eq!(
        mgr2.task(&child).unwrap().parent_task_id.as_ref(),
        Some(&parent)
    );
}

#[test]
fn pure_state_apply_folds_events() {
    let mut state = TaskManagerState::new();
    let id = BackgroundTaskId::new("task_0");
    state
        .apply(&TaskEvent::Started {
            task_id: id.clone(),
            task_kind: TaskKind::Monitor,
            parent_task_id: None,
        })
        .unwrap();
    assert_eq!(state.task(&id).unwrap().status, TaskStatus::Running);

    state
        .apply(&TaskEvent::Suspended {
            task_id: id.clone(),
        })
        .unwrap();
    assert_eq!(state.task(&id).unwrap().status, TaskStatus::Suspended);
    state
        .apply(&TaskEvent::Resumed {
            task_id: id.clone(),
        })
        .unwrap();
    assert!(state
        .apply(&TaskEvent::Resumed {
            task_id: id.clone()
        })
        .is_err());

    // 未知任务与非法终态拒绝。
    let ghost = BackgroundTaskId::new("task_9");
    assert!(state
        .apply(&TaskEvent::Suspended {
            task_id: ghost.clone()
        })
        .is_err());
    assert!(state
        .apply(&TaskEvent::Finished {
            task_id: ghost,
            status: TaskStatus::Completed,
            detail: None,
        })
        .is_err());
    assert!(state
        .apply(&TaskEvent::Finished {
            task_id: id.clone(),
            status: TaskStatus::Queued,
            detail: None,
        })
        .is_err());
}

#[test]
fn cancel_propagates_to_descendants_without_orphans() {
    let mgr = manager();
    let parent = mgr.register(TaskKind::Process, None).unwrap();
    let child_a = mgr.register(TaskKind::Agent, Some(parent.clone())).unwrap();
    let child_b = mgr
        .register(TaskKind::Automation, Some(parent.clone()))
        .unwrap();
    let grandchild = mgr
        .register(TaskKind::Monitor, Some(child_a.clone()))
        .unwrap();
    let unrelated = mgr.register(TaskKind::Agent, None).unwrap();
    for id in [&parent, &child_a, &child_b, &grandchild, &unrelated] {
        mgr.start(id).unwrap();
    }

    let events = mgr.cancel(&parent).unwrap();
    assert_eq!(events.len(), 4, "parent + 3 个后代");
    assert!(events.iter().all(|e| matches!(
        e,
        TaskEvent::Finished {
            status: TaskStatus::Canceled,
            ..
        }
    )));

    for id in [&parent, &child_a, &child_b, &grandchild] {
        let task = mgr.task(id).unwrap();
        assert_eq!(task.status, TaskStatus::Canceled);
        assert!(is_terminal_status(task.status));
    }
    assert_eq!(mgr.task(&unrelated).unwrap().status, TaskStatus::Running);
    // 无孤儿：取消树内不存在任何 active 任务（unrelated 不受影响）。
    let tree_ids = [
        parent.clone(),
        child_a.clone(),
        child_b.clone(),
        grandchild.clone(),
    ];
    assert!(mgr
        .tasks()
        .iter()
        .filter(|t| tree_ids.contains(&t.task_id))
        .all(|t| is_terminal_status(t.status)));

    // 取消产生的全部事件可重放重建同一视图。
    let snapshot = mgr.snapshot();
    let mgr2 = manager();
    mgr2.replay(snapshot.events.clone()).unwrap();
    assert_eq!(mgr2.snapshot(), snapshot);
}

#[test]
fn cancel_skips_terminal_and_removes_queued() {
    let mgr = manager();
    let parent = mgr.register(TaskKind::Process, None).unwrap();
    let done = mgr.register(TaskKind::Agent, Some(parent.clone())).unwrap();
    let queued = mgr
        .register(TaskKind::Monitor, Some(parent.clone()))
        .unwrap();
    mgr.start(&parent).unwrap();
    mgr.start(&done).unwrap();
    mgr.finish(&done, TaskStatus::Failed, Some("already failed".into()))
        .unwrap();

    let events = mgr.cancel(&parent).unwrap();
    // 仅 parent 发出 Finished{Canceled}；done 已终态跳过；queued 静默移除。
    assert_eq!(events.len(), 1);
    assert_eq!(mgr.task(&parent).unwrap().status, TaskStatus::Canceled);
    assert_eq!(mgr.task(&done).unwrap().status, TaskStatus::Failed);
    assert!(mgr.task(&queued).is_none(), "queued 任务取消后静默移除");
    // 事件日志只含已持久化的转移，可直接重放。
    let snapshot = mgr.snapshot();
    assert!(mgr.replay(snapshot.events).is_ok());
}

#[test]
fn shared_cancel_token_stops_real_executor() {
    // R-05：任务登记共享执行体的真实取消令牌时，cancel 除推进状态机外
    // 必须触发该令牌，让真实执行体（如 agent run）停止。
    let mgr = manager();
    let run_token = CancellationToken::new();
    let id = mgr
        .register_with_cancel_token(TaskKind::Agent, None, run_token.clone())
        .unwrap();
    mgr.start(&id).unwrap();

    let events = mgr.cancel(&id).unwrap();
    assert_eq!(events.len(), 1);
    assert!(run_token.is_cancelled(), "cancel 必须触达共享令牌");
    assert_eq!(mgr.task(&id).unwrap().status, TaskStatus::Canceled);

    // 重复 cancel 幂等：已终态跳过，不发第二份终态事件。
    let again = mgr.cancel(&id).unwrap();
    assert!(again.is_empty());
}

#[test]
fn replayed_suspend_and_resume_rebuild_and_host_can_finish_or_cancel() {
    let id = BackgroundTaskId::new("task_hist");
    let events = vec![
        TaskEvent::Started {
            task_id: id.clone(),
            task_kind: TaskKind::Agent,
            parent_task_id: None,
        },
        TaskEvent::Suspended {
            task_id: id.clone(),
        },
        TaskEvent::Resumed {
            task_id: id.clone(),
        },
        TaskEvent::Suspended {
            task_id: id.clone(),
        },
    ];

    let mgr = manager();
    assert_eq!(mgr.replay(events.clone()).unwrap(), 4);
    assert_eq!(mgr.task(&id).unwrap().status, TaskStatus::Suspended);

    let snapshot = mgr.snapshot();
    let json = serde_json::to_string(&snapshot).unwrap();
    let restored: pawork_workflow::task::TaskManagerSnapshot = serde_json::from_str(&json).unwrap();
    assert_eq!(restored, snapshot);

    let mgr2 = manager();
    mgr2.replay(restored.events).unwrap();
    assert_eq!(mgr2.task(&id).unwrap().status, TaskStatus::Suspended);
    mgr2.finish(&id, TaskStatus::Completed, Some("after suspend".into()))
        .unwrap();
    assert_eq!(mgr2.task(&id).unwrap().status, TaskStatus::Completed);
    assert_eq!(mgr2.event_log().len(), 5);

    let mgr3 = manager();
    mgr3.replay(events).unwrap();
    let canceled = mgr3.cancel(&id).unwrap();
    assert_eq!(canceled.len(), 1);
    assert!(matches!(
        &canceled[0],
        TaskEvent::Finished {
            status: TaskStatus::Canceled,
            ..
        }
    ));
    assert_eq!(mgr3.task(&id).unwrap().status, TaskStatus::Canceled);
}

#[test]
fn replay_then_register_keeps_ids_unique() {
    let mgr = manager();
    let first = mgr.register(TaskKind::Agent, None).unwrap();
    mgr.start(&first).unwrap();
    mgr.finish(&first, TaskStatus::Completed, None).unwrap();
    let snapshot = mgr.snapshot();

    let restored = manager();
    restored.replay(snapshot.events).unwrap();
    let second = restored.register(TaskKind::Automation, None).unwrap();
    assert_ne!(second, first);
    assert_eq!(restored.task(&first).unwrap().status, TaskStatus::Completed);
    assert_eq!(restored.task(&second).unwrap().status, TaskStatus::Queued);
}
