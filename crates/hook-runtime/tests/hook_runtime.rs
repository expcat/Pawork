use std::sync::Arc;

use agent_domain::{CancellationToken, CoreInstanceId, PluginId, RunId, WorkspaceId};
use hook_runtime::{HookDispatchReport, HookRuntime, HookRuntimeError, PluginHookOutcomeStatus};
use plugin_api::{
    plugin_api_version, PluginContext, PluginError, PluginErrorKind, PluginLifecycleEvent,
    PluginLifecycleEventKind,
};
use semver::Version;
use test_support::plugin_contract::{
    assert_api_compatible, assert_api_incompatible, assert_compatibility_matrix,
};
use test_support::{hook_plugin, hook_plugin_with_api, MockPlugin, MockPluginStep};
use tokio::time::{timeout, Duration};

fn context() -> PluginContext {
    PluginContext {
        instance_id: CoreInstanceId::from("core"),
        workspace_id: None,
        session_id: None,
        run_id: None,
    }
}

fn run_start_event() -> PluginLifecycleEvent {
    PluginLifecycleEvent::RunStart {
        run_id: RunId::from("run-1"),
    }
}

#[tokio::test]
async fn dispatch_is_sorted_by_plugin_id_and_only_for_subscribed_plugins() {
    let runtime = HookRuntime::new();
    let plugin_a = Arc::new(hook_plugin("a", [PluginLifecycleEventKind::RunStart]));
    let plugin_b = Arc::new(hook_plugin(
        "b",
        [
            PluginLifecycleEventKind::RunStart,
            PluginLifecycleEventKind::WorkspaceOpen,
        ],
    ));
    let plugin_c = Arc::new(hook_plugin("c", [PluginLifecycleEventKind::Stop]));

    // 逆序注册：派发顺序与注册顺序无关，只按 plugin id 确定性排序。
    runtime
        .register(plugin_c.clone())
        .await
        .expect("register c");
    runtime
        .register(plugin_b.clone())
        .await
        .expect("register b");
    runtime
        .register(plugin_a.clone())
        .await
        .expect("register a");
    runtime.start(context()).await.expect("start");

    let report = runtime
        .dispatch(run_start_event(), context(), CancellationToken::new())
        .await
        .expect("dispatch run_start");

    assert!(!report.cancelled);
    let ids: Vec<_> = report
        .outcomes
        .iter()
        .map(|outcome| outcome.plugin_id.as_str())
        .collect();
    assert_eq!(ids, ["a", "b"]);
    assert!(report
        .outcomes
        .iter()
        .all(|outcome| outcome.status == PluginHookOutcomeStatus::Success));
    assert_eq!(plugin_a.call_count(), 1);
    assert_eq!(plugin_b.call_count(), 1);
    assert_eq!(plugin_c.call_count(), 0);

    // 只有 manifest 声明的事件才会派发。
    let workspace_report = runtime
        .dispatch(
            PluginLifecycleEvent::WorkspaceOpen {
                workspace_id: WorkspaceId::from("workspace-1"),
            },
            context(),
            CancellationToken::new(),
        )
        .await
        .expect("dispatch workspace_open");
    assert_eq!(workspace_report.outcomes.len(), 1);
    assert_eq!(workspace_report.outcomes[0].plugin_id.as_str(), "b");
    assert_eq!(plugin_b.call_count(), 2);
    assert_eq!(plugin_a.call_count(), 1);
}

#[tokio::test]
async fn duplicate_registration_is_rejected_deterministically() {
    let runtime = HookRuntime::new();
    runtime
        .register(Arc::new(hook_plugin(
            "same",
            [PluginLifecycleEventKind::Start],
        )))
        .await
        .expect("first registration");

    let conflict = runtime
        .register(Arc::new(hook_plugin(
            "same",
            [PluginLifecycleEventKind::RunStart],
        )))
        .await
        .expect_err("duplicate id must be rejected");
    assert!(matches!(
        conflict,
        HookRuntimeError::Conflict { plugin_id } if plugin_id == PluginId::from("same")
    ));

    let registered = runtime.registered().await;
    assert_eq!(registered, [PluginId::from("same")]);
}

#[tokio::test]
async fn plugin_error_panic_and_cancel_are_isolated_and_runtime_survives() {
    let runtime = HookRuntime::new();
    let plugin_a = Arc::new(
        hook_plugin("a", [PluginLifecycleEventKind::RunStart]).with_step(MockPluginStep::Error(
            PluginError::new(PluginErrorKind::Trap, "plugin a trap"),
        )),
    );
    let plugin_b = Arc::new(hook_plugin("b", [PluginLifecycleEventKind::RunStart]));
    let plugin_c = Arc::new(
        hook_plugin("c", [PluginLifecycleEventKind::RunStart])
            .with_step(MockPluginStep::Panic("plugin c panic".into())),
    );
    let plugin_d = Arc::new(
        hook_plugin("d", [PluginLifecycleEventKind::RunStart]).with_step(MockPluginStep::Error(
            PluginError::cancelled("plugin d cancelled"),
        )),
    );
    for plugin in [&plugin_a, &plugin_b, &plugin_c, &plugin_d] {
        runtime.register((*plugin).clone()).await.expect("register");
    }
    runtime.start(context()).await.expect("start");

    let report = runtime
        .dispatch(run_start_event(), context(), CancellationToken::new())
        .await
        .expect("dispatch");

    assert_eq!(report.outcomes.len(), 4);
    assert!(matches!(
        &report.outcomes[0].status,
        PluginHookOutcomeStatus::Error { error } if error.kind == PluginErrorKind::Trap
    ));
    assert_eq!(report.outcomes[1].status, PluginHookOutcomeStatus::Success);
    assert!(matches!(
        &report.outcomes[2].status,
        PluginHookOutcomeStatus::Error { error }
            if error.kind == PluginErrorKind::Internal
                && error.message == "plugin hook task panicked"
    ));
    assert!(matches!(
        &report.outcomes[3].status,
        PluginHookOutcomeStatus::Cancelled { .. }
    ));

    // 错误/取消/panic 不中断后续插件，所有订阅插件都被调用。
    assert_eq!(plugin_a.call_count(), 1);
    assert_eq!(plugin_b.call_count(), 1);
    assert_eq!(plugin_c.call_count(), 1);
    assert_eq!(plugin_d.call_count(), 1);

    // 运行时仍然可用，脚本耗尽后默认成功。
    let second = runtime
        .dispatch(run_start_event(), context(), CancellationToken::new())
        .await
        .expect("second dispatch");
    assert_eq!(second.outcomes.len(), 4);
    assert!(second
        .outcomes
        .iter()
        .all(|outcome| outcome.status == PluginHookOutcomeStatus::Success));
}

#[tokio::test]
async fn unregister_stops_dispatch_and_rejects_unknown_ids() {
    let runtime = HookRuntime::new();
    let plugin_a = Arc::new(hook_plugin("a", [PluginLifecycleEventKind::RunStart]));
    let plugin_b = Arc::new(hook_plugin("b", [PluginLifecycleEventKind::RunStart]));
    runtime
        .register(plugin_a.clone())
        .await
        .expect("register a");
    runtime
        .register(plugin_b.clone())
        .await
        .expect("register b");
    runtime.start(context()).await.expect("start");

    runtime
        .unregister(&PluginId::from("a"))
        .await
        .expect("unregister a");
    let report = runtime
        .dispatch(run_start_event(), context(), CancellationToken::new())
        .await
        .expect("dispatch after unregister");
    assert_eq!(report.outcomes.len(), 1);
    assert_eq!(report.outcomes[0].plugin_id.as_str(), "b");
    assert_eq!(plugin_a.call_count(), 0);
    assert_eq!(plugin_b.call_count(), 1);

    let missing = runtime
        .unregister(&PluginId::from("a"))
        .await
        .expect_err("a already removed");
    assert!(matches!(missing, HookRuntimeError::NotFound { .. }));
    let unknown = runtime
        .unregister(&PluginId::from("nope"))
        .await
        .expect_err("unknown id");
    assert!(matches!(unknown, HookRuntimeError::NotFound { .. }));
}

#[tokio::test]
async fn start_and_stop_drive_lifecycle_and_gate_dispatch() {
    let runtime = HookRuntime::new();
    let plugin = Arc::new(hook_plugin(
        "a",
        [
            PluginLifecycleEventKind::Start,
            PluginLifecycleEventKind::Stop,
            PluginLifecycleEventKind::RunStart,
        ],
    ));
    runtime.register(plugin.clone()).await.expect("register");
    assert!(!runtime.is_started().await);

    let not_started = runtime
        .dispatch(run_start_event(), context(), CancellationToken::new())
        .await
        .expect_err("dispatch before start");
    assert!(matches!(not_started, HookRuntimeError::NotStarted));

    let start_report = runtime.start(context()).await.expect("start");
    assert_eq!(start_report.event, PluginLifecycleEventKind::Start);
    assert_eq!(start_report.outcomes.len(), 1);
    assert_eq!(plugin.calls()[0].event, PluginLifecycleEventKind::Start);
    assert!(runtime.is_started().await);

    let already_started = runtime.start(context()).await.expect_err("double start");
    assert!(matches!(already_started, HookRuntimeError::AlreadyStarted));

    runtime
        .dispatch(run_start_event(), context(), CancellationToken::new())
        .await
        .expect("dispatch while started");
    assert_eq!(plugin.calls()[1].event, PluginLifecycleEventKind::RunStart);

    let stop_report = runtime.stop(context()).await.expect("stop");
    assert_eq!(stop_report.event, PluginLifecycleEventKind::Stop);
    assert_eq!(stop_report.outcomes.len(), 1);
    assert_eq!(plugin.calls()[2].event, PluginLifecycleEventKind::Stop);
    assert!(!runtime.is_started().await);

    let stopped = runtime
        .dispatch(run_start_event(), context(), CancellationToken::new())
        .await
        .expect_err("dispatch after stop");
    assert!(matches!(stopped, HookRuntimeError::NotStarted));

    let double_stop = runtime.stop(context()).await.expect_err("double stop");
    assert!(matches!(double_stop, HookRuntimeError::NotStarted));

    // 停止后可再次启动，生命周期可重入。
    runtime.start(context()).await.expect("restart");
    assert!(runtime.is_started().await);
}

#[tokio::test]
async fn dispatch_report_round_trips_through_json() {
    let runtime = HookRuntime::new();
    let plugin = Arc::new(
        hook_plugin("a", [PluginLifecycleEventKind::RunStart]).with_step(MockPluginStep::Error(
            PluginError::new(PluginErrorKind::Timeout, "slow"),
        )),
    );
    runtime.register(plugin.clone()).await.expect("register");
    runtime.start(context()).await.expect("start");

    let report = runtime
        .dispatch(run_start_event(), context(), CancellationToken::new())
        .await
        .expect("dispatch");

    let encoded = serde_json::to_string(&report).expect("serialize report");
    let decoded: HookDispatchReport = serde_json::from_str(&encoded).expect("deserialize report");
    assert_eq!(decoded, report);
    assert_eq!(decoded.event, PluginLifecycleEventKind::RunStart);
    assert!(matches!(
        &decoded.outcomes[0].status,
        PluginHookOutcomeStatus::Error { error } if error.kind == PluginErrorKind::Timeout
    ));
}

#[tokio::test]
async fn dispatch_cancel_marks_pending_plugins_cancelled_without_invocation() {
    let runtime = HookRuntime::new();
    let plugin_a = Arc::new(hook_plugin("a", [PluginLifecycleEventKind::RunStart]));
    let plugin_b = Arc::new(hook_plugin("b", [PluginLifecycleEventKind::RunStart]));
    runtime
        .register(plugin_a.clone())
        .await
        .expect("register a");
    runtime
        .register(plugin_b.clone())
        .await
        .expect("register b");
    runtime.start(context()).await.expect("start");

    let cancel = CancellationToken::new();
    cancel.cancel();
    let report = runtime
        .dispatch(run_start_event(), context(), cancel)
        .await
        .expect("dispatch with cancelled token");

    assert!(report.cancelled);
    assert_eq!(report.outcomes.len(), 2);
    assert!(report
        .outcomes
        .iter()
        .all(|outcome| { matches!(outcome.status, PluginHookOutcomeStatus::Cancelled { .. }) }));
    assert_eq!(plugin_a.call_count(), 0);
    assert_eq!(plugin_b.call_count(), 0);
}

#[tokio::test]
async fn cancellation_during_last_plugin_sets_dispatch_flag() {
    let runtime = HookRuntime::new();
    let plugin = Arc::new(
        hook_plugin("only", [PluginLifecycleEventKind::RunStart])
            .with_step(MockPluginStep::WaitForCancellation),
    );
    runtime
        .register(plugin.clone())
        .await
        .expect("register plugin");
    runtime.start(context()).await.expect("start");

    let cancel = CancellationToken::new();
    let cancel_after_entry = cancel.clone();
    let observed = plugin.clone();
    let canceller = tokio::spawn(async move {
        timeout(Duration::from_secs(1), async {
            while observed.call_count() == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("plugin invocation must start");
        cancel_after_entry.cancel();
    });

    let report = timeout(
        Duration::from_secs(1),
        runtime.dispatch(run_start_event(), context(), cancel),
    )
    .await
    .expect("dispatch must observe cancellation")
    .expect("dispatch report");
    canceller.await.expect("canceller task");

    assert!(report.cancelled);
    assert_eq!(report.outcomes.len(), 1);
    assert!(matches!(
        report.outcomes[0].status,
        PluginHookOutcomeStatus::Cancelled { .. }
    ));
    assert_eq!(plugin.call_count(), 1);
}

#[tokio::test]
async fn registration_enforces_host_api_semver_compatibility() {
    let runtime = HookRuntime::new();
    assert_eq!(runtime.host_api_version().await, plugin_api_version());

    runtime
        .register(Arc::new(hook_plugin_with_api(
            "compatible",
            "^1.0",
            [PluginLifecycleEventKind::RunStart],
        )))
        .await
        .expect("same major/minor requirement must register");

    let cross_major = runtime
        .register(Arc::new(hook_plugin_with_api(
            "cross-major",
            "^2.0",
            [PluginLifecycleEventKind::RunStart],
        )))
        .await
        .expect_err("cross major must be rejected");
    assert!(matches!(
        cross_major,
        HookRuntimeError::IncompatibleApi { .. }
    ));

    let below_minimum = runtime
        .register(Arc::new(hook_plugin_with_api(
            "below-minimum",
            "^1.2",
            [PluginLifecycleEventKind::RunStart],
        )))
        .await
        .expect_err("unsatisfied range must be rejected");
    assert!(matches!(
        below_minimum,
        HookRuntimeError::IncompatibleApi { .. }
    ));

    // 宿主版本可配置：同一范围在更高宿主上通过。
    let future = HookRuntime::with_host_api_version(Version::new(1, 2, 0));
    future
        .register(Arc::new(hook_plugin_with_api(
            "future",
            "^1.2",
            [PluginLifecycleEventKind::RunStart],
        )))
        .await
        .expect("range satisfied by configured host must register");

    // P10-6 断言辅助与矩阵也作为本套件的门禁。
    assert_api_compatible("^1.0", "1.2.3");
    assert_api_incompatible("^1.2", "1.1.9");
    assert_api_incompatible("^1.0", "2.0.0");
    assert_compatibility_matrix();
}

#[tokio::test]
async fn registration_rejects_invalid_manifest() {
    let runtime = HookRuntime::new();
    let mut manifest = hook_plugin("bad", [PluginLifecycleEventKind::RunStart])
        .manifest()
        .clone();
    manifest.capabilities.clear();

    let error = runtime
        .register(Arc::new(MockPlugin::new(manifest)))
        .await
        .expect_err("hooks without LifecycleHook capability must be rejected");
    assert!(matches!(error, HookRuntimeError::InvalidManifest(_)));
}

#[tokio::test]
async fn registration_contains_manifest_accessor_panic() {
    let runtime = HookRuntime::new();
    let plugin =
        hook_plugin("panic-manifest", [PluginLifecycleEventKind::RunStart]).with_manifest_panic();

    let error = runtime
        .register(Arc::new(plugin))
        .await
        .expect_err("manifest panic must become a registration error");
    assert!(matches!(
        error,
        HookRuntimeError::InvalidManifest(message)
            if message == "plugin manifest accessor panicked"
    ));
    assert!(runtime.registered().await.is_empty());
}
