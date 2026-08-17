//! 状态机 / 事件日志 / 快照 / 重放 / 取消传播 的定向测试。

use pawork_domain::{BackgroundTaskId, TaskEvent, TaskKind, TaskStatus};
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

    let suspended = mgr.suspend(&id).unwrap();
    assert!(matches!(suspended, TaskEvent::Suspended { task_id } if task_id == id));
    assert_eq!(mgr.task(&id).unwrap().status, TaskStatus::Suspended);

    let resumed = mgr.resume(&id).unwrap();
    assert!(matches!(resumed, TaskEvent::Resumed { task_id } if task_id == id));
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
    assert_eq!(mgr.event_log().len(), 4);
}

#[test]
fn illegal_transitions_rejected() {
    let mgr = manager();
    let id = mgr.register(TaskKind::Process, None).unwrap();

    assert!(matches!(
        mgr.suspend(&id),
        Err(TaskManagerError::InvalidTransition { .. })
    ));
    assert!(matches!(
        mgr.resume(&id),
        Err(TaskManagerError::InvalidTransition { .. })
    ));
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
        mgr.resume(&id),
        Err(TaskManagerError::InvalidTransition { .. })
    ));

    mgr.suspend(&id).unwrap();
    assert!(matches!(
        mgr.suspend(&id),
        Err(TaskManagerError::InvalidTransition { .. })
    ));
    mgr.resume(&id).unwrap();

    assert!(matches!(
        mgr.finish(&id, TaskStatus::Canceled, None),
        Err(TaskManagerError::InvalidFinishedStatus(_))
    ));
    mgr.finish(&id, TaskStatus::Completed, None).unwrap();
    assert!(matches!(
        mgr.finish(&id, TaskStatus::Completed, None),
        Err(TaskManagerError::InvalidTransition { .. })
    ));
    assert!(matches!(
        mgr.suspend(&id),
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
    mgr.suspend(&child).unwrap();
    mgr.finish(&parent, TaskStatus::Completed, Some("ok".into()))
        .unwrap();

    let snapshot = mgr.snapshot();
    assert_eq!(snapshot.tasks.len(), 2);
    assert_eq!(snapshot.events.len(), 4);

    // 事件日志可 JSON 序列化 / 反序列化（持久化 + 重放输入）。
    let json = serde_json::to_string(&snapshot).unwrap();
    let restored: pawork_workflow::task::TaskManagerSnapshot = serde_json::from_str(&json).unwrap();
    assert_eq!(restored, snapshot);

    // 全新 manager 重放后重建出相同视图。
    let mgr2 = manager();
    let count = mgr2.replay(restored.events).unwrap();
    assert_eq!(count, 4);
    assert_eq!(mgr2.snapshot(), snapshot);
    assert_eq!(mgr2.task(&parent).unwrap().status, TaskStatus::Completed);
    assert_eq!(mgr2.task(&child).unwrap().status, TaskStatus::Suspended);
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
fn events_since_returns_increment() {
    let mgr = manager();
    let id = mgr.register(TaskKind::Automation, None).unwrap();
    assert!(mgr.events_since(0).is_empty());

    mgr.start(&id).unwrap();
    assert_eq!(mgr.events_since(0).len(), 1);
    let cursor = mgr.event_log().len() as u64;

    mgr.suspend(&id).unwrap();
    mgr.resume(&id).unwrap();
    let increment = mgr.events_since(cursor);
    assert_eq!(increment.len(), 2);
    assert!(matches!(increment[0], TaskEvent::Suspended { .. }));
    assert!(matches!(increment[1], TaskEvent::Resumed { .. }));
    assert!(mgr.events_since(100).is_empty());
}
