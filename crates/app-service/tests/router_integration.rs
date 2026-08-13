//! 统一 Command Router 集成测试（P13-1）。
//!
//! 覆盖：幂等去重（网络重试不重复建 Run）、来源/身份记录、无凭据结构化错误、
//! 快照聚合与查询面（snapshot_fetch / diff / artifact / session / run / model）。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use agent_domain::{
    ActorId, ArtifactId, CommandId, ConnectionId, GuiClientId, ProviderId, QueryId, RunId,
    SessionId, Timestamp, ToolCallId, WorkspaceId,
};
use app_service::{CommandRouter, RouterConfig, RunSupervisorStats};
use core_api::{
    ActorIdentity, ApiVersion, AppCommand, AppCommandEnvelope, AppQuery, AppQueryEnvelope,
    AppResponse, AppResponseEnvelope, ClientContextSnapshot, CommandSource, RunState,
    WorkspaceRelativePath, API_VERSION,
};
use diff_service::{DiffFile, FileStatus};
use provider_api::ModelProvider;
use serde_json::{json, Value};

static NEXT_ID: AtomicU64 = AtomicU64::new(0);

fn unique(prefix: &str) -> String {
    format!("{prefix}-{}", NEXT_ID.fetch_add(1, Ordering::SeqCst))
}

fn cli_source() -> CommandSource {
    CommandSource::LocalCli {
        terminal_session_id: Some("terminal-1".into()),
    }
}

fn cli_identity() -> ActorIdentity {
    ActorIdentity::LocalUser {
        actor_id: ActorId::from("tester"),
        display_name: Some("Tester".into()),
    }
}

/// P18-2：本地 CLI 身份解析出的真实租户（local/default）。
fn local_tenant() -> agent_domain::TenantId {
    tenant_service::IdentityContext::local().tenant_id
}

fn gui_source() -> CommandSource {
    CommandSource::LocalGui {
        client_id: GuiClientId::from("gui-1"),
    }
}

fn gui_identity() -> ActorIdentity {
    ActorIdentity::AuthenticatedClient {
        actor_id: ActorId::from("tester"),
        subject: "user-x".into(),
    }
}

fn command(
    source: CommandSource,
    identity: ActorIdentity,
    command: AppCommand,
) -> AppCommandEnvelope {
    AppCommandEnvelope {
        api_version: API_VERSION,
        command_id: CommandId::from(unique("cmd")),
        source,
        identity,
        expected_revision: None,
        idempotency_key: None,
        issued_at: Timestamp::from_unix_millis(1),
        command,
    }
}

fn query(source: CommandSource, identity: ActorIdentity, query: AppQuery) -> AppQueryEnvelope {
    AppQueryEnvelope {
        api_version: API_VERSION,
        request_id: QueryId::from(unique("req")),
        source,
        identity,
        issued_at: Timestamp::from_unix_millis(1),
        query,
    }
}

fn router_with_mock_provider(script: test_support::MockScript) -> CommandRouter {
    let router = CommandRouter::new(RouterConfig::default());
    let provider: Arc<dyn ModelProvider> =
        Arc::new(test_support::MockProvider::new(script).with_id(ProviderId::from("mock")));
    router.register_provider(provider);
    router
}

/// 创建真实存在的临时目录（workspace-service 会 canonicalize 路径）。
fn temp_workspace_dir() -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("pawork-app-service-{}", unique("ws")));
    std::fs::create_dir_all(&path).expect("create temp workspace dir");
    path
}

fn workspace_id_from(response: &AppResponseEnvelope) -> WorkspaceId {
    match &response.response {
        AppResponse::Data(value) => WorkspaceId::from(
            value
                .get("id")
                .and_then(Value::as_str)
                .expect("workspace id"),
        ),
        other => panic!("expected workspace data, got {other:?}"),
    }
}

fn session_id_from(response: &AppResponseEnvelope) -> SessionId {
    match &response.response {
        AppResponse::Data(value) => SessionId::from(
            value
                .get("session_id")
                .and_then(Value::as_str)
                .expect("session id"),
        ),
        other => panic!("expected session data, got {other:?}"),
    }
}

fn add_workspace(router: &CommandRouter, root: &std::path::Path) -> WorkspaceId {
    let response = router.dispatch(command(
        cli_source(),
        cli_identity(),
        AppCommand::WorkspaceAdd {
            root_path: root.to_string_lossy().into_owned(),
        },
    ));
    workspace_id_from(&response)
}

fn create_session(router: &CommandRouter, workspace_id: &WorkspaceId) -> SessionId {
    let response = router.dispatch(command(
        cli_source(),
        cli_identity(),
        AppCommand::SessionCreate {
            workspace_id: workspace_id.clone(),
            title: Some("integration".into()),
        },
    ));
    session_id_from(&response)
}

async fn wait_until<F: Fn() -> bool>(condition: F, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if condition() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    condition()
}

#[tokio::test]
async fn idempotent_run_start_replays_without_duplicate_run() {
    let router = router_with_mock_provider(test_support::MockScript::new().complete());
    let workspace_id = add_workspace(&router, &temp_workspace_dir());
    let session_id = create_session(&router, &workspace_id);

    let envelope = AppCommandEnvelope {
        api_version: API_VERSION,
        command_id: CommandId::from("cmd-idempotent"),
        source: cli_source(),
        identity: cli_identity(),
        expected_revision: None,
        idempotency_key: Some("network-retry-1".into()),
        issued_at: Timestamp::from_unix_millis(1),
        command: AppCommand::RunStart {
            session_id: session_id.clone(),
            user_message: "hello".into(),
            model: None,
            profile: None,
        },
    };

    let first = router.dispatch(envelope.clone());
    assert!(matches!(first.response, AppResponse::Accepted { .. }));

    // 网络重试：同 command_id + 同 idempotency_key 重发，重放首次响应，不重复建 Run。
    let second = router.dispatch(envelope.clone());
    assert_eq!(first, second, "重放应返回与首次完全一致的响应");
    assert!(matches!(second.response, AppResponse::Accepted { .. }));

    // 同 idempotency_key、不同 command_id 同样去重。
    let third = router.dispatch(AppCommandEnvelope {
        command_id: CommandId::from("cmd-idempotent-retry-2"),
        ..envelope
    });
    assert!(matches!(third.response, AppResponse::Accepted { .. }));

    assert_eq!(router.supervisor().total(), 1, "幂等重试不得重复建 Run");
    assert_eq!(router.aggregate().runs().len(), 1);
    let stats: RunSupervisorStats = router.supervisor().stats();
    assert_eq!(stats.started, 1);
}

/// P17-7 评审 #3 回归：并发来源各自从自己的 Accepted 响应取 run id 绑定，
/// 不依赖全局 `last_started_run`。两个并发 RunStart 的 run id 必须互不相同，
/// 且各自绑定到自己的会话；取消其中一个不得影响另一个。
#[tokio::test]
async fn concurrent_run_starts_carry_distinct_run_ids_bound_to_their_own_runs() {
    let router = router_with_mock_provider(test_support::MockScript::new().wait_for_cancellation());
    let workspace_id = add_workspace(&router, &temp_workspace_dir());
    let session_a = create_session(&router, &workspace_id);
    let session_b = create_session(&router, &workspace_id);

    let response_a = router.dispatch(command(
        cli_source(),
        cli_identity(),
        AppCommand::RunStart {
            session_id: session_a.clone(),
            user_message: "run a".into(),
            model: None,
            profile: None,
        },
    ));
    let response_b = router.dispatch(command(
        gui_source(),
        gui_identity(),
        AppCommand::RunStart {
            session_id: session_b.clone(),
            user_message: "run b".into(),
            model: None,
            profile: None,
        },
    ));
    let AppResponse::Accepted {
        run_id: Some(run_id_a),
        ..
    } = response_a.response
    else {
        panic!(
            "RunStart A 应 Accepted 且携带 run id，got {:?}",
            response_a.response
        );
    };
    let AppResponse::Accepted {
        run_id: Some(run_id_b),
        ..
    } = response_b.response
    else {
        panic!(
            "RunStart B 应 Accepted 且携带 run id，got {:?}",
            response_b.response
        );
    };
    assert_ne!(
        run_id_a, run_id_b,
        "并发 RunStart 必须返回各自确定的 run id"
    );
    assert_eq!(router.aggregate().runs().len(), 2, "两个 run 都应被创建");

    // 响应中的 run id 必须绑定到发起它的会话，而不是"最近一次启动的 run"。
    let record_a = router
        .aggregate()
        .get_run(&run_id_a, &local_tenant())
        .expect("run a 存在");
    let record_b = router
        .aggregate()
        .get_run(&run_id_b, &local_tenant())
        .expect("run b 存在");
    assert_eq!(record_a.session_id, session_a);
    assert_eq!(record_b.session_id, session_b);

    // 取消 A 只作用于 A：B 保持活跃。
    let outcome = router.supervisor().cancel(&run_id_a).expect("cancel a");
    assert!(!outcome.already_cancelled);
    assert!(
        wait_until(
            || {
                router
                    .aggregate()
                    .get_run(&run_id_a, &local_tenant())
                    .map(|run| run.state == RunState::Cancelled)
                    .unwrap_or(false)
            },
            Duration::from_secs(5),
        )
        .await,
        "run A 应进入 Cancelled"
    );
    assert!(!router.supervisor().is_active(&run_id_a), "run A 已取消");
    assert!(
        router.supervisor().is_active(&run_id_b),
        "并发 run B 不应受 A 取消影响"
    );
}

#[tokio::test]
async fn idempotency_replay_is_tenant_scoped() {
    use tenant_service::IdentityContext;

    #[derive(Clone)]
    struct TenantResolver;

    impl tenant_service::IdentityResolver for TenantResolver {
        fn resolve(
            &self,
            principal: Option<&str>,
        ) -> Result<IdentityContext, tenant_service::IdentityError> {
            match principal {
                Some("authenticated_client:tenant-a") => Ok(IdentityContext::new(
                    agent_domain::TenantId::new("tenant-a"),
                    agent_domain::PrincipalId::new("principal-a"),
                )),
                Some("authenticated_client:tenant-b") => Ok(IdentityContext::new(
                    agent_domain::TenantId::new("tenant-b"),
                    agent_domain::PrincipalId::new("principal-b"),
                )),
                Some(value) if !value.trim().is_empty() => Ok(IdentityContext::local()),
                Some(_) => Err(tenant_service::IdentityError::EmptyPrincipal),
                None => Err(tenant_service::IdentityError::MissingIdentity(
                    "no principal".into(),
                )),
            }
        }
    }

    let router =
        CommandRouter::with_identity_resolver(RouterConfig::default(), Arc::new(TenantResolver));
    let workspace_id = add_workspace(&router, &temp_workspace_dir());
    let tenant_a = ActorIdentity::AuthenticatedClient {
        actor_id: ActorId::from("actor-a"),
        subject: "tenant-a".into(),
    };
    let tenant_b = ActorIdentity::AuthenticatedClient {
        actor_id: ActorId::from("actor-b"),
        subject: "tenant-b".into(),
    };
    let shared = AppCommandEnvelope {
        api_version: API_VERSION,
        command_id: CommandId::from("shared-command"),
        source: gui_source(),
        identity: tenant_a.clone(),
        expected_revision: None,
        idempotency_key: Some("shared-key".into()),
        issued_at: Timestamp::from_unix_millis(1),
        command: AppCommand::SessionCreate {
            workspace_id,
            title: Some("tenant-owned".into()),
        },
    };

    let first_a = router.dispatch(shared.clone());
    let session_a = session_id_from(&first_a);
    let first_b = router.dispatch(AppCommandEnvelope {
        identity: tenant_b.clone(),
        ..shared.clone()
    });
    let session_b = session_id_from(&first_b);
    assert_ne!(session_a, session_b, "Tenant B must execute, not replay A");
    assert!(router
        .aggregate()
        .get_session(&session_a, &agent_domain::TenantId::new("tenant-a"))
        .is_some());
    assert!(router
        .aggregate()
        .get_session(&session_b, &agent_domain::TenantId::new("tenant-b"))
        .is_some());

    let replay_a = router.dispatch(AppCommandEnvelope {
        command_id: CommandId::from("tenant-a-key-retry"),
        ..shared.clone()
    });
    let replay_b = router.dispatch(AppCommandEnvelope {
        command_id: CommandId::from("tenant-b-key-retry"),
        identity: tenant_b,
        ..shared
    });
    assert_eq!(replay_a, first_a, "Tenant A replays only Tenant A response");
    assert_eq!(replay_b, first_b, "Tenant B replays only Tenant B response");
}

#[tokio::test]
async fn sources_and_identities_are_recorded_per_command() {
    let router = router_with_mock_provider(test_support::MockScript::new().complete());

    let cli_response = router.dispatch(command(
        cli_source(),
        cli_identity(),
        AppCommand::CoreInitialize,
    ));
    assert!(matches!(cli_response.response, AppResponse::Data(_)));

    let gui_response = router.dispatch(command(
        gui_source(),
        gui_identity(),
        AppCommand::CoreInitialize,
    ));
    assert!(matches!(gui_response.response, AppResponse::Data(_)));

    let sources = router.source_stats();
    assert_eq!(sources.get("local_cli"), Some(&1));
    assert_eq!(sources.get("local_gui"), Some(&1));
    let identities = router.identity_stats();
    assert_eq!(identities.get("local_user:tester"), Some(&1));
    assert_eq!(identities.get("authenticated_client:user-x"), Some(&1));
    assert_eq!(router.commands_handled(), 2);

    // Run 记录来源：GUI 发起的 run 落 LocalGui。
    let workspace_id = add_workspace(&router, &temp_workspace_dir());
    let session_id = create_session(&router, &workspace_id);
    let run_response = router.dispatch(command(
        gui_source(),
        gui_identity(),
        AppCommand::RunStart {
            session_id: session_id.clone(),
            user_message: "from gui".into(),
            model: None,
            profile: None,
        },
    ));
    let AppResponse::Accepted {
        run_id: Some(run_id),
        ..
    } = run_response.response
    else {
        panic!("RunStart 应 Accepted 且携带 run id");
    };
    let run = router
        .aggregate()
        .get_run(&run_id, &local_tenant())
        .expect("run recorded");
    assert_eq!(
        run.source,
        gui_source(),
        "RunRecord 应记录真实 CommandSource"
    );
    assert_eq!(run.session_id, session_id);
    let session = router
        .aggregate()
        .get_session(&run.session_id, &local_tenant())
        .expect("session");
    assert_eq!(session.workspace_id, workspace_id);
}

#[tokio::test]
async fn run_start_without_provider_returns_structured_authentication_error() {
    // 不注册任何 Provider。
    let router = CommandRouter::new(RouterConfig::default());
    let workspace_id = add_workspace(&router, &temp_workspace_dir());
    let session_id = create_session(&router, &workspace_id);

    let response = router.dispatch(command(
        cli_source(),
        cli_identity(),
        AppCommand::RunStart {
            session_id: session_id.clone(),
            user_message: "hello".into(),
            model: None,
            profile: None,
        },
    ));
    match &response.response {
        AppResponse::Error(context) => {
            assert_eq!(
                context.category,
                agent_domain::ErrorCategory::Authentication,
                "无凭据/未注册 Provider 应返回结构化 Authentication 错误而非 panic"
            );
            assert!(!context.message.is_empty());
        }
        other => panic!("expected structured error, got {other:?}"),
    }
    assert_eq!(router.aggregate().runs().len(), 0, "失败不得残留 Run");
    assert_eq!(router.supervisor().total(), 0);
}

#[tokio::test]
async fn snapshot_and_queries_reflect_aggregate() {
    let router = router_with_mock_provider(test_support::MockScript::new().complete());
    let workspace_id = add_workspace(&router, &temp_workspace_dir());
    let session_id = create_session(&router, &workspace_id);
    let run_response = router.dispatch(command(
        cli_source(),
        cli_identity(),
        AppCommand::RunStart {
            session_id: session_id.clone(),
            user_message: "snapshot me".into(),
            model: None,
            profile: None,
        },
    ));
    let AppResponse::Accepted {
        run_id: Some(run_id),
        ..
    } = &run_response.response
    else {
        panic!("RunStart 应 Accepted 且携带 run id");
    };
    let run_id = run_id.clone();
    let completed = wait_until(
        || {
            router
                .aggregate()
                .get_run(&run_id, &local_tenant())
                .is_some_and(|run| run.state == core_api::RunState::Completed)
        },
        Duration::from_secs(5),
    )
    .await;
    assert!(completed, "run 应在 5s 内完成");

    // 种子 diff 与 artifact，再走查询面。
    router
        .aggregate()
        .seed_diff(
            &workspace_id,
            vec![DiffFile {
                path: "src/main.rs".into(),
                status: FileStatus::Modified,
                additions: 3,
                deletions: 1,
                ..DiffFile::default()
            }],
        )
        .expect("seed diff");
    router
        .aggregate()
        .put_artifact(ArtifactId::from("artifact-1"), 42, "text/plain".into())
        .expect("put artifact");

    // SnapshotFetch：workspaces / sessions / runs / approvals / providers 齐全。
    let snapshot_response =
        router.dispatch_query(query(cli_source(), cli_identity(), AppQuery::SnapshotFetch));
    match &snapshot_response.response {
        AppResponse::Data(value) => {
            assert_eq!(value["workspaces"].as_array().map(Vec::len), Some(1));
            assert_eq!(value["sessions"].as_array().map(Vec::len), Some(1));
            let runs = value["runs"].as_array().expect("runs");
            assert_eq!(runs.len(), 1);
            assert_eq!(runs[0]["state"], "completed");
            assert_eq!(value["providers"].as_array().map(Vec::len), Some(1));
            assert_eq!(value["artifacts"].as_array().map(Vec::len), Some(1));
            assert!(value["revision"].as_u64().unwrap_or(0) > 0);
        }
        other => panic!("expected snapshot data, got {other:?}"),
    }

    // SessionGet / RunStatus。
    let session_response = router.dispatch_query(query(
        cli_source(),
        cli_identity(),
        AppQuery::SessionGet {
            session_id: session_id.clone(),
        },
    ));
    match &session_response.response {
        AppResponse::Data(value) => {
            assert_eq!(value["session_id"], json!(session_id.as_str()));
            assert_eq!(value["workspace_id"], json!(workspace_id.as_str()));
        }
        other => panic!("expected session data, got {other:?}"),
    }
    let run_response = router.dispatch_query(query(
        cli_source(),
        cli_identity(),
        AppQuery::RunStatus {
            run_id: run_id.clone(),
        },
    ));
    match &run_response.response {
        AppResponse::Data(value) => {
            assert_eq!(value["run_id"], json!(run_id.as_str()));
            assert_eq!(value["state"], "completed");
            assert_eq!(value["source"]["type"], "local_cli");
        }
        other => panic!("expected run data, got {other:?}"),
    }

    // ModelList：按 provider 过滤目录。
    let models_response = router.dispatch_query(query(
        cli_source(),
        cli_identity(),
        AppQuery::ModelList {
            provider_id: Some(ProviderId::from("openai")),
        },
    ));
    match &models_response.response {
        AppResponse::Data(value) => {
            let entries = value.as_array().expect("model list");
            assert!(!entries.is_empty(), "openai 目录应有内置模型");
            assert!(entries.iter().all(|entry| entry["provider"] == "openai"));
        }
        other => panic!("expected model list, got {other:?}"),
    }

    // DiffListFiles / DiffGet。
    let diffs_response = router.dispatch_query(query(
        cli_source(),
        cli_identity(),
        AppQuery::DiffListFiles {
            workspace_id: workspace_id.clone(),
        },
    ));
    match &diffs_response.response {
        AppResponse::Data(value) => {
            let files = value.as_array().expect("diff files");
            assert_eq!(files.len(), 1);
            assert_eq!(files[0]["path"], "src/main.rs");
        }
        other => panic!("expected diff list, got {other:?}"),
    }
    let diff_response = router.dispatch_query(query(
        cli_source(),
        cli_identity(),
        AppQuery::DiffGet {
            workspace_id: workspace_id.clone(),
            path: WorkspaceRelativePath::new("src/main.rs").expect("relative path"),
            cursor: None,
        },
    ));
    match &diff_response.response {
        AppResponse::Data(value) => {
            assert_eq!(value["path"], "src/main.rs");
            assert_eq!(value["status"], "modified");
        }
        other => panic!("expected diff data, got {other:?}"),
    }

    // ArtifactRead → AppResponse::Artifact。
    let artifact_response = router.dispatch_query(query(
        cli_source(),
        cli_identity(),
        AppQuery::ArtifactRead {
            artifact_id: ArtifactId::from("artifact-1"),
            offset: 0,
            limit: 1024,
        },
    ));
    match &artifact_response.response {
        AppResponse::Artifact {
            artifact_id,
            byte_length,
            media_type,
        } => {
            assert_eq!(artifact_id.as_str(), "artifact-1");
            assert_eq!(*byte_length, 42);
            assert_eq!(media_type, "text/plain");
        }
        other => panic!("expected artifact, got {other:?}"),
    }

    // 不存在的查询目标 → NotFound 错误。
    let missing_response = router.dispatch_query(query(
        cli_source(),
        cli_identity(),
        AppQuery::SessionGet {
            session_id: SessionId::from("nope"),
        },
    ));
    assert!(matches!(
        missing_response.response,
        AppResponse::Error(ref context)
            if context.category == agent_domain::ErrorCategory::NotFound
    ));

    // diff/artifact 数据也进入快照。
    let snapshot_response =
        router.dispatch_query(query(cli_source(), cli_identity(), AppQuery::SnapshotFetch));
    match &snapshot_response.response {
        AppResponse::Data(value) => {
            assert_eq!(
                value["revision"].as_u64(),
                Some(router.aggregate().revision())
            );
        }
        other => panic!("expected snapshot data, got {other:?}"),
    }
}

#[test]
fn run_start_without_runtime_returns_structured_error() {
    let router = CommandRouter::new(RouterConfig::default());
    let workspace_id = add_workspace(&router, &temp_workspace_dir());
    let session_id = create_session(&router, &workspace_id);
    // 无 tokio 运行时（当前测试线程）；RunStart 应返回 NoRuntime 而非 panic。
    let response = router.dispatch(command(
        cli_source(),
        cli_identity(),
        AppCommand::RunStart {
            session_id,
            user_message: "hello".into(),
            model: None,
            profile: None,
        },
    ));
    match &response.response {
        AppResponse::Error(context) => {
            assert_eq!(
                context.category,
                agent_domain::ErrorCategory::Unavailable,
                "无运行时错误应映射为 Unavailable"
            );
            assert!(context.retryable);
        }
        other => panic!("expected error, got {other:?}"),
    }
    assert_eq!(router.aggregate().runs().len(), 0, "失败不得残留 Run");
}

#[test]
fn unknown_tool_call_approval_and_missing_session_return_errors() {
    let router = CommandRouter::new(RouterConfig::default());
    let response = router.dispatch(command(
        cli_source(),
        cli_identity(),
        AppCommand::ToolApprove {
            run_id: RunId::from("run-1"),
            tool_call_id: ToolCallId::from("call-1"),
            decision: core_api::ApprovalDecision::Deny,
        },
    ));
    assert!(matches!(
        response.response,
        AppResponse::Error(ref context)
            if context.category == agent_domain::ErrorCategory::NotFound
    ));

    let response = router.dispatch(command(
        cli_source(),
        cli_identity(),
        AppCommand::SessionOpen {
            session_id: SessionId::from("nope"),
        },
    ));
    assert!(matches!(
        response.response,
        AppResponse::Error(ref context)
            if context.category == agent_domain::ErrorCategory::NotFound
    ));
}

#[test]
fn api_version_mismatch_is_rejected_with_structured_error() {
    let router = CommandRouter::new(RouterConfig::default());
    let envelope = command(cli_source(), cli_identity(), AppCommand::CoreInitialize);
    let incompatible = AppCommandEnvelope {
        api_version: ApiVersion {
            major: API_VERSION.major + 1,
            minor: 0,
        },
        ..envelope
    };
    let response = router.dispatch(incompatible);
    match &response.response {
        AppResponse::Error(context) => {
            assert_eq!(
                context.category,
                agent_domain::ErrorCategory::InvalidRequest
            );
            assert!(context.message.contains("incompatible"));
        }
        other => panic!("expected incompatible api version error, got {other:?}"),
    }
}

#[test]
fn legacy_service_route_is_unavailable_until_tool_runtime() {
    let router = CommandRouter::new(RouterConfig::default());
    let response = router.dispatch(command(
        cli_source(),
        cli_identity(),
        AppCommand::RunTool {
            run_id: RunId::from("run-1"),
            tool_name: "shell".into(),
            input: json!({}),
        },
    ));
    match &response.response {
        AppResponse::Error(context) => {
            assert_eq!(
                context.category,
                agent_domain::ErrorCategory::Unavailable,
                "RunTool 在 tool-runtime 集成前应返回 Unavailable"
            );
        }
        other => panic!("expected unavailable error, got {other:?}"),
    }
}

#[test]
fn terminal_and_git_stage_commands_round_trip() {
    let router = CommandRouter::new(RouterConfig::default());
    let workspace_id = WorkspaceId::from("ws-1");
    let terminal_response = router.dispatch(command(
        cli_source(),
        cli_identity(),
        AppCommand::TerminalCreate {
            workspace_id: workspace_id.clone(),
            working_directory: None,
        },
    ));
    let terminal_id = match &terminal_response.response {
        AppResponse::Data(value) => value["terminal_session_id"]
            .as_str()
            .expect("terminal id")
            .to_string(),
        other => panic!("expected terminal data, got {other:?}"),
    };
    let write_response = router.dispatch(command(
        cli_source(),
        cli_identity(),
        AppCommand::TerminalWrite {
            terminal_session_id: terminal_id.clone(),
            data: "echo hi\n".into(),
        },
    ));
    assert!(matches!(write_response.response, AppResponse::Data(_)));

    let stage_response = router.dispatch(command(
        cli_source(),
        cli_identity(),
        AppCommand::GitStage {
            workspace_id: workspace_id.clone(),
            paths: vec![WorkspaceRelativePath::new("a.txt").expect("path")],
        },
    ));
    assert!(matches!(stage_response.response, AppResponse::Data(_)));
}

fn empty_client_context() -> ClientContextSnapshot {
    ClientContextSnapshot {
        revision: 1,
        active_document: None,
        open_documents: vec![],
        diagnostics: vec![],
    }
}

fn assert_context_denied(response: &AppResponseEnvelope) {
    match &response.response {
        AppResponse::Error(context) => {
            assert_eq!(context.category, agent_domain::ErrorCategory::Authorization);
            assert!(
                context.message.contains("session_client_context_replace"),
                "unexpected authorization message: {}",
                context.message
            );
        }
        other => panic!("expected authorization error, got {other:?}"),
    }
}

#[test]
fn session_client_context_replace_is_source_gated() {
    let router = CommandRouter::new(RouterConfig::default());
    let workspace_id = add_workspace(&router, &temp_workspace_dir());
    let session_id = create_session(&router, &workspace_id);
    let snapshot = empty_client_context();

    for source in [
        gui_source(),
        CommandSource::RemoteGui {
            client_id: GuiClientId::from("remote-gui"),
            connection_id: ConnectionId::from("conn-1"),
        },
        CommandSource::Plugin,
        CommandSource::Mcp,
    ] {
        let response = router.dispatch(command(
            source,
            cli_identity(),
            AppCommand::SessionClientContextReplace {
                session_id: session_id.clone(),
                snapshot: snapshot.clone(),
            },
        ));
        assert_context_denied(&response);
        assert!(
            router.aggregate().client_context(&session_id).is_none(),
            "denied sources must not persist client context"
        );
    }

    for source in [cli_source(), CommandSource::Automation] {
        let allowed_session = create_session(&router, &workspace_id);
        let response = router.dispatch(command(
            source,
            cli_identity(),
            AppCommand::SessionClientContextReplace {
                session_id: allowed_session.clone(),
                snapshot: snapshot.clone(),
            },
        ));
        match &response.response {
            AppResponse::Data(value) => {
                assert_eq!(value["replaced"], true);
                assert_eq!(value["revision"], 1);
            }
            other => panic!("expected allowed replace, got {other:?}"),
        }
        assert_eq!(
            router.aggregate().client_context(&allowed_session),
            Some(snapshot.clone())
        );
    }
}

/// P18-2：System 显式映射为 local/system，不能与 local/user 混同。
#[tokio::test]
async fn system_identity_uses_explicit_local_system_principal() {
    let router = CommandRouter::new(RouterConfig::default());
    let workspace_id = add_workspace(&router, &temp_workspace_dir());
    let response = router.dispatch(command(
        cli_source(),
        ActorIdentity::System,
        AppCommand::SessionCreate {
            workspace_id,
            title: Some("system session".into()),
        },
    ));
    let session_id = session_id_from(&response);
    let session = router
        .aggregate()
        .get_session(&session_id, &local_tenant())
        .expect("system session");
    assert_eq!(session.principal_id.as_str(), "local/system");
}

/// P18-2 身份传播：Session 记录携带解析出的真实 tenant/principal，且
/// SessionGet / RunStatus 查询按调用者租户隔离，跨租户视同不存在。
#[tokio::test]
async fn sessions_and_runs_are_created_and_queried_under_resolved_tenant() {
    use tenant_service::IdentityContext;

    #[derive(Clone)]
    struct MappingResolver;

    impl tenant_service::IdentityResolver for MappingResolver {
        fn resolve(
            &self,
            principal: Option<&str>,
        ) -> Result<IdentityContext, tenant_service::IdentityError> {
            match principal {
                // 空 subject（principal 以 `:` 结尾、无实际主体）：fail-closed。
                Some(p) if p.ends_with(':') => Err(tenant_service::IdentityError::EmptyPrincipal),
                Some(p) if p.ends_with("tenant-a:user") => Ok(IdentityContext::new(
                    agent_domain::TenantId::new("tenant-a"),
                    agent_domain::PrincipalId::new("tenant-a:user"),
                )),
                Some(p) if p.ends_with("tenant-b:user") => Ok(IdentityContext::new(
                    agent_domain::TenantId::new("tenant-b"),
                    agent_domain::PrincipalId::new("tenant-b:user"),
                )),
                Some(_) => Ok(IdentityContext::local()),
                None => Err(tenant_service::IdentityError::MissingIdentity(
                    "no principal".into(),
                )),
            }
        }
    }

    // P18-9 deny-first：未知非 local/default 租户回落 Viewer，本测试的
    // tenant-a / tenant-b 按「已配置租户」播种显式 User profile（身份传播
    // 测试本身不覆盖策略矩阵）。
    use tenant_service::TenantPolicyEngine;
    let engine = Arc::new(tenant_service::InMemoryTenantPolicyEngine::default());
    for tenant in ["tenant-a", "tenant-b"] {
        engine.set_policy(
            agent_domain::TenantId::new(tenant),
            tenant_service::TenantPolicy {
                permission_profile: Some(tenant_service::PermissionProfile {
                    default_role: Some(tenant_service::PrincipalRole::User),
                    ..Default::default()
                }),
                ..Default::default()
            },
        );
    }
    let router = CommandRouter::with_tenant_policy(
        RouterConfig::default(),
        Arc::new(MappingResolver),
        engine,
    );
    let workspace_id = add_workspace(&router, &temp_workspace_dir());

    let tenant_a = ActorIdentity::AuthenticatedClient {
        actor_id: ActorId::from("a"),
        subject: "tenant-a:user".into(),
    };
    let tenant_b = ActorIdentity::AuthenticatedClient {
        actor_id: ActorId::from("b"),
        subject: "tenant-b:user".into(),
    };

    // Tenant A 创建 session：记录必须携带真实 tenant/principal。
    let created = router.dispatch(command(
        gui_source(),
        tenant_a.clone(),
        AppCommand::SessionCreate {
            workspace_id: workspace_id.clone(),
            title: Some("tenant-a session".into()),
        },
    ));
    let session_id = session_id_from(&created);
    let stored = router
        .aggregate()
        .get_session(&session_id, &agent_domain::TenantId::new("tenant-a"))
        .expect("session visible to tenant-a");
    assert_eq!(stored.tenant_id.as_str(), "tenant-a");
    assert_eq!(stored.principal_id.as_str(), "tenant-a:user");

    // Tenant B 查询同一 session：跨租户视同不存在（不泄漏存在性）。
    let cross = router.dispatch_query(query(
        gui_source(),
        tenant_b.clone(),
        AppQuery::SessionGet {
            session_id: session_id.clone(),
        },
    ));
    match cross.response {
        AppResponse::Error(context) => {
            assert_eq!(context.category, agent_domain::ErrorCategory::NotFound);
        }
        other => panic!("expected not-found for cross-tenant session, got {other:?}"),
    }

    // Tenant A 查询同一 session：命中且返回真实租户。
    let own = router.dispatch_query(query(
        gui_source(),
        tenant_a.clone(),
        AppQuery::SessionGet {
            session_id: session_id.clone(),
        },
    ));
    let AppResponse::Data(value) = own.response else {
        panic!("expected own-tenant session data, got {:?}", own.response);
    };
    assert_eq!(value["tenant_id"], json!("tenant-a"));
    assert_eq!(value["principal_id"], json!("tenant-a:user"));

    // 缺省主体（空 principal）fail-closed：查询被拒绝，不落到默认租户。
    let empty = ActorIdentity::AuthenticatedClient {
        actor_id: ActorId::from("empty"),
        subject: "".into(),
    };
    let denied = router.dispatch_query(query(
        gui_source(),
        empty,
        AppQuery::SessionGet {
            session_id: session_id.clone(),
        },
    ));
    match denied.response {
        AppResponse::Error(context) => {
            assert_eq!(context.category, agent_domain::ErrorCategory::Authorization);
        }
        other => panic!("expected authorization error, got {other:?}"),
    }

    // 为 Tenant A 建立一个活跃 run；Tenant B 对其所有 mutating command 都必须
    // 得到 NotFound，且 SnapshotFetch 不得泄漏 session/run/approval。
    let provider: Arc<dyn ModelProvider> = Arc::new(
        test_support::MockProvider::new(test_support::MockScript::new().wait_for_cancellation())
            .with_id(ProviderId::from("mock")),
    );
    router.register_provider(provider);
    let started = router.dispatch(command(
        gui_source(),
        tenant_a.clone(),
        AppCommand::RunStart {
            session_id: session_id.clone(),
            user_message: "tenant scoped run".into(),
            model: None,
            profile: None,
        },
    ));
    let run_id = match started.response {
        AppResponse::Accepted {
            run_id: Some(id), ..
        } => id,
        other => panic!("Tenant A RunStart should be Accepted with run id, got {other:?}"),
    };

    for command_value in [
        AppCommand::RunCancel {
            run_id: run_id.clone(),
        },
        AppCommand::RunRetry {
            run_id: run_id.clone(),
        },
        AppCommand::ToolApprove {
            run_id: run_id.clone(),
            tool_call_id: ToolCallId::from("cross-tenant-tool"),
            decision: core_api::ApprovalDecision::ApproveOnce,
        },
    ] {
        let response = router.dispatch(command(gui_source(), tenant_b.clone(), command_value));
        match response.response {
            AppResponse::Error(context) => {
                assert_eq!(context.category, agent_domain::ErrorCategory::NotFound);
            }
            other => panic!("expected cross-tenant not-found, got {other:?}"),
        }
    }

    let tenant_b_snapshot =
        router.dispatch_query(query(gui_source(), tenant_b, AppQuery::SnapshotFetch));
    let AppResponse::Data(snapshot) = tenant_b_snapshot.response else {
        panic!("expected tenant-b snapshot data");
    };
    assert_eq!(snapshot["sessions"].as_array().map(Vec::len), Some(0));
    assert_eq!(snapshot["runs"].as_array().map(Vec::len), Some(0));
    assert_eq!(snapshot["approvals"].as_array().map(Vec::len), Some(0));

    let tenant_a_snapshot = router.dispatch_query(query(
        gui_source(),
        tenant_a.clone(),
        AppQuery::SnapshotFetch,
    ));
    let AppResponse::Data(snapshot) = tenant_a_snapshot.response else {
        panic!("expected tenant-a snapshot data");
    };
    assert_eq!(snapshot["sessions"].as_array().map(Vec::len), Some(1));
    assert_eq!(snapshot["runs"].as_array().map(Vec::len), Some(1));

    // 清理活跃任务，避免测试退出时遗留后台 run。
    let cancelled = router.dispatch(command(
        gui_source(),
        tenant_a,
        AppCommand::RunCancel { run_id },
    ));
    assert!(matches!(cancelled.response, AppResponse::Data(_)));
}

#[test]
fn dispatch_rejects_blank_identity_from_custom_resolver() {
    #[derive(Clone)]
    struct BlankIdentityResolver;

    impl tenant_service::IdentityResolver for BlankIdentityResolver {
        fn resolve(
            &self,
            principal: Option<&str>,
        ) -> Result<tenant_service::IdentityContext, tenant_service::IdentityError> {
            match principal {
                Some("authenticated_client:blank-tenant") => {
                    Ok(tenant_service::IdentityContext::new(
                        agent_domain::TenantId::new(" \t"),
                        agent_domain::PrincipalId::new("client/blank-tenant"),
                    ))
                }
                Some("authenticated_client:blank-principal") => {
                    Ok(tenant_service::IdentityContext::new(
                        agent_domain::TenantId::new("tenant-a"),
                        agent_domain::PrincipalId::new("\n"),
                    ))
                }
                _ => Err(tenant_service::IdentityError::MissingIdentity(
                    "test resolver requires a recognized principal".into(),
                )),
            }
        }
    }

    let router = CommandRouter::with_identity_resolver(
        RouterConfig::default(),
        Arc::new(BlankIdentityResolver),
    );

    for (subject, expected_fragment) in
        [("blank-tenant", "tenant"), ("blank-principal", "principal")]
    {
        let response = router.dispatch(command(
            gui_source(),
            ActorIdentity::AuthenticatedClient {
                actor_id: ActorId::from(subject),
                subject: subject.into(),
            },
            AppCommand::CoreInitialize,
        ));

        match response.response {
            AppResponse::Error(context) => {
                assert_eq!(context.category, agent_domain::ErrorCategory::Authorization);
                assert!(context.message.contains(expected_fragment));
            }
            other => panic!("blank custom identity must fail closed, got {other:?}"),
        }
    }

    assert!(!router.aggregate().snapshot().core_ready);
}
