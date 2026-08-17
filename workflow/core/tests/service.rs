//! MonitorService 集成测试：task-manager TaskKind::Monitor 注册、事件可消费、
//! 无直连 spawn 自查断言。

use std::path::PathBuf;

use pawork_domain::{MonitorEvent, MonitorId, TaskKind, TaskStatus};
use pawork_workflow::monitor::{Monitor, MonitorConfig, MonitorService, Observation};
use pawork_workflow::task::TaskManager;

fn port_monitor(id: &str) -> Monitor {
    Monitor::new(
        id,
        MonitorConfig::PortState {
            host: "127.0.0.1".into(),
            port: 8080,
        },
    )
}

fn service_with_task_manager() -> (MonitorService, TaskManager) {
    let tm = TaskManager::new();
    let svc = MonitorService::with_task_manager(tm.clone());
    (svc, tm)
}

#[test]
fn monitor_registers_as_task_kind_monitor() {
    let (svc, tm) = service_with_task_manager();
    let id = svc.register(port_monitor("m1"), None).unwrap();

    // task-manager 侧出现一条 TaskKind::Monitor 的 Queued 任务。
    let task_id = svc.monitor_task_id(&id).expect("task registered");
    let snapshot = tm.task(&task_id).expect("task exists");
    assert_eq!(snapshot.task_kind, TaskKind::Monitor);
    assert_eq!(snapshot.status, TaskStatus::Queued);

    // start 后任务 Running。
    svc.start(&id).unwrap();
    assert_eq!(tm.task(&task_id).unwrap().status, TaskStatus::Running);

    // stop 后任务 Completed（best-effort finish）。
    svc.stop(&id, Some("done".into())).unwrap();
    assert_eq!(tm.task(&task_id).unwrap().status, TaskStatus::Completed);
}

#[test]
fn stop_then_unregister_allows_reregister_same_id() {
    let (svc, tm) = service_with_task_manager();
    let id = svc.register(port_monitor("same"), None).unwrap();
    let first_task_id = svc.monitor_task_id(&id).expect("task registered");
    svc.start(&id).unwrap();
    svc.stop(&id, Some("done".into())).unwrap();
    assert_eq!(tm.task(&first_task_id).unwrap().status, TaskStatus::Completed);

    let unregistered = svc.unregister(&id).unwrap();
    assert!(matches!(
        unregistered,
        MonitorEvent::Unregistered { ref monitor_id } if monitor_id == &id
    ));
    assert!(svc.config(&id).is_none(), "config must be removed");
    assert!(svc.monitor_task_id(&id).is_none(), "task map must be removed");
    assert!(svc.record(&id).is_none(), "view record must be removed");
    assert!(
        matches!(
            svc.event_log().last(),
            Some(MonitorEvent::Unregistered { monitor_id }) if monitor_id == &id
        ),
        "unregister must be persistable/replayable"
    );
    assert!(
        matches!(
            svc.unregister(&id).unwrap_err(),
            pawork_workflow::monitor::MonitorServiceError::UnknownMonitor(_)
        ),
        "unknown unregister must fail closed"
    );

    let again = svc.register(port_monitor("same"), None).unwrap();
    assert_eq!(again, id);
    let second_task_id = svc.monitor_task_id(&again).expect("reregistered task");
    assert_ne!(second_task_id, first_task_id);
    assert_eq!(tm.task(&second_task_id).unwrap().status, TaskStatus::Queued);

    svc.start(&again).unwrap();
    let rec = svc.record(&again).expect("fresh record after reregister+start");
    assert_eq!(rec.trigger_count, 0);
    assert!(rec.last_detail.is_none());
}

#[test]
fn unregister_while_running_drops_view_and_cancels_task() {
    let (svc, tm) = service_with_task_manager();
    let id = svc.register(port_monitor("live"), None).unwrap();
    let task_id = svc.monitor_task_id(&id).expect("task registered");
    svc.start(&id).unwrap();
    svc.evaluate(
        &id,
        &Observation::PortState {
            host: "127.0.0.1".into(),
            port: 8080,
            open: true,
        },
    )
    .unwrap();
    assert_eq!(svc.record(&id).unwrap().trigger_count, 1);
    assert_eq!(tm.task(&task_id).unwrap().status, TaskStatus::Running);

    svc.unregister(&id).unwrap();
    assert!(svc.record(&id).is_none());
    assert!(svc.snapshot().monitors.iter().all(|rec| rec.monitor_id != id));
    assert_eq!(tm.task(&task_id).unwrap().status, TaskStatus::Canceled);

    let again = svc.register(port_monitor("live"), None).unwrap();
    svc.start(&again).unwrap();
    assert_eq!(svc.record(&again).unwrap().trigger_count, 0);
}

#[test]
fn duplicate_monitor_id_is_rejected_without_orphan_task() {
    let (svc, tm) = service_with_task_manager();
    let id = svc.register(port_monitor("same"), None).unwrap();
    let first_task_id = svc.monitor_task_id(&id).expect("task registered");

    let duplicate = Monitor::new(
        "same",
        MonitorConfig::PortState {
            host: "127.0.0.1".into(),
            port: 9090,
        },
    );
    let err = svc.register(duplicate, None).unwrap_err();
    assert!(matches!(
        err,
        pawork_workflow::monitor::MonitorServiceError::AlreadyRegistered(ref duplicate_id)
            if duplicate_id == &id
    ));

    assert_eq!(tm.tasks().len(), 1, "duplicate must not create orphan task");
    assert_eq!(svc.monitor_task_id(&id), Some(first_task_id));
    assert_eq!(
        svc.config(&id).unwrap().config,
        port_monitor("same").config,
        "duplicate must not overwrite the original config"
    );
}

#[test]
fn triggered_event_consumable_via_broadcast() {
    let (svc, _tm) = service_with_task_manager();
    let id = svc.register(port_monitor("m1"), None).unwrap();
    svc.start(&id).unwrap();

    let mut rx = svc.subscribe();
    let detail = svc
        .evaluate(
            &id,
            &Observation::PortState {
                host: "127.0.0.1".into(),
                port: 8080,
                open: true,
            },
        )
        .unwrap();
    assert_eq!(detail.as_deref(), Some("port 8080 open"));

    // 广播出的 AgentEvent::Monitor(Triggered) 可被订阅者消费（automation event
    // 触发器来源）。
    let mut saw_triggered = false;
    while let Ok(event) = rx.try_recv() {
        if let pawork_domain::AgentEvent::Monitor(MonitorEvent::Triggered { monitor_id, detail }) =
            event
        {
            assert_eq!(monitor_id, MonitorId::new("m1"));
            assert_eq!(detail, "port 8080 open");
            saw_triggered = true;
        }
    }
    assert!(saw_triggered, "Triggered event should be broadcast");

    let rec = svc.record(&id).unwrap();
    assert_eq!(rec.trigger_count, 1);
}

/// task start 失败时不得先广播 Started：monitor 状态与事件流保持未启动，
/// 不出现「已发 Started 但任务仍 Queued/Running 分叉」。
#[test]
fn start_failure_does_not_broadcast_started() {
    let (svc, tm) = service_with_task_manager();
    let id = svc.register(port_monitor("m1"), None).unwrap();
    let task_id = svc.monitor_task_id(&id).unwrap();

    // 预先手动把任务推到 Running（模拟外部已 start），使 svc.start 的镜像 start
    // 触发 InvalidTransition 失败。
    tm.start(&task_id).unwrap();
    let err = svc.start(&id).unwrap_err();
    assert!(
        matches!(err, pawork_workflow::monitor::MonitorServiceError::TaskManager(_)),
        "expected task-manager error, got {err:?}"
    );

    // 未广播 Started、未推进 monitor 状态。
    let mut rx = svc.subscribe();
    assert!(rx.try_recv().is_err(), "no Started may be broadcast");
    assert!(svc.event_log().is_empty(), "no monitor events emitted");
    assert!(svc.record(&id).is_none(), "no state change may be recorded");
}

#[test]
fn deterministic_evaluate_without_task_manager() {
    // 无 task-manager 时，evaluate 仍可独立确定性工作。
    let svc = MonitorService::new();
    let id = svc
        .register(
            Monitor::new(
                "m",
                MonitorConfig::RegexMatch {
                    stream: "stdout".into(),
                    pattern: r"error: \w+".into(),
                },
            ),
            None,
        )
        .unwrap();
    svc.start(&id).unwrap();
    let detail = svc
        .evaluate(
            &id,
            &Observation::RegexMatch {
                stream: "stdout".into(),
                text: "error: boom".into(),
            },
        )
        .unwrap();
    assert_eq!(detail.as_deref(), Some("regex matched: error: boom"));
}

#[test]
fn no_direct_process_spawn_in_source() {
    // 硬约束断言：monitor-service 源码不得直连 tokio::process::Command /
    // std::process::Command；子进程一律经注入的 task-manager。
    let src = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/monitor");
    let needles = [
        "tokio::process::Command",
        "std::process::Command",
        "process::Command::new",
    ];
    let mut violations = String::new();
    for entry in std::fs::read_dir(&src).expect("src dir") {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        let content = std::fs::read_to_string(&path).unwrap();
        // 忽略注释行（含 ///、//!、//），只检查实际代码引用——文档中为了
        // 说明约束而提及的禁止模式不应触发断言。
        let code: String = content
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        for needle in needles {
            if code.contains(needle) {
                violations.push_str(&format!("{}: contains `{}`\n", path.display(), needle));
            }
        }
    }
    assert!(
        violations.is_empty(),
        "monitor-service must not spawn processes directly:\n{violations}"
    );
}
