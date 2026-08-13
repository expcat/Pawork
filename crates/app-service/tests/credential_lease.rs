//! P18-4 生产接线集成测试：run attempt 经 CredentialPool acquire → 持有
//! LeaseGuard 至终态 → 释放；usage 归属来自真实 CredentialLease；acquire
//! 失败 fail-closed；retry 重新 acquire。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use agent_domain::{
    ActorId, AgentId, CommandId, ProviderId, RunId, SessionId, Timestamp, TokenUsage, WorkspaceId,
};
use app_service::{CommandRouter, QuotaRuntime, RouterConfig};
use core_api::{
    ActorIdentity, AppCommand, AppCommandEnvelope, AppResponse, CommandSource, RunState,
    API_VERSION,
};
use provider_control::{AccountId, CredentialPool, InMemoryCredentialPool};

static NEXT_ID: AtomicU64 = AtomicU64::new(0);

fn unique(prefix: &str) -> String {
    format!("{prefix}-{}", NEXT_ID.fetch_add(1, Ordering::SeqCst))
}

fn cli_source() -> CommandSource {
    CommandSource::LocalCli {
        terminal_session_id: None,
    }
}

fn cli_identity() -> ActorIdentity {
    ActorIdentity::LocalUser {
        actor_id: ActorId::from("tester"),
        display_name: None,
    }
}

/// P18-2：本地 CLI 身份解析出的真实租户（local/default）。
fn local_tenant() -> agent_domain::TenantId {
    tenant_service::IdentityContext::local().tenant_id
}

fn command(command: AppCommand) -> AppCommandEnvelope {
    AppCommandEnvelope {
        api_version: API_VERSION,
        command_id: CommandId::from(unique("cmd")),
        source: cli_source(),
        identity: cli_identity(),
        expected_revision: None,
        idempotency_key: None,
        issued_at: Timestamp::from_unix_millis(1),
        command,
    }
}

fn temp_dir() -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("pawork-credential-lease-{}", unique("ws")));
    std::fs::create_dir_all(&path).expect("temp dir");
    path
}

/// 带共享 Quota 运行时与池的 router：记账走同一 ledger，可断言 lease 归属。
fn router_with_pool_and_ledger(
    pool: Arc<InMemoryCredentialPool>,
) -> (CommandRouter, Arc<dyn usage_ledger::UsageLedger>) {
    let runtime = QuotaRuntime::production_in_memory();
    let ledger = runtime.ledger.clone();
    let router = CommandRouter::new(RouterConfig::default());
    router.set_credential_pool(pool);
    router.set_quota_runtime(Arc::clone(&runtime));
    let provider: Arc<dyn provider_api::ModelProvider> = Arc::new(
        test_support::MockProvider::new(
            test_support::MockScript::new()
                // 账本拒绝全 0 用量记录，必须产生非零 usage 才能持久化归属。
                .usage(TokenUsage {
                    input_tokens: 1,
                    ..Default::default()
                })
                .complete(),
        )
        .with_id(ProviderId::from("mock")),
    );
    router.register_provider(provider);
    (router, ledger)
}

fn prepare_session(router: &CommandRouter) -> SessionId {
    let response = router.dispatch(command(AppCommand::WorkspaceAdd {
        root_path: temp_dir().to_string_lossy().into_owned(),
    }));
    let workspace_id = match &response.response {
        AppResponse::Data(value) => WorkspaceId::from(
            value
                .get("id")
                .and_then(serde_json::Value::as_str)
                .expect("workspace id"),
        ),
        other => panic!("expected workspace data, got {other:?}"),
    };
    let response = router.dispatch(command(AppCommand::SessionCreate {
        workspace_id,
        title: Some("credential-lease".into()),
    }));
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

fn start_run(router: &CommandRouter, session_id: &SessionId) -> RunId {
    let response = router.dispatch(command(AppCommand::RunStart {
        session_id: session_id.clone(),
        user_message: "lease me".into(),
        model: None,
        profile: None,
    }));
    assert!(
        matches!(response.response, AppResponse::Accepted { .. }),
        "RunStart 应 Accepted，got {:?}",
        response.response
    );
    router.last_started_run().expect("run id")
}

/// session 作用域 canonical root AgentId（P18-4 审查补救）：与
/// `app_service` 生产派生契约一致（`root-<session_id>`），客户端不可选择。
fn canonical_root_agent_id(session_id: &SessionId) -> AgentId {
    AgentId::new(format!("root-{}", session_id.as_str()))
}

async fn wait_for_run_state(router: &CommandRouter, run_id: &RunId, expected: RunState) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if router
            .aggregate()
            .get_run(run_id, &local_tenant())
            .is_some_and(|run| run.state == expected)
        {
            return;
        }
        if Instant::now() >= deadline {
            panic!("run {run_id} did not reach {expected:?}");
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

async fn wait_until<F: Fn() -> bool>(condition: F) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if condition() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(condition(), "condition not met within timeout");
}

/// 池默认账号（acquire 请求 account_id=None 时由池回退）。
fn pool_default_account() -> AccountId {
    AccountId::new("local/default")
}

/// 成功 run：acquire 一次 lease、usage 归属来自真实 CredentialLease、
/// 终态释放归还额度。
#[tokio::test]
async fn run_acquires_lease_and_attributes_usage_from_it() {
    let pool = Arc::new(InMemoryCredentialPool::new(1));
    let (router, ledger) = router_with_pool_and_ledger(pool.clone());
    let session_id = prepare_session(&router);
    let run_id = start_run(&router, &session_id);

    // `complete()` 脚本立即完成，中间态 active=1 窗口不可靠观测；租约持有至
    // 终态 + 归还由本文件其它用例（阻塞流 + 取消）覆盖。此处聚焦终态：
    // 释放无泄漏 + usage 归属来自真实 CredentialLease。
    wait_for_run_state(&router, &run_id, RunState::Completed).await;
    wait_until(|| pool.active_count_for(&local_tenant(), &pool_default_account()) == 0).await;

    // usage 归属来自真实 CredentialLease：account/credential/trace 均非 synthetic。
    let records = ledger
        .query(&usage_ledger::UsageQuery {
            tenant_id: Some(local_tenant()),
            run_id: Some(run_id.clone()),
            ..Default::default()
        })
        .await
        .expect("query ledger");
    assert!(!records.is_empty(), "run 应产生 usage 记录");
    let expected_agent = canonical_root_agent_id(&session_id);
    for record in &records {
        assert_eq!(record.credential_id.as_deref(), Some("default"));
        assert_eq!(record.account_id, "local/default");
        assert_eq!(
            record.agent_id, expected_agent,
            "usage 归属必须为 session 作用域 canonical root agent（客户端不可选）"
        );
        assert_ne!(record.agent_id, AgentId::default());
        // trace 格式：run:<run_id>:attempt:<attempt>（来自 acquire 请求）。
        assert!(
            record.trace_id.as_deref() == Some(&format!("run:{run_id}:attempt:0")),
            "trace 应来自 acquire 请求，got {:?}",
            record.trace_id
        );
        assert_eq!(record.tenant_id, local_tenant());
        assert_eq!(record.principal_id.as_str(), "local/user");
    }
}

/// acquire 失败 fail-closed：并发额度满时第二个 run 直接 Failed，绝不调用
/// provider；第一个 run 的租约不受影响。
#[tokio::test]
async fn acquire_failure_fails_closed_without_provider_call() {
    let pool = Arc::new(InMemoryCredentialPool::new(1));
    let provider = Arc::new(
        test_support::MockProvider::new(test_support::MockScript::new().wait_for_cancellation())
            .with_id(ProviderId::from("mock")),
    );
    let runtime = QuotaRuntime::production_in_memory();
    let router = CommandRouter::new(RouterConfig::default());
    router.set_credential_pool(pool.clone());
    router.set_quota_runtime(runtime);
    router.register_provider(provider.clone());
    let session_id = prepare_session(&router);

    // run1 持有租约并阻塞在 provider 流上。
    let run1 = start_run(&router, &session_id);
    wait_for_run_state(&router, &run1, RunState::StreamingResponse).await;
    assert_eq!(
        pool.active_count_for(&local_tenant(), &pool_default_account()),
        1
    );

    // run2 在 acquire 处 fail-closed：Failed，且 provider 调用数仍为 1（run2 未到达）。
    let run2 = start_run(&router, &session_id);
    wait_for_run_state(&router, &run2, RunState::Failed).await;
    assert_eq!(provider.calls().len(), 1, "run2 不得调用 provider");
    assert_eq!(
        pool.active_count_for(&local_tenant(), &pool_default_account()),
        1,
        "run1 的租约不受 run2 失败影响"
    );

    // run1 取消后额度归还。
    router.dispatch(command(AppCommand::RunCancel {
        run_id: run1.clone(),
    }));
    wait_for_run_state(&router, &run1, RunState::Cancelled).await;
    wait_until(|| pool.active_count_for(&local_tenant(), &pool_default_account()) == 0).await;
}

/// retry 重新 acquire：取消后额度归还，retry 的新 attempt 再次持租约运行。
#[tokio::test]
async fn retry_reacquires_lease_for_new_attempt() {
    let pool = Arc::new(InMemoryCredentialPool::new(1));
    let provider: Arc<dyn provider_api::ModelProvider> = Arc::new(
        test_support::MockProvider::new(test_support::MockScript::new().wait_for_cancellation())
            .with_id(ProviderId::from("mock")),
    );
    let runtime = QuotaRuntime::production_in_memory();
    let router = CommandRouter::new(RouterConfig::default());
    router.set_credential_pool(pool.clone());
    router.set_quota_runtime(runtime);
    router.register_provider(provider);
    let session_id = prepare_session(&router);

    // 首次 attempt：acquire → 阻塞 → 取消 → 释放。
    let run_id = start_run(&router, &session_id);
    wait_for_run_state(&router, &run_id, RunState::StreamingResponse).await;
    assert_eq!(
        pool.active_count_for(&local_tenant(), &pool_default_account()),
        1
    );
    router.dispatch(command(AppCommand::RunCancel {
        run_id: run_id.clone(),
    }));
    wait_for_run_state(&router, &run_id, RunState::Cancelled).await;
    wait_until(|| pool.active_count_for(&local_tenant(), &pool_default_account()) == 0).await;

    // retry：新 attempt 重新 acquire（阻塞流期间 active 再次为 1）。
    let retry = router.dispatch(command(AppCommand::RunRetry {
        run_id: run_id.clone(),
    }));
    assert!(
        matches!(retry.response, AppResponse::Data(_)),
        "retry 应成功，got {:?}",
        retry.response
    );
    wait_for_run_state(&router, &run_id, RunState::StreamingResponse).await;
    assert_eq!(
        pool.active_count_for(&local_tenant(), &pool_default_account()),
        1,
        "retry 的 attempt 必须重新 acquire"
    );
    assert_eq!(router.supervisor().stats().retried, 1);

    router.dispatch(command(AppCommand::RunCancel {
        run_id: run_id.clone(),
    }));
    wait_for_run_state(&router, &run_id, RunState::Cancelled).await;
    wait_until(|| pool.active_count_for(&local_tenant(), &pool_default_account()) == 0).await;
}
