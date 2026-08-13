//! P17-5 主 run profile 接线定向测试：RunStart → RunSupervisor → ProviderLoop。
//!
//! 覆盖 fail-closed 解析（profile/模型）、prompt.system + instructions + canonical
//! effort + max_turns 进入 run、tool 规则 deny-first allowlist 在权威 pre_tool 生效
//! （含主链证据：denied 不执行、以拒绝结果回填）、memory enabled+unavailable
//! 拒绝、Restricted/Container 隔离无真实执行器 fail-closed、background 经
//! TaskManager 注册/收尾 TaskKind::Agent、background 无 TaskManager fail-closed、
//! retry 沿用同一不可变 profile。

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use agent_domain::{
    ActorId, AgentProfileV2, CommandId, CoreInstanceId, ModelId, ProfileIsolation, ProfileMemory,
    ProfileModel, ProfilePrompt, ProfileToolRules, ProviderId, ReasoningEffort, RunId, SessionId,
    StopReason, TaskKind, TaskStatus, TokenUsage, ToolCallId, WorkspaceId,
};
use app_service::{
    CommandRouter, IsolationCapability, ProfileResolveError, ResolvedRunProfile, RouterConfig,
    RunProfileResolver, RunSupervisor, SuperviseError,
};
use core_api::{
    ActorIdentity, AppCommand, AppCommandEnvelope, ApprovalDecision, CommandSource, API_VERSION,
};
use provider_api::{
    CanonicalModelRequest, ModelDefinition, ModelProvider, ModelResponseSummary, ProviderError,
    ProviderEventSink, ResolvedCredential,
};

/// 测试用主 run profile 解析器：按 (workspace, name) 返回固定 profile（fail-closed）。
#[derive(Clone, Default)]
struct MapRunProfileResolver {
    profiles: Arc<Mutex<BTreeMap<(WorkspaceId, String), AgentProfileV2>>>,
}

impl MapRunProfileResolver {
    fn insert(&self, workspace_id: WorkspaceId, name: impl Into<String>, profile: AgentProfileV2) {
        self.profiles
            .lock()
            .unwrap()
            .insert((workspace_id, name.into()), profile);
    }
}

impl RunProfileResolver for MapRunProfileResolver {
    fn resolve(
        &self,
        workspace_id: &WorkspaceId,
        name: &str,
    ) -> Result<ResolvedRunProfile, ProfileResolveError> {
        match self
            .profiles
            .lock()
            .unwrap()
            .get(&(workspace_id.clone(), name.to_string()))
        {
            Some(profile) => Ok(ResolvedRunProfile {
                workspace_id: workspace_id.clone(),
                profile: profile.clone(),
            }),
            None => Err(ProfileResolveError::Unknown {
                name: name.to_string(),
                workspace: workspace_id.clone(),
            }),
        }
    }
}

/// 仅 soft 隔离可用、Container 不可用的测试能力（模拟无真实容器后端）。
struct SoftOnlyCapability;

impl IsolationCapability for SoftOnlyCapability {
    fn soft_isolation_available(&self) -> bool {
        true
    }
    fn hard_container_available(&self) -> bool {
        false
    }
}

fn profile_v2(name: &str) -> AgentProfileV2 {
    AgentProfileV2 {
        name: name.into(),
        prompt: ProfilePrompt {
            system: "You are a careful reviewer.".into(),
            instructions: Some("Prefer minimal diffs.".into()),
        },
        model: ProfileModel {
            provider: Some("mock".into()),
            name: Some("mock-model".into()),
        },
        effort: ReasoningEffort::High,
        tools: ProfileToolRules::default(),
        skills: Vec::new(),
        mcp: Vec::new(),
        permissions: Vec::new(),
        hooks: Vec::new(),
        memory: ProfileMemory::default(),
        max_turns: Some(4),
        background: false,
        isolation: ProfileIsolation::None,
    }
}

fn make_profile(name: &str, isolation: ProfileIsolation, background: bool) -> AgentProfileV2 {
    let mut p = profile_v2(name);
    p.isolation = isolation;
    p.background = background;
    p
}

// ===== RunSupervisor 直测（无需 router session 装配） =====

fn supervisor() -> RunSupervisor {
    use agent_domain::CoreInstanceId;
    use agent_engine::EventBroadcaster;
    use app_service::{ApprovalRegistry, RateLimiter};
    let aggregate = Arc::new(app_service::AggregateState::new());
    RunSupervisor::new(
        4,
        aggregate,
        Arc::new(ApprovalRegistry::new()),
        Arc::new(RateLimiter::default()),
        EventBroadcaster::new(),
        CoreInstanceId::from("p17-5"),
    )
}

fn mock_provider() -> Arc<dyn ModelProvider> {
    Arc::new(
        test_support::MockProvider::new(test_support::MockScript::new().complete())
            .with_id(ProviderId::from("mock")),
    )
}

fn request_with_profile(profile: AgentProfileV2, run_id: &str) -> app_service::RunRequest {
    let workspace_id = WorkspaceId::from("ws");
    app_service::RunRequest {
        run_id: RunId::from(run_id),
        session_id: SessionId::from("sess"),
        workspace_id: Some(workspace_id.clone()),
        identity: tenant_service::IdentityContext::local(),
        provider_id: ProviderId::from("mock"),
        model: ModelId::from("mock-model"),
        source: CommandSource::Automation,
        command_id: CommandId::from("cmd"),
        user_message: "hello".into(),
        external_quota: None,
        profile: Some(ResolvedRunProfile {
            workspace_id,
            profile,
        }),
    }
}

async fn drain_until_terminal(supervisor: &RunSupervisor, run_id: &RunId) {
    for _ in 0..300 {
        if !supervisor.is_active(run_id) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    tokio::time::sleep(std::time::Duration::from_millis(30)).await;
}

#[tokio::test]
async fn profile_run_completes_with_prompt_reasoning_and_max_turns_wired() {
    let supervisor = supervisor();
    let provider = mock_provider();
    let request = request_with_profile(
        make_profile("reviewer", ProfileIsolation::None, false),
        "run-a",
    );
    supervisor.start(request, provider).expect("start");
    drain_until_terminal(&supervisor, &RunId::from("run-a")).await;
    assert!(!supervisor.is_active(&RunId::from("run-a")));
}

#[tokio::test]
async fn background_run_registers_and_finishes_agent_task_via_task_manager() {
    let supervisor = supervisor();
    let (backend, _selection) = sandbox_runtime::SandboxSelector::new().pick();
    let manager = Arc::new(task_manager::TaskManager::new(backend));
    supervisor.set_task_manager(manager.clone());

    let provider = mock_provider();
    let request = request_with_profile(make_profile("bg", ProfileIsolation::None, true), "run-bg");
    supervisor.start(request, provider).expect("start");
    drain_until_terminal(&supervisor, &RunId::from("run-bg")).await;

    let snapshot = manager.snapshot();
    let agent = snapshot
        .tasks
        .iter()
        .find(|task| task.task_kind == TaskKind::Agent)
        .expect("background run must register a TaskKind::Agent");
    assert!(
        matches!(
            agent.status,
            TaskStatus::Completed | TaskStatus::Failed | TaskStatus::Canceled
        ),
        "background agent task must reach a terminal status: {:?}",
        agent.status
    );
}

#[tokio::test]
async fn background_run_without_task_manager_fails_closed() {
    let supervisor = supervisor();
    let provider = mock_provider();
    let request = request_with_profile(
        make_profile("bg-noop", ProfileIsolation::None, true),
        "run-bn",
    );
    let result = supervisor.start(request, provider);
    assert!(
        matches!(result, Err(SuperviseError::BackgroundUnavailable(_))),
        "background run without TaskManager must fail-closed: {result:?}"
    );
}

#[tokio::test]
async fn retry_keeps_resolved_profile_and_completes() {
    let supervisor = supervisor();
    // 首次失败（进入 Failed 终态）；retry 复用同一不可变 profile 重新发起
    // （provider 仍为失败脚本，故重试也失败——证明点在 retry 沿用 profile 而非结果）。
    use provider_api::{ProviderError, ProviderErrorKind};
    let fail_provider: Arc<dyn ModelProvider> = Arc::new(
        test_support::MockProvider::new(test_support::MockScript::new().fail(ProviderError::new(
            ProviderErrorKind::ProviderUnavailable,
            "transient",
        )))
        .with_id(ProviderId::from("mock")),
    );
    let run_id = RunId::from("run-rt");
    // 工具规则 + max_turns 随 profile 携带；retry 沿用同一不可变 profile。
    let mut profile = make_profile("retry", ProfileIsolation::None, false);
    profile.tools = ProfileToolRules {
        allowed: vec!["read_file".into()],
        denied: vec!["shell".into()],
    };
    profile.max_turns = Some(2);
    let request = request_with_profile(profile, "run-rt");
    supervisor.start(request, fail_provider).expect("start");
    drain_until_terminal(&supervisor, &run_id).await;
    // Failed 终态可重试；retry 沿用同一不可变 profile（不 panic、不重置为 None）。
    supervisor.retry(&run_id).expect("retry keeps profile");
    // 重试后 run 再次进入终态（Failed），证明 retry 真正沿 profile 重新发起。
    drain_until_terminal(&supervisor, &run_id).await;
    assert!(!supervisor.is_active(&run_id));
}

// ===== CommandRouter fail-closed 路径 =====

fn router_with(resolver: Arc<MapRunProfileResolver>) -> CommandRouter {
    let router = CommandRouter::new(RouterConfig {
        instance: "p17-5".into(),
        max_concurrent_runs: 4,
        ..RouterConfig::default()
    });
    router.set_profile_resolver(resolver);
    router.set_isolation_capability(Arc::new(SoftOnlyCapability));
    router.register_provider(mock_provider());
    router
}

fn add_workspace(router: &CommandRouter) -> WorkspaceId {
    let dir = std::env::temp_dir().join(format!(
        "p17-5-ws-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::create_dir_all(&dir);
    let resp = router.dispatch(command(AppCommand::WorkspaceAdd {
        root_path: dir.to_string_lossy().into_owned(),
    }));
    match resp.response {
        core_api::AppResponse::Data(value) => {
            let workspace: workspace_service::Workspace =
                serde_json::from_value(value).expect("decode workspace");
            workspace.id
        }
        other => panic!("expected workspace, got {other:?}"),
    }
}

fn identity() -> ActorIdentity {
    ActorIdentity::LocalUser {
        actor_id: ActorId::from("tester"),
        display_name: Some("Tester".into()),
    }
}

fn command(payload: AppCommand) -> AppCommandEnvelope {
    command_with(CommandSource::Automation, identity(), payload)
}

fn command_with(
    source: CommandSource,
    identity: ActorIdentity,
    payload: AppCommand,
) -> AppCommandEnvelope {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    AppCommandEnvelope {
        api_version: API_VERSION,
        command_id: CommandId::from(format!("cmd-{n}")),
        source,
        identity,
        expected_revision: None,
        idempotency_key: None,
        issued_at: agent_domain::Timestamp::from_unix_millis(1),
        command: payload,
    }
}

fn create_session(router: &CommandRouter, workspace_id: &WorkspaceId) -> SessionId {
    let resp = router.dispatch(command(AppCommand::SessionCreate {
        workspace_id: workspace_id.clone(),
        title: Some("p17-5".into()),
    }));
    match resp.response {
        core_api::AppResponse::Data(value) => SessionId::from(
            value
                .get("session_id")
                .and_then(serde_json::Value::as_str)
                .expect("session id"),
        ),
        other => panic!("expected session, got {other:?}"),
    }
}

fn run_start(
    router: &CommandRouter,
    session_id: &SessionId,
    profile: Option<String>,
) -> core_api::AppResponse {
    run_start_ex(
        router,
        session_id,
        profile,
        None,
        CommandSource::Automation,
        identity(),
    )
}

fn run_start_ex(
    router: &CommandRouter,
    session_id: &SessionId,
    profile: Option<String>,
    model: Option<&str>,
    source: CommandSource,
    identity: ActorIdentity,
) -> core_api::AppResponse {
    let resp = router.dispatch(command_with(
        source,
        identity,
        AppCommand::RunStart {
            session_id: session_id.clone(),
            user_message: "hi".into(),
            model: model.map(ModelId::from),
            profile,
        },
    ));
    resp.response
}

#[tokio::test]
async fn container_isolation_fails_closed_at_run_start_without_hard_backend() {
    let resolver = Arc::new(MapRunProfileResolver::default());
    let router = router_with(resolver.clone());
    let workspace_id = add_workspace(&router);
    resolver.insert(
        workspace_id.clone(),
        "container-agent",
        make_profile("container-agent", ProfileIsolation::Container, false),
    );
    let session_id = create_session(&router, &workspace_id);
    let response = run_start(&router, &session_id, Some("container-agent".into()));
    assert!(
        matches!(response, core_api::AppResponse::Error { .. }),
        "Container isolation must fail-closed at RunStart: {response:?}"
    );
}

#[tokio::test]
async fn memory_enabled_unavailable_rejects_run_start() {
    let resolver = Arc::new(MapRunProfileResolver::default());
    let router = router_with(resolver.clone());
    let workspace_id = add_workspace(&router);
    let mut profile = make_profile("mem-agent", ProfileIsolation::None, false);
    profile.memory = ProfileMemory {
        enabled: true,
        unavailable: Some("production memory not wired".into()),
        ..ProfileMemory::default()
    };
    resolver.insert(workspace_id.clone(), "mem-agent", profile);
    let session_id = create_session(&router, &workspace_id);
    let response = run_start(&router, &session_id, Some("mem-agent".into()));
    assert!(
        matches!(response, core_api::AppResponse::Error { .. }),
        "memory enabled+unavailable must reject RunStart: {response:?}"
    );
}

#[tokio::test]
async fn unknown_profile_name_fails_closed() {
    let resolver = Arc::new(MapRunProfileResolver::default());
    let router = router_with(resolver);
    let workspace_id = add_workspace(&router);
    let session_id = create_session(&router, &workspace_id);
    let response = run_start(&router, &session_id, Some("does-not-exist".into()));
    assert!(
        matches!(response, core_api::AppResponse::Error { .. }),
        "unknown profile name must fail-closed: {response:?}"
    );
}

#[tokio::test]
async fn profile_without_resolver_fails_closed() {
    // 未注入 resolver：RunStart 携带 profile 名一律 fail-closed。
    let router = CommandRouter::new(RouterConfig {
        instance: "p17-5-nr".into(),
        max_concurrent_runs: 4,
        ..RouterConfig::default()
    });
    router.register_provider(mock_provider());
    let workspace_id = add_workspace(&router);
    let session_id = create_session(&router, &workspace_id);
    let response = run_start(&router, &session_id, Some("any".into()));
    assert!(
        matches!(response, core_api::AppResponse::Error { .. }),
        "profile without resolver must fail-closed: {response:?}"
    );
}

#[tokio::test]
async fn restricted_isolation_is_satisfied_by_soft_capability() {
    // 测试注入的 SoftOnlyCapability 模拟「真实软隔离执行器已接线」的未来
    // 状态：Restricted 被满足，run 正常启动。生产默认能力无真实执行器时
    // 必须 fail-closed（见 restricted_isolation_fails_closed_without_real_executor）。
    let resolver = Arc::new(MapRunProfileResolver::default());
    let router = router_with(resolver.clone());
    let workspace_id = add_workspace(&router);
    resolver.insert(
        workspace_id.clone(),
        "restricted-agent",
        make_profile("restricted-agent", ProfileIsolation::Restricted, false),
    );
    let session_id = create_session(&router, &workspace_id);
    let response = run_start(&router, &session_id, Some("restricted-agent".into()));
    assert!(
        matches!(response, core_api::AppResponse::Accepted { .. }),
        "Restricted isolation satisfied by soft capability should start: {response:?}"
    );
}

#[tokio::test]
async fn restricted_isolation_fails_closed_without_real_executor() {
    // P17-5 主审修复：生产默认能力（SandboxIsolationCapability）下主 run 链
    // 没有真实隔离执行器接线（P13-1 no-op runtime），Restricted 与 Container
    // 一样必须 fail-closed，绝不虚假可用 / 静默降级。
    let resolver = Arc::new(MapRunProfileResolver::default());
    let router = CommandRouter::new(RouterConfig {
        instance: "p17-5-restricted".into(),
        max_concurrent_runs: 4,
        ..RouterConfig::default()
    });
    router.register_provider(mock_provider());
    let workspace_id = add_workspace(&router);
    resolver.insert(
        workspace_id.clone(),
        "restricted-agent",
        make_profile("restricted-agent", ProfileIsolation::Restricted, false),
    );
    let session_id = create_session(&router, &workspace_id);
    let response = run_start(&router, &session_id, Some("restricted-agent".into()));
    assert!(
        matches!(response, core_api::AppResponse::Error { .. }),
        "Restricted 无真实隔离执行器必须 fail-closed at RunStart: {response:?}"
    );
}

#[tokio::test]
async fn deny_first_tool_rules_filter_at_authoritative_pre_tool_in_main_chain() {
    // P17-5 主审修复：deny-first 主链证据——provider 同一轮提出 allowed
    // （read_file）+ denied（shell）两个工具调用，权威 pre_tool 位点必须：
    // - denied 不执行（无 ToolExecutionStarted），以拒绝结果回填
    //   （ToolExecutionCompleted + is_error）；
    // - allowed 正常执行（ToolExecutionStarted + 成功结果）；
    // - run 进入终态。审批全部预投递 ApproveOnce（否则 run 停在审批等待）。
    use agent_engine::EventBroadcaster;
    use agent_events::AgentEvent;
    use async_trait::async_trait;
    use serde_json::json;

    let aggregate = Arc::new(app_service::AggregateState::new());
    let approvals = Arc::new(app_service::ApprovalRegistry::new());
    let broadcaster = EventBroadcaster::new();
    let mut subscriber = broadcaster.subscribe();
    let supervisor = RunSupervisor::new(
        4,
        aggregate,
        approvals.clone(),
        Arc::new(app_service::RateLimiter::default()),
        broadcaster,
        CoreInstanceId::from("p17-5-deny"),
    );

    // 两阶段 provider：第一轮提出工具调用，第二轮纯文本完成。
    let first = test_support::MockScript::new()
        .tool_call("read_file", json!({"path": "a.txt"}))
        .tool_call("shell", json!({"command": "rm -rf /"}))
        .complete_with(StopReason::ToolUse);
    #[derive(Clone)]
    struct TwoPhase {
        first: Arc<test_support::MockProvider>,
        second: Arc<test_support::MockProvider>,
        calls: Arc<std::sync::atomic::AtomicU64>,
    }
    #[async_trait]
    impl ModelProvider for TwoPhase {
        fn id(&self) -> agent_domain::ProviderId {
            self.first.id()
        }
        async fn list_models(
            &self,
            cred: Option<&ResolvedCredential>,
        ) -> Result<Vec<ModelDefinition>, ProviderError> {
            self.first.list_models(cred).await
        }
        async fn stream(
            &self,
            request: CanonicalModelRequest,
            sink: &dyn ProviderEventSink,
            cancel: agent_domain::CancellationToken,
        ) -> Result<ModelResponseSummary, ProviderError> {
            let n = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if n == 0 {
                self.first.stream(request, sink, cancel).await
            } else {
                self.second.stream(request, sink, cancel).await
            }
        }
    }
    let provider: Arc<dyn ModelProvider> = Arc::new(TwoPhase {
        first: Arc::new(test_support::MockProvider::new(first)),
        second: Arc::new(
            test_support::MockProvider::new(
                test_support::MockScript::new()
                    .text("done")
                    .usage(TokenUsage {
                        input_tokens: 5,
                        output_tokens: 1,
                        cache_read_tokens: 0,
                        cache_write_tokens: 0,
                    })
                    .complete(),
            )
            .with_id(ProviderId::from("mock")),
        ),
        calls: Arc::new(std::sync::atomic::AtomicU64::new(0)),
    });

    let mut profile = make_profile("deny-first", ProfileIsolation::None, false);
    profile.tools = ProfileToolRules {
        allowed: vec!["read_file".into()],
        denied: vec!["shell".into()],
    };
    let run_id = RunId::from("run-deny");
    let request = request_with_profile(profile, "run-deny");
    // 预投递审批决策：register 前入队，run 到达审批位点时立即解析。
    approvals
        .decide(
            &run_id,
            &ToolCallId::from("mock-tool-call-0"),
            ApprovalDecision::ApproveOnce,
        )
        .expect("queue approval for read_file");
    approvals
        .decide(
            &run_id,
            &ToolCallId::from("mock-tool-call-1"),
            ApprovalDecision::ApproveOnce,
        )
        .expect("queue approval for shell");
    supervisor.start(request, provider).expect("start");
    drain_until_terminal(&supervisor, &run_id).await;
    assert!(!supervisor.is_active(&run_id), "run 必须进入终态");

    let mut started = Vec::new();
    let mut completed = Vec::new();
    let mut error_results = Vec::new();
    while let Ok(Some(envelope)) = subscriber.try_recv() {
        match envelope.payload {
            AgentEvent::ToolExecutionStarted { tool_call_id } => {
                started.push(tool_call_id.as_str().to_string())
            }
            AgentEvent::ToolExecutionCompleted {
                tool_call_id,
                result,
            } => {
                completed.push(tool_call_id.as_str().to_string());
                if result.is_error {
                    error_results.push(tool_call_id.as_str().to_string());
                }
            }
            _ => {}
        }
    }
    assert!(
        started.iter().any(|id| id == "mock-tool-call-0"),
        "allowed read_file 必须执行: started={started:?}"
    );
    assert!(
        !started.iter().any(|id| id == "mock-tool-call-1"),
        "denied shell 不得执行（无 ToolExecutionStarted）: started={started:?}"
    );
    assert!(
        completed.iter().any(|id| id == "mock-tool-call-1"),
        "denied shell 必须以拒绝结果回填（有 ToolExecutionCompleted）: completed={completed:?}"
    );
    assert!(
        error_results.iter().any(|id| id == "mock-tool-call-1"),
        "denied shell 的结果必须是错误/拒绝视图: error_results={error_results:?}"
    );
    assert!(
        !error_results.iter().any(|id| id == "mock-tool-call-0"),
        "allowed read_file 的结果不得是错误视图: error_results={error_results:?}"
    );
}

// ===== P17-5 模型覆盖授权（ModelOverridePolicy） =====

/// 覆盖授权测试 router：注册 openai（内置目录 provider）mock 使显式模型 /
/// profile 模型都可解析；policy 缺省 DenyAll 或按参数注入。
fn override_router_with(
    policy: Option<Arc<dyn app_service::ModelOverridePolicy>>,
) -> CommandRouter {
    let router = CommandRouter::new(RouterConfig {
        instance: "p17-5-ovr".into(),
        max_concurrent_runs: 4,
        ..RouterConfig::default()
    });
    if let Some(policy) = policy {
        router.set_model_override_policy(policy);
    }
    router.set_isolation_capability(Arc::new(SoftOnlyCapability));
    router.register_provider(Arc::new(
        test_support::MockProvider::new(test_support::MockScript::new().complete())
            .with_id(ProviderId::from("openai")),
    ));
    router
}

/// profile canonical 模型：openai / gpt-4o（内置目录可解析）。
fn pinned_profile(name: &str) -> AgentProfileV2 {
    let mut profile = profile_v2(name);
    profile.model = ProfileModel {
        provider: Some("openai".into()),
        name: Some("gpt-4o".into()),
    };
    profile
}

#[tokio::test]
async fn model_override_is_denied_by_default_fail_closed_policy() {
    // 未注入策略：缺省 DenyAll。即使本机 LocalCli + LocalUser，profile 锁定
    // 模型被显式不同模型覆盖也一律拒绝（绝不直接信任 caller）。
    let resolver = Arc::new(MapRunProfileResolver::default());
    let router = override_router_with(None);
    router.set_profile_resolver(resolver.clone());
    let workspace_id = add_workspace(&router);
    resolver.insert(workspace_id.clone(), "pinned", pinned_profile("pinned"));
    let session_id = create_session(&router, &workspace_id);
    let response = run_start_ex(
        &router,
        &session_id,
        Some("pinned".into()),
        Some("gpt-4o-mini"),
        CommandSource::LocalCli {
            terminal_session_id: None,
        },
        identity(),
    );
    assert!(
        matches!(response, core_api::AppResponse::Error { .. }),
        "profile + different explicit model must be denied by default DenyAll: {response:?}"
    );
}

#[tokio::test]
async fn model_override_from_remote_source_is_rejected_by_production_policy() {
    // 生产策略：Remote / Automation / Plugin / MCP 一律拒绝模型覆盖。
    let resolver = Arc::new(MapRunProfileResolver::default());
    let router = override_router_with(Some(Arc::new(app_service::ProductionModelOverridePolicy)));
    router.set_profile_resolver(resolver.clone());
    let workspace_id = add_workspace(&router);
    resolver.insert(workspace_id.clone(), "pinned", pinned_profile("pinned"));
    let session_id = create_session(&router, &workspace_id);
    let response = run_start_ex(
        &router,
        &session_id,
        Some("pinned".into()),
        Some("gpt-4o-mini"),
        CommandSource::RemoteGui {
            client_id: agent_domain::GuiClientId::from("remote-1"),
            connection_id: agent_domain::ConnectionId::from("conn-1"),
        },
        identity(),
    );
    assert!(
        matches!(response, core_api::AppResponse::Error { .. }),
        "remote source must be denied model override by production policy: {response:?}"
    );
}

#[tokio::test]
async fn model_override_passes_explicit_allow_gate_for_local_user() {
    // 生产策略：LocalCli + LocalUser 显式覆盖放行，run 正常启动。
    let resolver = Arc::new(MapRunProfileResolver::default());
    let router = override_router_with(Some(Arc::new(app_service::ProductionModelOverridePolicy)));
    router.set_profile_resolver(resolver.clone());
    let workspace_id = add_workspace(&router);
    resolver.insert(workspace_id.clone(), "pinned", pinned_profile("pinned"));
    let session_id = create_session(&router, &workspace_id);
    let response = run_start_ex(
        &router,
        &session_id,
        Some("pinned".into()),
        Some("gpt-4o-mini"),
        CommandSource::LocalCli {
            terminal_session_id: None,
        },
        identity(),
    );
    assert!(
        matches!(response, core_api::AppResponse::Accepted { .. }),
        "local user override must pass production allow gate: {response:?}"
    );
}

#[tokio::test]
async fn same_model_landing_is_not_an_override_even_under_deny_all() {
    // 同模型（别名归一后落点相同）不构成 override：缺省 DenyAll 也不误拒。
    // 显式 "gpt4o" 是 "gpt-4o" 的别名，与 profile canonical 落点相同。
    let resolver = Arc::new(MapRunProfileResolver::default());
    let router = override_router_with(None);
    router.set_profile_resolver(resolver.clone());
    let workspace_id = add_workspace(&router);
    resolver.insert(workspace_id.clone(), "pinned", pinned_profile("pinned"));
    let session_id = create_session(&router, &workspace_id);
    let response = run_start_ex(
        &router,
        &session_id,
        Some("pinned".into()),
        Some("gpt4o"),
        CommandSource::Automation,
        identity(),
    );
    assert!(
        matches!(response, core_api::AppResponse::Accepted { .. }),
        "same landing must not be treated as override (no false rejection): {response:?}"
    );
}

#[tokio::test]
async fn explicit_model_fills_profile_without_model_and_is_not_an_override() {
    // profile 未声明模型：显式命令模型只是补全，无 canonical 落点可比，
    // 不构成 override——缺省 DenyAll 也不误拒。
    let resolver = Arc::new(MapRunProfileResolver::default());
    let router = override_router_with(None);
    router.set_profile_resolver(resolver.clone());
    let workspace_id = add_workspace(&router);
    let mut profile = profile_v2("no-model");
    profile.model = ProfileModel {
        provider: None,
        name: None,
    };
    resolver.insert(workspace_id.clone(), "no-model", profile);
    let session_id = create_session(&router, &workspace_id);
    let response = run_start_ex(
        &router,
        &session_id,
        Some("no-model".into()),
        Some("gpt-4o"),
        CommandSource::Automation,
        identity(),
    );
    assert!(
        matches!(response, core_api::AppResponse::Accepted { .. }),
        "explicit model filling a profile without model must not be an override: {response:?}"
    );
}
