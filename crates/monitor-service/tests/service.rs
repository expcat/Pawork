//! MonitorService 集成测试：task-manager TaskKind::Monitor 注册、事件可消费、
//! 无直连 spawn 自查断言。

use std::path::PathBuf;

use agent_domain::{MonitorEvent, MonitorId, TaskKind, TaskStatus};
use monitor_service::{Monitor, MonitorConfig, MonitorService, Observation};
use process_runtime::ProcessRuntime;
use task_manager::TaskManager;

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
    let tm = TaskManager::with_platform_default(ProcessRuntime::new());
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
        if let agent_events::AgentEvent::Monitor(MonitorEvent::Triggered { monitor_id, detail }) =
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
    let src = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
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
