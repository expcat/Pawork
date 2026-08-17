#![cfg(feature = "process-exec")]

//! 进程类任务接线、policy 透传、断连续存与执行所有权断言的定向测试。

mod common;

use std::path::Path;
use std::time::Duration;

use pawork_domain::{BackgroundTaskId, TaskKind, TaskStatus};
use pawork_exec::{
    default_env_allowlist, CommandSpec, NativeRestricted, NetworkMode, ProcessEvent,
    ProcessRuntime, SandboxPolicy, SandboxProcessSpec,
};
use pawork_workflow::task::{is_terminal_status, TaskManager, TaskManagerError};

fn native_manager() -> TaskManager {
    TaskManager::with_backend(Box::new(NativeRestricted::with_runtime(
        ProcessRuntime::new(),
    )))
}

/// 测试用最小放行 policy：允许 spawn + env 白名单 + 网络仅 Hint。
/// 与 untrusted_default 同源，仅放开执行所需的最小面。
fn test_policy() -> SandboxPolicy {
    SandboxPolicy {
        allow_spawn: true,
        env_clear: true,
        env_allowlist: default_env_allowlist(),
        network_mode: NetworkMode::Hint,
        ..SandboxPolicy::untrusted_default(Vec::new())
    }
}

#[cfg(unix)]
fn echo_spec() -> CommandSpec {
    CommandSpec::new("echo").arg("p16-4-ok")
}

#[cfg(windows)]
fn echo_spec() -> CommandSpec {
    CommandSpec::new("cmd").args(["/C", "echo p16-4-ok"])
}

#[cfg(unix)]
fn fail_spec() -> CommandSpec {
    CommandSpec::new("sh").args(["-c", "exit 3"])
}

#[cfg(windows)]
fn fail_spec() -> CommandSpec {
    CommandSpec::new("cmd").args(["/C", "exit 3"])
}

#[cfg(unix)]
fn sleep_spec(seconds: &str) -> CommandSpec {
    CommandSpec::new("sh")
        .arg("-c")
        .arg(format!("sleep {seconds}"))
}

#[cfg(windows)]
fn sleep_spec(seconds: &str) -> CommandSpec {
    CommandSpec::new("ping").args(["-n", seconds, "127.0.0.1"])
}

async fn wait_status(
    manager: &TaskManager,
    task_id: &BackgroundTaskId,
    predicate: impl Fn(TaskStatus) -> bool,
) -> TaskStatus {
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            if let Some(task) = manager.task(task_id) {
                if predicate(task.status) {
                    return task.status;
                }
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("timed out waiting for task status")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn process_completes_with_output() {
    let manager = native_manager();
    let task_id = manager
        .start_process(
            SandboxProcessSpec {
                command: echo_spec(),
                workspace_roots: Vec::new(),
            },
            test_policy(),
            None,
        )
        .await
        .unwrap();

    let status = wait_status(&manager, &task_id, is_terminal_status).await;
    assert_eq!(status, TaskStatus::Completed);
    assert_eq!(manager.task(&task_id).unwrap().task_kind, TaskKind::Process);

    let output = manager.output(&task_id);
    assert!(output.iter().any(|o| matches!(
        &o.event,
        ProcessEvent::Stdout(bytes) if bytes.windows(b"p16-4-ok".len()).any(|w| w == b"p16-4-ok")
    )));
    assert!(output
        .iter()
        .any(|o| matches!(&o.event, ProcessEvent::Exit { code: Some(0), .. })));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn process_failure_reported() {
    let manager = native_manager();
    let task_id = manager
        .start_process(
            SandboxProcessSpec {
                command: fail_spec(),
                workspace_roots: Vec::new(),
            },
            test_policy(),
            None,
        )
        .await
        .unwrap();

    let status = wait_status(&manager, &task_id, is_terminal_status).await;
    assert_eq!(status, TaskStatus::Failed);
    let detail = manager.task(&task_id).unwrap().detail;
    assert!(detail.is_some() && detail.unwrap().contains("exit"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn process_cancel_terminates_tree() {
    let manager = native_manager();
    let task_id = manager
        .start_process(
            SandboxProcessSpec {
                command: sleep_spec("30"),
                workspace_roots: Vec::new(),
            },
            test_policy(),
            None,
        )
        .await
        .unwrap();
    wait_status(&manager, &task_id, |s| s == TaskStatus::Running).await;

    let events = manager.cancel(&task_id).unwrap();
    assert_eq!(events.len(), 1);
    let status = wait_status(&manager, &task_id, is_terminal_status).await;
    assert_eq!(status, TaskStatus::Canceled);
    // 进程树被杀后流关闭：驱动看到 Exit{code: None}，且不产生重复终态事件。
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            if manager
                .output(&task_id)
                .iter()
                .any(|o| matches!(&o.event, ProcessEvent::Exit { code: None, .. }))
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("timed out waiting for Exit{code: None}");
    assert_eq!(manager.event_log().len(), 2); // Started + Finished{Canceled}
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn process_parent_cancel_cascades() {
    let manager = native_manager();
    let parent = manager
        .start_process(
            SandboxProcessSpec {
                command: sleep_spec("30"),
                workspace_roots: Vec::new(),
            },
            test_policy(),
            None,
        )
        .await
        .unwrap();
    let child = manager
        .start_process(
            SandboxProcessSpec {
                command: sleep_spec("30"),
                workspace_roots: Vec::new(),
            },
            test_policy(),
            Some(parent.clone()),
        )
        .await
        .unwrap();
    for id in [&parent, &child] {
        wait_status(&manager, id, |s| s == TaskStatus::Running).await;
    }

    let events = manager.cancel(&parent).unwrap();
    assert_eq!(events.len(), 2);
    for id in [&parent, &child] {
        let status = wait_status(&manager, id, is_terminal_status).await;
        assert_eq!(status, TaskStatus::Canceled);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn process_suspend_resume_lifecycle() {
    let manager = native_manager();
    let task_id = manager
        .start_process(
            SandboxProcessSpec {
                command: sleep_spec("1"),
                workspace_roots: Vec::new(),
            },
            test_policy(),
            None,
        )
        .await
        .unwrap();
    wait_status(&manager, &task_id, |s| s == TaskStatus::Running).await;

    manager.suspend(&task_id).unwrap();
    assert_eq!(
        manager.task(&task_id).unwrap().status,
        TaskStatus::Suspended
    );
    manager.resume(&task_id).unwrap();

    let status = wait_status(&manager, &task_id, is_terminal_status).await;
    assert_eq!(status, TaskStatus::Completed);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn process_exit_while_suspended_finishes() {
    let manager = native_manager();
    let task_id = manager
        .start_process(
            SandboxProcessSpec {
                command: sleep_spec("1"),
                workspace_roots: Vec::new(),
            },
            test_policy(),
            None,
        )
        .await
        .unwrap();
    wait_status(&manager, &task_id, |s| s == TaskStatus::Running).await;
    manager.suspend(&task_id).unwrap();

    // 挂起期间进程退出：Suspended → Completed 仍由驱动折叠。
    let status = wait_status(&manager, &task_id, is_terminal_status).await;
    assert_eq!(status, TaskStatus::Completed);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn output_cursor_resume_incremental() {
    let manager = native_manager();
    let task_id = manager
        .start_process(
            SandboxProcessSpec {
                command: echo_spec(),
                workspace_roots: Vec::new(),
            },
            test_policy(),
            None,
        )
        .await
        .unwrap();
    wait_status(&manager, &task_id, is_terminal_status).await;

    let cursor = manager.task(&task_id).unwrap().output_seq;
    assert!(cursor > 0);
    // 断连后从 cursor 续读：无新输出，且不重复。
    assert!(manager.output_since(&task_id, cursor).is_empty());
    assert_eq!(
        manager.output(&task_id).len(),
        manager.output_since(&task_id, 0).len()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn disconnect_and_reconnect_semantics() {
    let manager = native_manager();
    let task_id = manager
        .start_process(
            SandboxProcessSpec {
                command: echo_spec(),
                workspace_roots: Vec::new(),
            },
            test_policy(),
            None,
        )
        .await
        .unwrap();

    // 客户端订阅后断连：任务继续运行，与连接解耦。
    let subscriber = manager.subscribe();
    drop(subscriber);
    let status = wait_status(&manager, &task_id, is_terminal_status).await;
    assert_eq!(status, TaskStatus::Completed);

    // 重连：snapshot 恢复任务视图，output_since 续读增量输出。
    let snapshot = manager.snapshot();
    assert_eq!(snapshot.tasks.len(), 1);
    assert_eq!(snapshot.tasks[0].status, TaskStatus::Completed);
    let output = manager.output_since(&task_id, 0);
    assert!(output
        .iter()
        .any(|o| matches!(&o.event, ProcessEvent::Stdout(bytes) if !bytes.is_empty())));

    // 全新 manager 重放同一事件日志，重建出相同视图（进程重启恢复）。
    let restarted = native_manager();
    restarted.replay(snapshot.events.clone()).unwrap();
    // output_seq / output_bytes 是运行期游标（事件日志不含输出内容），
    // 重放恢复的是任务视图本身；输出增量由活 manager 的 cursor 续读。
    assert_same_view(&restarted.snapshot(), &snapshot);
}

/// 比较两个快照的任务视图（忽略运行期输出游标）。
fn assert_same_view(
    left: &pawork_workflow::task::TaskManagerSnapshot,
    right: &pawork_workflow::task::TaskManagerSnapshot,
) {
    assert_eq!(left.tasks.len(), right.tasks.len());
    for (a, b) in left.tasks.iter().zip(right.tasks.iter()) {
        assert_eq!(a.task_id, b.task_id);
        assert_eq!(a.task_kind, b.task_kind);
        assert_eq!(a.parent_task_id, b.parent_task_id);
        assert_eq!(a.status, b.status);
        assert_eq!(a.detail, b.detail);
    }
    assert_eq!(left.events, right.events);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn lagged_subscriber_recovers_via_snapshot() {
    let (manager, _backend) = common::manager_with_recording_backend_capacity(4);
    let mut subscriber = manager.subscribe();
    for _ in 0..20 {
        let id = manager.register(TaskKind::Agent, None).unwrap();
        manager.start(&id).unwrap();
        manager.finish(&id, TaskStatus::Completed, None).unwrap();
    }

    assert!(
        matches!(
            subscriber.try_recv(),
            Err(tokio::sync::broadcast::error::TryRecvError::Lagged(_))
        ),
        "慢客户端必须收到 Lagged，随后经 snapshot + events_since 恢复"
    );
    let snapshot = manager.snapshot();
    let events = manager.events_since(0);
    assert_eq!(events.len(), snapshot.events.len());

    let recovered = common::manager_with_recording_backend().0;
    recovered.replay(events).unwrap();
    assert_eq!(recovered.snapshot().tasks, snapshot.tasks);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn backend_receives_exact_policy_no_escalation() {
    let (manager, backend) = common::manager_with_recording_backend();
    let spec = SandboxProcessSpec {
        command: CommandSpec::new("echo").arg("x"),
        workspace_roots: Vec::new(),
    };
    let policy = test_policy();

    let result = manager.start_process(spec, policy.clone(), None).await;
    assert!(matches!(
        result,
        Err(TaskManagerError::Sandbox(
            pawork_exec::SandboxError::Denied(_)
        ))
    ));

    let calls = backend.calls();
    assert_eq!(calls.len(), 1);
    let (received_spec, received_policy) = &calls[0];
    // policy 原样透传：后台任务不因「后台」获得额外越权。
    assert_eq!(
        serde_json::to_value(received_policy).unwrap(),
        serde_json::to_value(&policy).unwrap()
    );
    assert_eq!(received_spec.command.program, "echo");
    // spawn 失败后 Queued 记录被清理，无幽灵任务、无残留事件。
    assert!(manager.tasks().is_empty());
    assert!(manager.event_log().is_empty());
}

#[test]
fn no_direct_process_spawn_or_self_made_cleanup() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut source = String::new();
    for entry in std::fs::read_dir(root.join("src/task")).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) == Some("rs") {
            source.push_str(&std::fs::read_to_string(&path).unwrap());
            source.push('\n');
        }
    }

    // 直连进程执行与自造进程树清理在 task-manager 源码中必须不存在：
    // 执行所有权一律经注入的 SandboxBackend → ProcessRuntime。
    for needle in [
        "tokio::process",
        "std::process",
        "Command::new",
        "killpg",
        "JobObject",
        "kill_on_drop",
        "libc::",
    ] {
        assert!(
            !source.contains(needle),
            "task-manager 不得包含 `{needle}`：进程执行与清理必须委托给 process-runtime"
        );
    }
    // 不自定 filesystem/network policy。
    assert!(
        !source.contains("SandboxPolicy {"),
        "task-manager 不得自行构造 SandboxPolicy"
    );

    // tokio 依赖不启用 process feature（无直连子进程能力）。
    let manifest = std::fs::read_to_string(root.join("Cargo.toml")).unwrap();
    assert!(
        !manifest
            .lines()
            .any(|line| line.contains("tokio") && line.contains("process")),
        "task-manager 的 tokio 依赖不得启用 process feature"
    );
}
