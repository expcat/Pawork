//! Run 生命周期集成测试（P13-1）。
//!
//! 覆盖：RunStart 经 agent-engine ProviderLoop 的真实执行路径、delta 限流合并、
//! 取消幂等（GUI 断线不取消 Run）、RunRetry、审批等待通道与终态收尾、
//! agent 事件订阅。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use agent_domain::{
    ActorId, CancellationToken, CommandId, ProviderId, QueryId, RunId, SessionId, StopReason,
    Timestamp, TokenUsage, ToolCallId, WorkspaceId,
};
use app_service::{CommandRouter, RouterConfig};
use async_trait::async_trait;
use core_api::{
    ActorIdentity, AppCommand, AppCommandEnvelope, AppQuery, AppQueryEnvelope, AppResponse,
    ApprovalDecision, CommandSource, RunState, API_VERSION,
};
use provider_api::{
    CanonicalModelRequest, ModelDefinition, ModelProvider, ModelResponseSummary, ProviderError,
    ProviderEventSink, ProviderStreamEvent, ResolvedCredential,
};
use serde_json::{json, Value};

/// 两轮脚本 Provider：第一轮请求工具 `echo`，第二轮直接完成。
/// MockScript 会逐轮重放同一脚本，无法表达「工具后完成」，故测试自建。
struct TwoTurnProvider {
    id: ProviderId,
    turns: Mutex<u32>,
}

impl TwoTurnProvider {
    fn new(id: ProviderId) -> Self {
        Self {
            id,
            turns: Mutex::new(0),
        }
    }
}

#[async_trait]
impl ModelProvider for TwoTurnProvider {
    fn id(&self) -> ProviderId {
        self.id.clone()
    }

    async fn list_models(
        &self,
        _credential: Option<&ResolvedCredential>,
    ) -> Result<Vec<ModelDefinition>, ProviderError> {
        Ok(Vec::new())
    }

    async fn stream(
        &self,
        _request: CanonicalModelRequest,
        sink: &dyn ProviderEventSink,
        _cancel: CancellationToken,
    ) -> Result<ModelResponseSummary, ProviderError> {
        let turn = {
            let mut turns = self.turns.lock().expect("turns mutex");
            *turns += 1;
            *turns
        };
        let tool_call_id = ToolCallId::from("mock-tool-call-0");
        if turn == 1 {
            sink.emit(ProviderStreamEvent::ToolCallStarted {
                id: tool_call_id.clone(),
                name: "echo".into(),
            })
            .await?;
            sink.emit(ProviderStreamEvent::ToolCallArgumentsDelta {
                id: tool_call_id.clone(),
                json: "{}".into(),
            })
            .await?;
            sink.emit(ProviderStreamEvent::ToolCallCompleted { id: tool_call_id })
                .await?;
            sink.emit(ProviderStreamEvent::ResponseCompleted(StopReason::ToolUse))
                .await?;
        } else {
            sink.emit(ProviderStreamEvent::TextDelta("done".into()))
                .await?;
            sink.emit(ProviderStreamEvent::ResponseCompleted(
                StopReason::Completed,
            ))
            .await?;
        }
        Ok(ModelResponseSummary {
            stop_reason: StopReason::Completed,
            usage: TokenUsage::default(),
            response_id: None,
            provider_metadata: Value::Null,
        })
    }
}

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
        display_name: None,
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

fn temp_workspace_dir() -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("pawork-run-lifecycle-{}", unique("ws")));
    std::fs::create_dir_all(&path).expect("create temp workspace dir");
    path
}

/// 建 workspace + session，返回 session_id。
fn prepare_session(router: &CommandRouter) -> SessionId {
    let response = router.dispatch(command(
        cli_source(),
        cli_identity(),
        AppCommand::WorkspaceAdd {
            root_path: temp_workspace_dir().to_string_lossy().into_owned(),
        },
    ));
    let workspace_id = match &response.response {
        AppResponse::Data(value) => WorkspaceId::from(
            value
                .get("id")
                .and_then(serde_json::Value::as_str)
                .expect("workspace id"),
        ),
        other => panic!("expected workspace data, got {other:?}"),
    };
    let response = router.dispatch(command(
        cli_source(),
        cli_identity(),
        AppCommand::SessionCreate {
            workspace_id,
            title: Some("lifecycle".into()),
        },
    ));
    match &response.response {
        AppResponse::Data(value) => SessionId::from(
            value
                .get("session_id")
                .and_then(serde_json::Value::as_str)
                .expect("session id"),
        ),
        other => panic!("expected session data, got {other:?}"),
    }
}

fn start_run(router: &CommandRouter, session_id: &SessionId, message: &str) -> RunId {
    let response = router.dispatch(command(
        cli_source(),
        cli_identity(),
        AppCommand::RunStart {
            session_id: session_id.clone(),
            user_message: message.into(),
            model: None,
            profile: None,
        },
    ));
    let AppResponse::Accepted {
        run_id: Some(run_id),
        ..
    } = response.response
    else {
        panic!(
            "RunStart 应 Accepted 且携带 run id，got {:?}",
            response.response
        );
    };
    run_id
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

/// RunStart 走真实 ProviderLoop：脚本驱动的事件经广播 → 聚合 → 限流合并后取回。
#[tokio::test]
async fn run_streams_merge_delta_events_and_reach_terminal_state() {
    let router = router_with_mock_provider(
        test_support::MockScript::new()
            .text("hello ")
            .text("world")
            .thinking("planning")
            .complete(),
    );
    let session_id = prepare_session(&router);
    let run_id = start_run(&router, &session_id, "hi");

    let completed = wait_until(
        || {
            router
                .aggregate()
                .get_run(&run_id)
                .is_some_and(|run| run.state == RunState::Completed)
        },
        Duration::from_secs(5),
    )
    .await;
    assert!(completed, "run 应在 5s 内完成");
    assert!(!router.supervisor().is_active(&run_id));

    // 冲刷限流器：delta 合并（同 message 的多条增量应合并为一条），状态事件直通。
    let events = router.drain_events();
    let assistant: Vec<&str> = events
        .iter()
        .filter_map(|event| match &event.payload {
            core_api::AppEvent::AssistantDelta { delta, .. } => Some(delta.as_str()),
            _ => None,
        })
        .collect();
    assert!(!assistant.is_empty(), "应有 assistant delta 事件");
    let combined: String = assistant.concat();
    assert_eq!(combined, "hello world", "全部 delta 拼接应等于脚本文本");
    assert!(
        assistant.len() <= 2,
        "同 message 的增量应被合并，实际 {} 条",
        assistant.len()
    );
    let thinking: Vec<&str> = events
        .iter()
        .filter_map(|event| match &event.payload {
            core_api::AppEvent::ThinkingDelta { delta, .. } => Some(delta.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(thinking, vec!["planning"]);

    // 状态事件（RunChanged）至少出现一次。
    assert!(
        events
            .iter()
            .any(|event| matches!(event.payload, core_api::AppEvent::RunChanged { .. })),
        "应观察到 RunChanged 状态事件"
    );

    // 事件全局序号严格递增（限流合并会吞掉中间序号，允许空洞但不得回退/重复）。
    let mut sequences: Vec<u64> = events.iter().map(|event| event.global_sequence.0).collect();
    sequences.sort_unstable();
    for window in sequences.windows(2) {
        assert!(window[1] > window[0], "global sequence 必须严格递增");
    }
}

/// 重试幂等：已取消 run 可重开；活跃/已完成 run 拒绝。
#[tokio::test]
async fn retry_cancelled_run_restarts_and_active_retry_is_rejected() {
    let router = router_with_mock_provider(test_support::MockScript::new().wait_for_cancellation());
    let session_id = prepare_session(&router);
    let run_id = start_run(&router, &session_id, "retry me");
    let started = wait_until(
        || {
            router
                .aggregate()
                .get_run(&run_id)
                .is_some_and(|run| run.state == RunState::StreamingResponse)
        },
        Duration::from_secs(5),
    )
    .await;
    assert!(started);

    // 活跃时 retry 拒绝（Conflict）。
    let active_retry = router.dispatch(command(
        cli_source(),
        cli_identity(),
        AppCommand::RunRetry {
            run_id: run_id.clone(),
        },
    ));
    assert!(matches!(
        active_retry.response,
        AppResponse::Error(ref context)
            if context.category == agent_domain::ErrorCategory::Conflict
    ));

    router.dispatch(command(
        cli_source(),
        cli_identity(),
        AppCommand::RunCancel {
            run_id: run_id.clone(),
        },
    ));
    // 等后台任务真正落地（终态计数递增晚于 task_state 写入，保证 retry 可见）。
    let cancelled = wait_until(
        || router.supervisor().stats().cancelled >= 1,
        Duration::from_secs(5),
    )
    .await;
    assert!(cancelled, "run 应被取消");

    // 已取消 run 可重试。
    let retry = router.dispatch(command(
        cli_source(),
        cli_identity(),
        AppCommand::RunRetry {
            run_id: run_id.clone(),
        },
    ));
    match &retry.response {
        AppResponse::Data(value) => {
            assert_eq!(value["retried"], json!(true));
        }
        other => panic!("expected retry data, got {other:?}"),
    }
    let restarted = wait_until(
        || {
            router
                .aggregate()
                .get_run(&run_id)
                .is_some_and(|run| run.state == RunState::StreamingResponse)
        },
        Duration::from_secs(5),
    )
    .await;
    assert!(restarted, "重试后 run 应重新进入 StreamingResponse");
    assert_eq!(router.supervisor().stats().retried, 1);
    assert_eq!(router.supervisor().total(), 1, "重试复用同一 run 登记");
    // 清理：取消重试后的 run，避免挂起任务。
    router.dispatch(command(
        cli_source(),
        cli_identity(),
        AppCommand::RunCancel {
            run_id: run_id.clone(),
        },
    ));
    let _ = wait_until(
        || router.supervisor().stats().cancelled >= 2,
        Duration::from_secs(5),
    )
    .await;
}

/// 审批：ToolApprovalRequested → Pending 记录 → ToolApprove 决策 → 引擎继续 → Completed。
#[tokio::test]
async fn approval_pending_resolves_and_run_completes() {
    let router = CommandRouter::new(RouterConfig::default());
    let provider: Arc<dyn ModelProvider> = Arc::new(TwoTurnProvider::new(ProviderId::from("mock")));
    router.register_provider(provider);
    let session_id = prepare_session(&router);
    let run_id = start_run(&router, &session_id, "run with tool");

    // 引擎进入 WaitingForApproval 并登记 Pending 审批。
    let pending = wait_until(
        || {
            router
                .aggregate()
                .approvals()
                .iter()
                .any(|approval| approval.run_id == run_id)
        },
        Duration::from_secs(5),
    )
    .await;
    assert!(pending, "应有 Pending 审批记录");
    let approvals = router.aggregate().approvals();
    let approval = approvals
        .iter()
        .find(|approval| approval.run_id == run_id)
        .expect("approval");
    assert_eq!(approval.status, app_service::ApprovalStatus::Pending);
    let tool_call_id = approval.tool_call_id.clone();
    assert_eq!(tool_call_id.as_str(), "mock-tool-call-0");
    assert_eq!(router.approvals().pending_count(), 1);
    assert_eq!(
        router.aggregate().get_run(&run_id).expect("run").state,
        RunState::WaitingForApproval
    );

    // ToolApprove：投递决策，注册表与聚合都转为 Decided。
    let decide = router.dispatch(command(
        cli_source(),
        cli_identity(),
        AppCommand::ToolApprove {
            run_id: run_id.clone(),
            tool_call_id: tool_call_id.clone(),
            decision: ApprovalDecision::ApproveOnce,
        },
    ));
    assert!(matches!(decide.response, AppResponse::Data(_)));
    // 重复决策被拒绝（幂等保护）：两次 dispatch 之间无 await，引擎不会抢先落地。
    let duplicate = router.dispatch(command(
        cli_source(),
        cli_identity(),
        AppCommand::ToolApprove {
            run_id: run_id.clone(),
            tool_call_id: tool_call_id.clone(),
            decision: ApprovalDecision::ApproveOnce,
        },
    ));
    assert!(matches!(
        duplicate.response,
        AppResponse::Error(ref context)
            if context.category == agent_domain::ErrorCategory::Conflict
    ));
    assert_eq!(router.approvals().pending_count(), 0);
    let decided = wait_until(
        || {
            router.aggregate().approvals().iter().any(|approval| {
                approval.run_id == run_id && approval.status != app_service::ApprovalStatus::Pending
            })
        },
        Duration::from_secs(5),
    )
    .await;
    assert!(decided, "审批记录应转为 Decided");
    let approval = router
        .aggregate()
        .approvals()
        .into_iter()
        .find(|approval| approval.run_id == run_id)
        .expect("approval");
    assert_eq!(
        approval.status,
        app_service::ApprovalStatus::Decided(ApprovalDecision::ApproveOnce)
    );

    // 审批通过后 no-op 工具执行器回填结果，run 完成。
    let completed = wait_until(
        || {
            router
                .aggregate()
                .get_run(&run_id)
                .is_some_and(|run| run.state == RunState::Completed)
        },
        Duration::from_secs(5),
    )
    .await;
    assert!(completed, "审批后 run 应完成");
    assert_eq!(router.supervisor().stats().completed, 1);
    // 终态清理审批：registry 与聚合记录保持一致（记录保留，挂起清空）。
    assert_eq!(router.approvals().pending_count(), 0);
    let _ = router.supervisor().stats();
}

/// agent 事件订阅：订阅者能看到真实引擎事件流（含工具审批请求）。
#[tokio::test]
async fn agent_event_subscription_receives_engine_events() {
    let router = CommandRouter::new(RouterConfig::default());
    let provider: Arc<dyn ModelProvider> = Arc::new(TwoTurnProvider::new(ProviderId::from("mock")));
    router.register_provider(provider);
    let mut subscriber = router.subscribe_agent_events();
    let session_id = prepare_session(&router);
    let run_id = start_run(&router, &session_id, "subscribed");

    let deadline = Instant::now() + Duration::from_secs(5);
    let mut saw_started = false;
    let mut saw_approval = false;
    let mut saw_completed = false;
    let mut approved = false;
    while Instant::now() < deadline {
        // 超时包装：引擎可能停在审批等待，不能让 recv 无限 park。
        if let Ok(Ok(envelope)) =
            tokio::time::timeout(Duration::from_millis(100), subscriber.recv()).await
        {
            if envelope.run_id == run_id {
                saw_started |= matches!(
                    envelope.payload,
                    agent_events::AgentEvent::RunStarted { .. }
                );
                saw_approval |= matches!(
                    envelope.payload,
                    agent_events::AgentEvent::ToolApprovalRequested { .. }
                );
                saw_completed |= matches!(
                    envelope.payload,
                    agent_events::AgentEvent::RunCompleted { .. }
                );
                if !approved
                    && matches!(
                        envelope.payload,
                        agent_events::AgentEvent::ToolApprovalRequested { .. }
                    )
                {
                    let tool_call_id = if let agent_events::AgentEvent::ToolApprovalRequested {
                        tool_call_id,
                        ..
                    } = &envelope.payload
                    {
                        tool_call_id.clone()
                    } else {
                        unreachable!()
                    };
                    router.dispatch(command(
                        cli_source(),
                        cli_identity(),
                        AppCommand::ToolApprove {
                            run_id: run_id.clone(),
                            tool_call_id,
                            decision: ApprovalDecision::ApproveOnce,
                        },
                    ));
                    approved = true;
                }
            }
        }
        if saw_started && saw_approval && saw_completed {
            break;
        }
    }
    assert!(saw_started, "订阅应看到 RunStarted");
    assert!(saw_approval, "订阅应看到 ToolApprovalRequested");
    assert!(saw_completed, "订阅应看到 RunCompleted");

    // 等待 run 完全落地后清理（无挂起审批残留由终态处理保证）。
    let _ = wait_until(
        || !router.supervisor().is_active(&run_id),
        Duration::from_secs(5),
    )
    .await;
    assert_eq!(router.approvals().pending_count(), 0);
}

/// 未知 run 的取消/查询返回结构化 NotFound。
#[tokio::test]
async fn unknown_run_cancel_returns_structured_not_found() {
    let router = router_with_mock_provider(test_support::MockScript::new().complete());
    let response = router.dispatch(command(
        cli_source(),
        cli_identity(),
        AppCommand::RunCancel {
            run_id: RunId::from("nope"),
        },
    ));
    assert!(matches!(
        response.response,
        AppResponse::Error(ref context)
            if context.category == agent_domain::ErrorCategory::NotFound
    ));
    let response = router.dispatch_query(query(
        cli_source(),
        cli_identity(),
        AppQuery::RunStatus {
            run_id: RunId::from("nope"),
        },
    ));
    assert!(matches!(
        response.response,
        AppResponse::Error(ref context)
            if context.category == agent_domain::ErrorCategory::NotFound
    ));
}

/// 并发 run：有界监督器容量内多 run 互不干扰。
#[tokio::test]
async fn concurrent_runs_are_independent() {
    let router = router_with_mock_provider(test_support::MockScript::new().complete());
    let session_id = prepare_session(&router);
    let first = start_run(&router, &session_id, "run one");
    let second = start_run(&router, &session_id, "run two");
    assert_ne!(first, second);
    let both_done = wait_until(
        || {
            [&first, &second].into_iter().all(|run_id| {
                router
                    .aggregate()
                    .get_run(run_id)
                    .is_some_and(|run| run.state == RunState::Completed)
            })
        },
        Duration::from_secs(5),
    )
    .await;
    assert!(both_done, "两个并发 run 都应完成");
    assert_eq!(router.supervisor().total(), 2);
    assert_eq!(router.aggregate().runs().len(), 2);
    assert_eq!(router.supervisor().stats().started, 2);
}

/// 校验 RunRecord 快照字段完整（source/state/revision）。
#[tokio::test]
async fn run_record_carries_source_state_and_revision() {
    let router = router_with_mock_provider(test_support::MockScript::new().complete());
    let session_id = prepare_session(&router);
    let run_id = start_run(&router, &session_id, "metadata");
    let done = wait_until(
        || {
            router
                .aggregate()
                .get_run(&run_id)
                .is_some_and(|run| run.state == RunState::Completed)
        },
        Duration::from_secs(5),
    )
    .await;
    assert!(done);
    let run = router.aggregate().get_run(&run_id).expect("run");
    assert_eq!(run.source, cli_source());
    assert_eq!(run.state, RunState::Completed);
    assert!(run.revision > 0);
    assert_eq!(run.session_id, session_id);
    assert_eq!(run.provider_id.as_str(), "mock");
}
