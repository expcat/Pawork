//! P14-8 QuotaOverview 契约测试：授权、缓存查询、脱敏输出、幂等记账。

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use agent_domain::{
    ActorId, CancellationToken, CommandId, ConnectionId, GuiClientId, ModelId, ProviderId, QueryId,
    RunId, SessionId, TenantId, Timestamp, TokenUsage, WorkspaceId,
};
use app_service::{CommandRouter, QuotaRuntime, RouterConfig};
use core_api::{
    ActorIdentity, AppCommand, AppCommandEnvelope, AppQuery, AppQueryEnvelope, AppResponse,
    CommandSource, RunState, API_VERSION,
};

fn unique(prefix: &str) -> String {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    format!("{prefix}-{}", NEXT.fetch_add(1, Ordering::SeqCst))
}

fn temp_dir() -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("pawork-quota-{}", unique("ws")));
    std::fs::create_dir_all(&path).expect("temp dir");
    path
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

fn system_identity() -> ActorIdentity {
    ActorIdentity::System
}

fn remote_identity() -> ActorIdentity {
    ActorIdentity::AuthenticatedClient {
        subject: "remote-user".into(),
        actor_id: ActorId::from("remote-actor"),
    }
}

fn command(source: CommandSource, identity: ActorIdentity, cmd: AppCommand) -> AppCommandEnvelope {
    AppCommandEnvelope {
        api_version: API_VERSION,
        command_id: CommandId::from(unique("cmd")),
        source,
        identity,
        expected_revision: None,
        idempotency_key: None,
        issued_at: Timestamp::from_unix_millis(1),
        command: cmd,
    }
}

fn quota_query(
    source: CommandSource,
    identity: ActorIdentity,
    query: core_api::QuotaOverviewQuery,
) -> AppQueryEnvelope {
    AppQueryEnvelope {
        api_version: API_VERSION,
        request_id: QueryId::from(unique("req")),
        source,
        identity,
        issued_at: Timestamp::from_unix_millis(1),
        query: AppQuery::QuotaOverview { query },
    }
}

fn default_query() -> core_api::QuotaOverviewQuery {
    core_api::QuotaOverviewQuery::default_local()
}

struct CountingQuotaAdapter {
    calls: Arc<AtomicUsize>,
    values: quota_service::QuotaValues,
    confidence: quota_service::Confidence,
}

#[async_trait::async_trait]
impl quota_service::QuotaAdapter for CountingQuotaAdapter {
    fn kind(&self) -> quota_service::AdapterKind {
        quota_service::AdapterKind::ApiKeyApi
    }

    fn supports(&self, _request: &quota_service::QuotaRequest) -> bool {
        true
    }

    async fn fetch(
        &self,
        request: &quota_service::QuotaRequest,
        _credential: Option<&provider_api::ResolvedCredential>,
        _cancel: &CancellationToken,
    ) -> Result<quota_service::QuotaSnapshot, quota_service::QuotaError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(quota_service::QuotaSnapshot {
            scope: request.scope.clone(),
            window: request.window,
            unit: request.unit.clone(),
            values: self.values,
            reset: quota_service::QuotaReset::Unknown,
            confidence: self.confidence,
            provenance: quota_service::QuotaProvenance::new(
                quota_service::AdapterKind::ApiKeyApi,
                "counting-test-adapter",
                Timestamp::from_unix_millis(1_000_000),
            ),
        })
    }
}

fn exact_values(used: u64, limit: u64, remaining: u64) -> quota_service::QuotaValues {
    quota_service::QuotaValues {
        used: quota_service::QuotaMeasure::Exact(used),
        limit: quota_service::QuotaMeasure::Exact(limit),
        remaining: quota_service::QuotaMeasure::Exact(remaining),
    }
}

fn runtime_with_counting_adapter(
    calls: Arc<AtomicUsize>,
    values: quota_service::QuotaValues,
    confidence: quota_service::Confidence,
    clock: Arc<quota_service::service::MutableQuotaClock>,
    ttl: Duration,
) -> Arc<QuotaRuntime> {
    let clock_trait: Arc<dyn quota_service::service::QuotaClock> = clock;
    let quota = Arc::new(quota_service::service::QuotaService::with_ttl(
        Arc::clone(&clock_trait),
        ttl,
    ));
    quota.register(
        quota_service::service::ScopeMatch::any(),
        Arc::new(CountingQuotaAdapter {
            calls,
            values,
            confidence,
        }),
    );
    QuotaRuntime::from_parts(
        Arc::new(usage_ledger::InMemoryUsageLedger::new()),
        quota,
        clock_trait,
    )
}

fn router_with_runtime_and_provider(
    runtime: Arc<QuotaRuntime>,
    provider: Arc<test_support::MockProvider>,
) -> CommandRouter {
    let router = CommandRouter::new(RouterConfig::default());
    router.register_provider(provider);
    router.set_quota_runtime(runtime);
    router
}

async fn seed_cache_for(
    runtime: &QuotaRuntime,
    scope: &quota_service::QuotaScope,
    windows: &[quota_service::QuotaWindow],
    unit: &quota_service::QuotaUnit,
) {
    let cancel = CancellationToken::new();
    let overview = runtime.quota.overview(scope, windows, unit, &cancel).await;
    assert_eq!(overview.ok_count(), windows.len(), "cache seed failed");
}

fn router_with_quota_and_provider() -> (CommandRouter, Arc<QuotaRuntime>) {
    router_with_quota_and_provider_at(1_000_000)
}

fn router_with_quota_and_provider_at(clock_start_ms: u64) -> (CommandRouter, Arc<QuotaRuntime>) {
    let router = CommandRouter::new(RouterConfig::default());
    let provider: Arc<dyn provider_api::ModelProvider> = Arc::new(
        test_support::MockProvider::new(
            test_support::MockScript::new()
                .usage(TokenUsage {
                    input_tokens: 100,
                    output_tokens: 50,
                    ..Default::default()
                })
                .complete(),
        )
        .with_id(ProviderId::from("mock")),
    );
    router.register_provider(provider);
    let ledger: Arc<dyn usage_ledger::UsageLedger> =
        Arc::new(usage_ledger::InMemoryUsageLedger::new());
    let clock: Arc<dyn quota_service::service::QuotaClock> = Arc::new(
        quota_service::service::MutableQuotaClock::at(clock_start_ms),
    );
    let runtime = QuotaRuntime::new(ledger, clock);
    router.set_quota_runtime(Arc::clone(&runtime));
    (router, runtime)
}

async fn seed_cache(runtime: &QuotaRuntime) {
    let scope = quota_service::QuotaScope::new(
        TenantId::new(core_api::DEFAULT_QUOTA_TENANT),
        quota_service::AccountId::new(core_api::DEFAULT_QUOTA_ACCOUNT),
        ProviderId::from("mock"),
        None,
    );
    let windows = vec![quota_service::QuotaWindow::Monthly];
    let unit = quota_service::QuotaUnit::Token;
    let cancel = CancellationToken::new();
    let _ = runtime
        .quota
        .overview(&scope, &windows, &unit, &cancel)
        .await;
}

fn prepare_session(router: &CommandRouter) -> SessionId {
    use serde_json::Value;
    let response = router.dispatch(command(
        cli_source(),
        cli_identity(),
        AppCommand::WorkspaceAdd {
            root_path: temp_dir().to_string_lossy().into_owned(),
        },
    ));
    let workspace_id = match &response.response {
        AppResponse::Data(value) => WorkspaceId::from(
            value
                .get("id")
                .and_then(Value::as_str)
                .expect("workspace id"),
        ),
        other => panic!("expected workspace data, got {other:?}"),
    };
    let response = router.dispatch(command(
        cli_source(),
        cli_identity(),
        AppCommand::SessionCreate {
            workspace_id,
            title: Some("quota".into()),
        },
    ));
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

async fn wait_for_run_state(router: &CommandRouter, run_id: &RunId, expected: RunState) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if router
            .aggregate()
            .get_run(run_id)
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

#[tokio::test]
async fn local_cli_reads_default_scope_from_cache() {
    let (router, runtime) = router_with_quota_and_provider();
    seed_cache(&runtime).await;
    let response =
        router.dispatch_query(quota_query(cli_source(), cli_identity(), default_query()));
    let AppResponse::Data(value) = response.response else {
        panic!("expected data, got {:?}", response.response);
    };
    assert_eq!(
        value.get("from_cache").and_then(serde_json::Value::as_bool),
        Some(true),
    );
    let windows = value
        .get("windows")
        .and_then(serde_json::Value::as_array)
        .expect("windows array");
    assert!(!windows.is_empty());
    let has_ok = windows.iter().any(|w| {
        w.get("read")
            .and_then(|r| r.get("status"))
            .and_then(serde_json::Value::as_str)
            == Some("ok")
    });
    assert!(has_ok, "expected at least one ok window: {value}");
}

#[tokio::test]
async fn local_gui_reads_default_scope_from_cache() {
    let (router, runtime) = router_with_quota_and_provider();
    seed_cache(&runtime).await;
    let response = router.dispatch_query(quota_query(
        CommandSource::LocalGui {
            client_id: GuiClientId::from("local-gui-1"),
        },
        cli_identity(),
        default_query(),
    ));
    assert!(
        matches!(response.response, AppResponse::Data(_)),
        "local GUI should read default quota scope: {:?}",
        response.response
    );
}

#[tokio::test]
async fn remote_gui_is_denied_without_grant() {
    let (router, _runtime) = router_with_quota_and_provider();
    let response = router.dispatch_query(quota_query(
        CommandSource::RemoteGui {
            client_id: GuiClientId::from("remote-1"),
            connection_id: ConnectionId::from("conn-1"),
        },
        remote_identity(),
        default_query(),
    ));
    match response.response {
        AppResponse::Error(context) => {
            assert_eq!(context.category, agent_domain::ErrorCategory::Authorization);
        }
        other => panic!("expected authorization error, got {other:?}"),
    }
}

#[tokio::test]
async fn system_reads_any_tenant() {
    let (router, runtime) = router_with_quota_and_provider();
    seed_cache(&runtime).await;
    let query = core_api::QuotaOverviewQuery {
        tenant_id: TenantId::new("local"),
        account_id: core_api::DEFAULT_QUOTA_ACCOUNT.into(),
        ..default_query()
    };
    let response = router.dispatch_query(quota_query(cli_source(), system_identity(), query));
    assert!(
        matches!(response.response, AppResponse::Data(_)),
        "system should read any tenant: {:?}",
        response.response
    );
}

#[tokio::test]
async fn local_cli_denied_for_non_default_scope() {
    let (router, _runtime) = router_with_quota_and_provider();
    let query = core_api::QuotaOverviewQuery {
        tenant_id: TenantId::new("acme"),
        account_id: "acme/team".into(),
        ..default_query()
    };
    let response = router.dispatch_query(quota_query(cli_source(), cli_identity(), query));
    match response.response {
        AppResponse::Error(context) => {
            assert_eq!(context.category, agent_domain::ErrorCategory::Authorization);
        }
        other => panic!("expected authorization error, got {other:?}"),
    }
}

#[tokio::test]
async fn no_runtime_returns_no_data_windows() {
    let router = CommandRouter::new(RouterConfig::default());
    let response =
        router.dispatch_query(quota_query(cli_source(), cli_identity(), default_query()));
    let AppResponse::Data(value) = response.response else {
        panic!("expected data, got {:?}", response.response);
    };
    assert_eq!(
        value.get("from_cache").and_then(serde_json::Value::as_bool),
        Some(false),
    );
    let windows = value
        .get("windows")
        .and_then(serde_json::Value::as_array)
        .expect("windows array");
    assert!(
        windows.iter().all(|w| {
            w.get("read")
                .and_then(|r| r.get("status"))
                .and_then(serde_json::Value::as_str)
                == Some("no_data")
        }),
        "all windows should be no_data: {value}"
    );
}

#[tokio::test]
async fn cache_only_miss_never_calls_adapter() {
    let calls = Arc::new(AtomicUsize::new(0));
    let runtime = runtime_with_counting_adapter(
        Arc::clone(&calls),
        exact_values(1, 10, 9),
        quota_service::Confidence::Exact,
        Arc::new(quota_service::service::MutableQuotaClock::at(1_000_000)),
        Duration::from_secs(30),
    );
    let provider = Arc::new(
        test_support::MockProvider::new(test_support::MockScript::new().complete())
            .with_id(ProviderId::from("mock")),
    );
    let router = router_with_runtime_and_provider(runtime, provider);
    let query = core_api::QuotaOverviewQuery {
        provider_id: Some(ProviderId::from("mock")),
        windows: vec![core_api::QuotaWindow::Weekly],
        unit: Some(core_api::QuotaUnit::Count),
        ..default_query()
    };
    let response = router.dispatch_query(quota_query(cli_source(), cli_identity(), query));
    let AppResponse::Data(value) = response.response else {
        panic!(
            "expected cache-only data response, got {:?}",
            response.response
        );
    };
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "cache miss fetched adapter"
    );
    assert_eq!(value["windows"].as_array().map(Vec::len), Some(1));
    assert_eq!(value["windows"][0]["window"], "weekly");
    assert_eq!(value["windows"][0]["read"]["status"], "no_data");
}

#[tokio::test]
async fn cache_only_query_respects_cost_unit_and_explicit_window_subset() {
    let calls = Arc::new(AtomicUsize::new(0));
    let runtime = runtime_with_counting_adapter(
        Arc::clone(&calls),
        exact_values(250_000, 1_000_000, 750_000),
        quota_service::Confidence::Exact,
        Arc::new(quota_service::service::MutableQuotaClock::at(1_000_000)),
        Duration::from_secs(30),
    );
    let provider = Arc::new(
        test_support::MockProvider::new(test_support::MockScript::new().complete())
            .with_id(ProviderId::from("mock")),
    );
    let router = router_with_runtime_and_provider(Arc::clone(&runtime), provider);
    let scope = quota_service::QuotaScope::new(
        TenantId::new(core_api::DEFAULT_QUOTA_TENANT),
        quota_service::AccountId::new(core_api::DEFAULT_QUOTA_ACCOUNT),
        ProviderId::from("mock"),
        None,
    );
    let unit = quota_service::QuotaUnit::Cost {
        currency: "USD".into(),
    };
    seed_cache_for(
        &runtime,
        &scope,
        &[quota_service::QuotaWindow::Monthly],
        &unit,
    )
    .await;
    assert_eq!(calls.load(Ordering::SeqCst), 1, "seed should fetch once");

    let query = core_api::QuotaOverviewQuery {
        provider_id: Some(ProviderId::from("mock")),
        windows: vec![core_api::QuotaWindow::Monthly],
        unit: Some(core_api::QuotaUnit::Cost {
            currency: "USD".into(),
        }),
        ..default_query()
    };
    let response = router.dispatch_query(quota_query(cli_source(), cli_identity(), query));
    let AppResponse::Data(value) = response.response else {
        panic!(
            "expected cache-only data response, got {:?}",
            response.response
        );
    };
    assert_eq!(calls.load(Ordering::SeqCst), 1, "query fetched adapter");
    let windows = value["windows"].as_array().expect("windows array");
    assert_eq!(
        windows.len(),
        1,
        "explicit subset must not add default windows"
    );
    assert_eq!(windows[0]["window"], "monthly");
    assert_eq!(windows[0]["read"]["status"], "ok");
    assert_eq!(windows[0]["read"]["snapshot"]["unit"]["kind"], "cost");
    assert_eq!(windows[0]["read"]["snapshot"]["unit"]["currency"], "USD");
}

#[tokio::test]
async fn credential_is_masked_in_output() {
    let (router, runtime) = router_with_quota_and_provider();
    seed_cache(&runtime).await;
    let secret = "sk-super-secret-key-abcd";
    let query = core_api::QuotaOverviewQuery {
        credential_id: Some(secret.into()),
        ..default_query()
    };
    let response = router.dispatch_query(quota_query(cli_source(), cli_identity(), query));
    let AppResponse::Data(value) = response.response else {
        panic!("expected data, got {:?}", response.response);
    };
    let json = serde_json::to_string(&value).expect("json");
    assert!(!json.contains(secret), "raw credential leaked: {json}");
    let hint = core_api::mask_credential_hint(secret).expect("masked");
    assert!(json.contains(&hint), "masked hint missing: {json}");
}

#[tokio::test]
async fn fresh_exact_zero_limit_cache_signal_hard_stops_without_fetch() {
    let calls = Arc::new(AtomicUsize::new(0));
    let runtime = runtime_with_counting_adapter(
        Arc::clone(&calls),
        exact_values(0, 0, 0),
        quota_service::Confidence::Exact,
        Arc::new(quota_service::service::MutableQuotaClock::at(1_000_000)),
        Duration::from_secs(30),
    );
    let provider = Arc::new(
        test_support::MockProvider::new(test_support::MockScript::new().complete())
            .with_id(ProviderId::from("mock")),
    );
    let router = router_with_runtime_and_provider(Arc::clone(&runtime), Arc::clone(&provider));
    let scope = quota_service::QuotaScope::new(
        TenantId::new(core_api::DEFAULT_QUOTA_TENANT),
        quota_service::AccountId::new(core_api::DEFAULT_QUOTA_ACCOUNT),
        ProviderId::from("mock"),
        Some(ModelId::from("default-model")),
    );
    seed_cache_for(
        &runtime,
        &scope,
        &[quota_service::QuotaWindow::Monthly],
        &quota_service::QuotaUnit::Token,
    )
    .await;
    assert_eq!(calls.load(Ordering::SeqCst), 1, "seed should fetch once");

    let session_id = prepare_session(&router);
    let response = router.dispatch(command(
        cli_source(),
        cli_identity(),
        AppCommand::RunStart {
            session_id,
            user_message: "must not reach provider".into(),
            model: None,
        },
    ));
    assert!(matches!(response.response, AppResponse::Accepted { .. }));
    let run_id = router.last_started_run().expect("run id");
    wait_for_run_state(&router, &run_id, RunState::Failed).await;

    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "run-start cache scan called quota adapter"
    );
    assert!(
        provider.calls().is_empty(),
        "fresh Exact limit=0 must hard-stop before provider"
    );
}

#[tokio::test]
async fn stale_exact_zero_limit_cache_signal_does_not_hard_stop_or_refetch() {
    let calls = Arc::new(AtomicUsize::new(0));
    let clock = Arc::new(quota_service::service::MutableQuotaClock::at(1_000_000));
    let runtime = runtime_with_counting_adapter(
        Arc::clone(&calls),
        exact_values(0, 0, 0),
        quota_service::Confidence::Exact,
        Arc::clone(&clock),
        Duration::from_millis(10),
    );
    let provider = Arc::new(
        test_support::MockProvider::new(test_support::MockScript::new().complete())
            .with_id(ProviderId::from("mock")),
    );
    let router = router_with_runtime_and_provider(Arc::clone(&runtime), Arc::clone(&provider));
    let scope = quota_service::QuotaScope::new(
        TenantId::new(core_api::DEFAULT_QUOTA_TENANT),
        quota_service::AccountId::new(core_api::DEFAULT_QUOTA_ACCOUNT),
        ProviderId::from("mock"),
        Some(ModelId::from("default-model")),
    );
    seed_cache_for(
        &runtime,
        &scope,
        &[quota_service::QuotaWindow::Monthly],
        &quota_service::QuotaUnit::Token,
    )
    .await;
    clock.advance(11);

    let session_id = prepare_session(&router);
    let response = router.dispatch(command(
        cli_source(),
        cli_identity(),
        AppCommand::RunStart {
            session_id,
            user_message: "stale signal is warning-only".into(),
            model: None,
        },
    ));
    assert!(matches!(response.response, AppResponse::Accepted { .. }));
    let run_id = router.last_started_run().expect("run id");
    wait_for_run_state(&router, &run_id, RunState::Completed).await;

    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "stale cache scan must not refresh adapter"
    );
    assert_eq!(provider.calls().len(), 1, "stale Exact must not hard-stop");
}

#[tokio::test]
async fn successful_run_records_usage_once() {
    let (router, runtime) = router_with_quota_and_provider();
    let session_id = prepare_session(&router);
    let start = command(
        cli_source(),
        cli_identity(),
        AppCommand::RunStart {
            session_id: session_id.clone(),
            user_message: "do work".into(),
            model: None,
        },
    );
    let accepted = router.dispatch(start);
    assert!(
        matches!(accepted.response, AppResponse::Accepted { .. }),
        "run should start: {:?}",
        accepted.response
    );
    let run_id = router.last_started_run().expect("run id");
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if router
            .aggregate()
            .get_run(&run_id)
            .is_some_and(|run| run.state == RunState::Completed)
        {
            break;
        }
        if Instant::now() >= deadline {
            panic!("run did not complete in time");
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    tokio::time::sleep(Duration::from_millis(100)).await;
    let query = usage_ledger::UsageQuery {
        tenant_id: Some(TenantId::new(core_api::DEFAULT_QUOTA_TENANT)),
        account_id: Some(core_api::DEFAULT_QUOTA_ACCOUNT.into()),
        run_id: Some(run_id.clone()),
        ..Default::default()
    };
    let records = runtime.ledger.query(&query).await;
    assert_eq!(
        records.len(),
        1,
        "expected exactly one usage record per run, got {records:?}"
    );
    assert_eq!(records[0].input_tokens, 100);
    assert_eq!(records[0].output_tokens, 50);
    let replay = records[0].clone();
    let _ = runtime.ledger.record(replay).await;
    let records2 = runtime.ledger.query(&query).await;
    assert_eq!(
        records2.len(),
        1,
        "idempotent replay must not double-record: {records2:?}"
    );
}

#[tokio::test]
async fn successful_run_publishes_cache_for_default_scope_token_and_cost_overviews() {
    // run 事件时间戳来自真实墙钟，而 ledger 窗口边界由 quota 时钟锚定；把固定
    // 时钟放到「现在 + 1h」，保证刚记录的 usage 严格落在四个窗口的半开区间内。
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_millis() as u64;
    let (router, runtime) = router_with_quota_and_provider_at(now_ms + 3_600_000);
    let session_id = prepare_session(&router);
    let response = router.dispatch(command(
        cli_source(),
        cli_identity(),
        AppCommand::RunStart {
            session_id,
            user_message: "cross-layer quota run".into(),
            model: None,
        },
    ));
    assert!(
        matches!(response.response, AppResponse::Accepted { .. }),
        "run should start: {:?}",
        response.response
    );
    let run_id = router.last_started_run().expect("run id");
    wait_for_run_state(&router, &run_id, RunState::Completed).await;

    // 终态可见后记账与本地缓存发布仍在同一任务中：轮询账户级 scope 的
    // Token 缓存，直到四个默认窗口全部命中（发布完成）。
    let scope = quota_service::QuotaScope::new(
        TenantId::new(core_api::DEFAULT_QUOTA_TENANT),
        quota_service::AccountId::new(core_api::DEFAULT_QUOTA_ACCOUNT),
        ProviderId::from("mock"),
        None,
    );
    let windows = [
        quota_service::QuotaWindow::Overall,
        quota_service::QuotaWindow::Rolling5h,
        quota_service::QuotaWindow::Weekly,
        quota_service::QuotaWindow::Monthly,
    ];
    let token_unit = quota_service::QuotaUnit::Token;
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match runtime
            .quota
            .overview_cache_only(&scope, &windows, &token_unit)
        {
            Ok(overview) if overview.hit_count() == windows.len() => break,
            _ if Instant::now() >= deadline => {
                panic!("token cache was not published after run completion")
            }
            _ => tokio::time::sleep(Duration::from_millis(25)).await,
        }
    }

    // 默认 local scope + 显式 provider，model/credential/unit 省略 → Token。
    let query = core_api::QuotaOverviewQuery {
        provider_id: Some(ProviderId::from("mock")),
        ..default_query()
    };
    let response = router.dispatch_query(quota_query(cli_source(), cli_identity(), query));
    let AppResponse::Data(value) = response.response else {
        panic!("expected data, got {:?}", response.response);
    };
    assert_eq!(
        value.get("from_cache").and_then(serde_json::Value::as_bool),
        Some(true),
    );
    assert_eq!(
        value
            .get("scope")
            .and_then(|scope| scope.get("provider_id"))
            .and_then(serde_json::Value::as_str),
        Some("mock"),
        "explicit provider must map into the returned scope"
    );
    let windows_json = value
        .get("windows")
        .and_then(serde_json::Value::as_array)
        .expect("windows array");
    assert_eq!(
        windows_json.len(),
        4,
        "default query should cover 4 windows"
    );
    let expected: [&str; 4] = ["overall", "rolling5h", "weekly", "monthly"];
    for (entry, name) in windows_json.iter().zip(expected) {
        assert_eq!(
            entry.get("window").and_then(serde_json::Value::as_str),
            Some(name),
            "unexpected window in {value}"
        );
        assert_eq!(
            entry
                .get("read")
                .and_then(|read| read.get("status"))
                .and_then(serde_json::Value::as_str),
            Some("ok"),
            "window {name} should hit cache: {entry}"
        );
        let snapshot = entry
            .get("read")
            .and_then(|read| read.get("snapshot"))
            .expect("snapshot");
        assert_eq!(
            snapshot
                .get("unit")
                .and_then(|unit| unit.get("kind"))
                .and_then(serde_json::Value::as_str),
            Some("token"),
        );
        let used = snapshot
            .get("values")
            .and_then(|values| values.get("used"))
            .expect("used measure");
        assert_eq!(
            used.get("kind").and_then(serde_json::Value::as_str),
            Some("exact"),
        );
        assert_eq!(
            used.get("value").and_then(serde_json::Value::as_u64),
            Some(150),
            "used must be total tokens (100 input + 50 output) in {entry}"
        );
    }

    // 显式 Cost USD：同样四个窗口全部命中。
    let query = core_api::QuotaOverviewQuery {
        provider_id: Some(ProviderId::from("mock")),
        unit: Some(core_api::QuotaUnit::Cost {
            currency: "USD".into(),
        }),
        ..default_query()
    };
    let response = router.dispatch_query(quota_query(cli_source(), cli_identity(), query));
    let AppResponse::Data(value) = response.response else {
        panic!("expected data, got {:?}", response.response);
    };
    assert_eq!(
        value.get("from_cache").and_then(serde_json::Value::as_bool),
        Some(true),
    );
    let windows_json = value
        .get("windows")
        .and_then(serde_json::Value::as_array)
        .expect("windows array");
    assert_eq!(
        windows_json.len(),
        4,
        "cost query should hit all 4 default windows"
    );
    for entry in windows_json {
        assert_eq!(
            entry
                .get("read")
                .and_then(|read| read.get("status"))
                .and_then(serde_json::Value::as_str),
            Some("ok"),
            "cost window should hit cache: {entry}"
        );
        let unit = entry
            .get("read")
            .and_then(|read| read.get("snapshot"))
            .and_then(|snapshot| snapshot.get("unit"))
            .expect("unit");
        assert_eq!(
            unit.get("kind").and_then(serde_json::Value::as_str),
            Some("cost"),
        );
        assert_eq!(
            unit.get("currency").and_then(serde_json::Value::as_str),
            Some("USD"),
        );
    }
}
