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

/// P18-2：本地 CLI 身份解析出的真实租户（local/default）。
fn local_tenant() -> TenantId {
    tenant_service::IdentityContext::local().tenant_id
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
    core_api::QuotaOverviewQuery {
        // P14 review §2.4：provider 是查询的必要维度，测试 fixture 显式提供
        // mock（本文件所有 router 都注册了 mock provider）；scope 仍为默认
        // local/local/default。
        provider_id: Some(ProviderId::from("mock")),
        ..core_api::QuotaOverviewQuery::default_local()
    }
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
        local_tenant(),
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
async fn system_identity_uses_explicit_local_system_principal() {
    let (router, runtime) = router_with_quota_and_provider();
    seed_cache(&runtime).await;
    let query = core_api::QuotaOverviewQuery {
        tenant_id: local_tenant(),
        account_id: core_api::DEFAULT_QUOTA_ACCOUNT.into(),
        ..default_query()
    };
    // P18-2：System 有显式 local/system principal；查询仍走 tenant scope。
    let response = router.dispatch_query(quota_query(cli_source(), ActorIdentity::System, query));
    assert!(matches!(response.response, AppResponse::Data(_)));
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
async fn canonical_tenant_default_scope_is_authorized_and_reads_same_ledger() {
    // P18-8 租户分歧回归：canonical 身份租户 `local/default`（P18-2 归一后
    // `record_run_usage` 写入账本所用的 tenant）与 legacy 哨兵 `local`
    // 必须映射为同一默认作用域。`pawork usage --tenant local/default` 不得
    // 被授权误拒，也不得查错租户。
    let (router, runtime) = router_with_quota_and_provider();
    let record = usage_ledger::UsageRecord {
        record_id: "canonical-tenant-1".into(),
        // 与 run 记账路径一致：identity.tenant_id = local/default。
        tenant_id: local_tenant(),
        principal_id: agent_domain::PrincipalId::default(),
        account_id: core_api::DEFAULT_QUOTA_ACCOUNT.to_string(),
        credential_id: None,
        session_id: SessionId::default(),
        agent_id: agent_domain::AgentId::default(),
        run_id: Some(RunId::from("run-canonical")),
        provider_id: ProviderId::from("mock"),
        model_id: ModelId::from("mock-model"),
        input_tokens: 100,
        output_tokens: 50,
        cache_read_tokens: 0,
        cache_write_tokens: 0,
        cost_micros: 0,
        currency: "USD".into(),
        occurred_at_ms: 1,
        ..usage_ledger::UsageRecord::default()
    };
    runtime
        .ledger
        .record(record.clone())
        .await
        .expect("record into ledger");
    runtime
        .refresh_local_cache(&record)
        .await
        .expect("cache refresh");

    // canonical tenant 形式：此前被误判为非默认作用域而拒绝。
    let canonical = core_api::QuotaOverviewQuery {
        tenant_id: TenantId::new(core_api::DEFAULT_QUOTA_TENANT_CANONICAL),
        ..default_query()
    };
    let response = router.dispatch_query(quota_query(cli_source(), cli_identity(), canonical));
    let AppResponse::Data(value) = response.response else {
        panic!(
            "canonical default scope must be authorized: {:?}",
            response.response
        );
    };
    assert_eq!(
        value
            .get("scope")
            .and_then(|s| s.get("tenant_id"))
            .and_then(serde_json::Value::as_str),
        Some(core_api::DEFAULT_QUOTA_TENANT_CANONICAL),
        "canonical 查询按同一默认作用域读账本，不再查错租户"
    );
    assert_eq!(
        value
            .get("windows")
            .and_then(serde_json::Value::as_array)
            .map(|windows| windows.len()),
        Some(4),
        "windows present: {value}"
    );

    // legacy 哨兵形式：同一账本、同一聚合结果。
    let legacy = core_api::QuotaOverviewQuery {
        tenant_id: TenantId::new(core_api::DEFAULT_QUOTA_TENANT),
        ..default_query()
    };
    let response = router.dispatch_query(quota_query(cli_source(), cli_identity(), legacy));
    let AppResponse::Data(legacy_value) = response.response else {
        panic!(
            "legacy default scope must be authorized: {:?}",
            response.response
        );
    };
    assert_eq!(
        legacy_value
            .get("windows")
            .and_then(serde_json::Value::as_array)
            .and_then(|windows| windows.first())
            .and_then(|w| w.get("read"))
            .and_then(|r| r.get("snapshot"))
            .and_then(|s| s.get("values"))
            .and_then(|r| r.get("used"))
            .and_then(|u| u.get("kind"))
            .and_then(serde_json::Value::as_str),
        Some("exact"),
        "legacy 与 canonical 必须读到同一账本聚合（150 tokens）"
    );
    assert_eq!(
        legacy_value
            .get("windows")
            .and_then(serde_json::Value::as_array)
            .and_then(|windows| windows.first())
            .and_then(|w| w.get("read"))
            .and_then(|r| r.get("snapshot"))
            .and_then(|s| s.get("values"))
            .and_then(|r| r.get("used"))
            .and_then(|u| u.get("value"))
            .and_then(serde_json::Value::as_u64),
        Some(150),
        "legacy 与 canonical 必须读到同一账本聚合（150 tokens）"
    );
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
async fn missing_or_empty_provider_is_rejected_with_validation_error() {
    // P14 review §2.4：不再选择“首个已注册 provider”或空默认 ID。即使
    // 已注册 provider（旧实现会静默选中第一个），缺省/空 provider 也必须
    // 返回明确的 validation error。
    let (router, _runtime) = router_with_quota_and_provider();

    let missing = quota_query(
        cli_source(),
        cli_identity(),
        core_api::QuotaOverviewQuery::default_local(),
    );
    let response = router.dispatch_query(missing);
    match response.response {
        AppResponse::Error(context) => {
            assert_eq!(
                context.category,
                agent_domain::ErrorCategory::InvalidRequest,
                "missing provider must be a validation error: {context:?}"
            );
            assert!(
                context.message.contains("provider_id"),
                "error must name the missing dimension: {context:?}"
            );
        }
        other => panic!("expected validation error, got {other:?}"),
    }

    let empty = quota_query(
        cli_source(),
        cli_identity(),
        core_api::QuotaOverviewQuery {
            provider_id: Some(ProviderId::default()),
            ..core_api::QuotaOverviewQuery::default_local()
        },
    );
    let response = router.dispatch_query(empty);
    match response.response {
        AppResponse::Error(context) => {
            assert_eq!(
                context.category,
                agent_domain::ErrorCategory::InvalidRequest,
                "empty provider must be a validation error: {context:?}"
            );
        }
        other => panic!("expected validation error, got {other:?}"),
    }
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
        local_tenant(),
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
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_millis() as u64;
    let clock = Arc::new(quota_service::service::MutableQuotaClock::at(
        now_ms + 3_600_000,
    ));
    let runtime = runtime_with_counting_adapter(
        Arc::clone(&calls),
        exact_values(0, 0, 0),
        quota_service::Confidence::Exact,
        Arc::clone(&clock),
        Duration::from_secs(30),
    );
    let provider = Arc::new(
        test_support::MockProvider::new(
            test_support::MockScript::new()
                .usage(TokenUsage {
                    input_tokens: 1,
                    ..Default::default()
                })
                .complete(),
        )
        .with_id(ProviderId::from("mock")),
    );
    let router = router_with_runtime_and_provider(Arc::clone(&runtime), Arc::clone(&provider));

    // 先跑一次真实 LocalUser run，验证 usage 记账与缓存发布使用解析后的
    // local/default tenant，而不是 legacy quota tenant `local`。
    let first_session = prepare_session(&router);
    let first = router.dispatch(command(
        cli_source(),
        cli_identity(),
        AppCommand::RunStart {
            session_id: first_session,
            user_message: "prime real accounting cache".into(),
            model: None,
            profile: None,
        },
    ));
    assert!(matches!(first.response, AppResponse::Accepted { .. }));
    let first_run = router.last_started_run().expect("first run id");
    wait_for_run_state(&router, &first_run, RunState::Completed).await;

    let scope = quota_service::QuotaScope::new(
        local_tenant(),
        quota_service::AccountId::new(core_api::DEFAULT_QUOTA_ACCOUNT),
        ProviderId::from("mock"),
        Some(ModelId::from("default-model")),
    );
    assert_eq!(
        scope.tenant_id.as_str(),
        "local/default",
        "hard-stop cache must use the resolved LocalUser tenant"
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let cache_ready = runtime
            .quota
            .overview_cache_only(
                &scope,
                &[quota_service::QuotaWindow::Monthly],
                &quota_service::QuotaUnit::Token,
            )
            .is_ok_and(|overview| overview.hit_count() == 1);
        if cache_ready {
            break;
        }
        if Instant::now() >= deadline {
            panic!("real accounting chain did not publish local/default cache");
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let records = runtime
        .ledger
        .query(&usage_ledger::UsageQuery {
            tenant_id: Some(local_tenant()),
            run_id: Some(first_run),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(records.len(), 1, "first run must create one usage record");
    assert_eq!(records[0].principal_id.as_str(), "local/user");
    assert_eq!(provider.calls().len(), 1, "first run reaches provider once");

    // 用同一真实 scope 注入 fresh Exact=0。第二次 run 只能读取该 tenant 的
    // 缓存并在 provider 前硬停。
    runtime.quota.invalidate();
    seed_cache_for(
        &runtime,
        &scope,
        &[quota_service::QuotaWindow::Monthly],
        &quota_service::QuotaUnit::Token,
    )
    .await;
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "exact cap should fetch once"
    );

    let session_id = prepare_session(&router);
    let response = router.dispatch(command(
        cli_source(),
        cli_identity(),
        AppCommand::RunStart {
            session_id,
            user_message: "must not reach provider".into(),
            model: None,
            profile: None,
        },
    ));
    let AppResponse::Accepted {
        run_id: Some(run_id),
        ..
    } = &response.response
    else {
        panic!("RunStart 应 Accepted 且携带 run id");
    };
    let run_id = run_id.clone();
    wait_for_run_state(&router, &run_id, RunState::Failed).await;

    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "run-start cache scan called quota adapter"
    );
    assert_eq!(
        provider.calls().len(),
        1,
        "fresh Exact limit=0 must hard-stop before a second provider call"
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
        local_tenant(),
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
            profile: None,
        },
    ));
    let AppResponse::Accepted {
        run_id: Some(run_id),
        ..
    } = &response.response
    else {
        panic!("RunStart 应 Accepted 且携带 run id");
    };
    let run_id = run_id.clone();
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
            profile: None,
        },
    );
    let accepted = router.dispatch(start);
    let AppResponse::Accepted {
        run_id: Some(run_id),
        ..
    } = accepted.response
    else {
        panic!("run should start with a run id: {:?}", accepted.response);
    };
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if router
            .aggregate()
            .get_run(&run_id, &local_tenant())
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
        tenant_id: Some(local_tenant()),
        account_id: Some(core_api::DEFAULT_QUOTA_ACCOUNT.into()),
        run_id: Some(run_id.clone()),
        ..Default::default()
    };
    let records = runtime.ledger.query(&query).await.unwrap();
    assert_eq!(
        records.len(),
        1,
        "expected exactly one usage record per run, got {records:?}"
    );
    assert_eq!(records[0].input_tokens, 100);
    assert_eq!(records[0].output_tokens, 50);
    let replay = records[0].clone();
    let _ = runtime.ledger.record(replay).await;
    let records2 = runtime.ledger.query(&query).await.unwrap();
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
            profile: None,
        },
    ));
    let AppResponse::Accepted {
        run_id: Some(run_id),
        ..
    } = &response.response
    else {
        panic!("run should start with a run id: {:?}", response.response);
    };
    let run_id = run_id.clone();
    wait_for_run_state(&router, &run_id, RunState::Completed).await;

    // 终态可见后记账与本地缓存发布仍在同一任务中：轮询账户级 scope 的
    // Token 缓存，直到四个默认窗口全部命中（发布完成）。
    let scope = quota_service::QuotaScope::new(
        local_tenant(),
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
