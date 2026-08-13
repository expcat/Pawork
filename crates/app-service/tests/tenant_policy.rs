//! P18-9 租户策略 / RBAC 集成测试（facade 级强制入口）。
//!
//! 覆盖：默认 `local/default` 兼容、角色 deny-first、跨租户隔离、
//! Audit 读取权限与租户作用域、PolicyManage 闸口、routing adapter 复用
//! provider-control 契约且不可被覆盖。

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use agent_domain::{
    AccountId, ActorId, AgentId, CommandId, CredentialId, ErrorCategory, GuiClientId, ModelId,
    PrincipalId, ProviderId, QueryId, SessionId, TenantId, Timestamp, WorkspaceId,
};
use app_service::{
    AppService, AppServiceError, CommandRouter, LocalIdentityResolver, QuotaRuntime, RouterConfig,
    RoutingTenantPolicyAdapter, TenantPolicyGate,
};
use audit_log::{
    AuditAction, AuditDecision, AuditDimensions, AuditTargetKind, InMemoryOtelExporter,
};
use core_api::{
    ActorIdentity, AppCommand, AppCommandEnvelope, AppQuery, AppQueryEnvelope, AppResponse,
    AppResponseEnvelope, CommandSource, PolicyDecisionKind as ApiDecisionKind,
    PolicyGate as ApiGate, RunState, API_VERSION,
};
use provider_control::routing::{
    RouteCandidate, RouteContext, TenantPolicy as RoutingTenantPolicy,
};
use provider_control::{CredentialPool, InMemoryCredentialPool};
use quota_service::service::MutableQuotaClock;
use tenant_service::{
    AuditExportPolicy, IdentityContext, InMemoryTenantPolicyEngine, PermissionProfile,
    PolicyDecisionKind, PolicyGate, PrincipalRole, TenantPolicy, TenantPolicyEngine,
};
use usage_ledger::{InMemoryUsageLedger, UsageLedger};

fn cli_source() -> CommandSource {
    CommandSource::LocalCli {
        terminal_session_id: None,
    }
}

fn gui_source() -> CommandSource {
    CommandSource::LocalGui {
        client_id: GuiClientId::from("policy-gui-1"),
    }
}

fn cli_identity() -> ActorIdentity {
    ActorIdentity::LocalUser {
        actor_id: ActorId::from("tester"),
        display_name: None,
    }
}

fn query(source: CommandSource, identity: ActorIdentity, query: AppQuery) -> AppQueryEnvelope {
    AppQueryEnvelope {
        api_version: API_VERSION,
        request_id: QueryId::from("policy-req-1"),
        source,
        identity,
        issued_at: Timestamp::from_unix_millis(1),
        query,
    }
}

fn default_usage_query() -> AppQuery {
    AppQuery::QuotaOverview {
        query: core_api::QuotaOverviewQuery {
            provider_id: Some(ProviderId::new("mock")),
            ..core_api::QuotaOverviewQuery::default_local()
        },
    }
}

fn assert_authorization(response: &AppResponseEnvelope, fragment: Option<&str>) {
    match &response.response {
        AppResponse::Error(context) => {
            assert_eq!(
                context.category,
                ErrorCategory::Authorization,
                "expected authorization error, got {:?}",
                response.response
            );
            if let Some(fragment) = fragment {
                assert!(
                    context.message.contains(fragment),
                    "error message must contain `{fragment}`: {:?}",
                    context.message
                );
            }
        }
        other => panic!("expected authorization error, got {other:?}"),
    }
}

#[tokio::test]
async fn default_local_default_policy_keeps_legacy_queries_working() {
    // 未配置租户继续使用默认 local/default 策略（Admin）：查询放行并记录
    // 版本化决策事件。
    let service = AppService::new("tenant-policy-default");
    let response =
        service.dispatch_query(query(cli_source(), cli_identity(), default_usage_query()));
    assert!(
        matches!(response.response, AppResponse::Data(_)),
        "unconfigured local/default must keep working: {:?}",
        response.response
    );

    let decisions = service
        .audit_decisions(&IdentityContext::local())
        .expect("default local/default is Admin and may read audit");
    assert!(decisions.iter().any(|event| {
        event.gate == ApiGate::UsageQuery && event.decision == ApiDecisionKind::Allow
    }));

    let canonical = service
        .canonical_audit_events(&IdentityContext::local())
        .expect("admin may read canonical audit");
    assert!(canonical.iter().any(|event| {
        event.action == AuditAction::PolicyEvaluated
            && event.target_kind == AuditTargetKind::Policy
            && event.decision == AuditDecision::Allow
            && event.reason_code == "policy_allow"
    }));
}

#[tokio::test]
async fn service_role_is_denied_session_read_but_allowed_usage() {
    let engine = Arc::new(InMemoryTenantPolicyEngine::new(TenantPolicy {
        permission_profile: Some(PermissionProfile {
            default_role: Some(PrincipalRole::Service),
            ..PermissionProfile::default()
        }),
        ..TenantPolicy::default()
    }));
    let service = AppService::with_tenant_policy("tenant-policy-service", engine.clone());

    // Service 无 SessionRead：CLI 与 GUI 同一预检入口，deny 无法被上层覆盖。
    let session =
        service.dispatch_query(query(cli_source(), cli_identity(), AppQuery::SnapshotFetch));
    assert_authorization(&session, Some("SessionRead"));
    let session_gui =
        service.dispatch_query(query(gui_source(), cli_identity(), AppQuery::SnapshotFetch));
    assert_authorization(&session_gui, None);

    // Service 仍可读 Usage（角色矩阵），放行进入 router。
    let usage = service.dispatch_query(query(cli_source(), cli_identity(), default_usage_query()));
    assert!(
        matches!(usage.response, AppResponse::Data(_)),
        "{:?}",
        usage.response
    );

    let decisions = engine.decisions(&TenantId::new("local/default"));
    assert!(decisions.iter().any(|event| {
        event.gate == PolicyGate::SessionQuery && event.decision == PolicyDecisionKind::Deny
    }));
    assert!(decisions.iter().any(|event| {
        event.gate == PolicyGate::UsageQuery && event.decision == PolicyDecisionKind::Allow
    }));
}

#[tokio::test]
async fn local_identity_cannot_observe_other_tenants_usage() {
    let engine = Arc::new(InMemoryTenantPolicyEngine::default());
    let service = AppService::with_tenant_policy("tenant-policy-cross", engine.clone());
    let response = service.dispatch_query(query(
        cli_source(),
        cli_identity(),
        AppQuery::QuotaOverview {
            query: core_api::QuotaOverviewQuery {
                tenant_id: TenantId::new("acme"),
                provider_id: Some(ProviderId::new("mock")),
                ..core_api::QuotaOverviewQuery::default_local()
            },
        },
    ));
    assert_authorization(&response, Some("cross-tenant"));
    let decisions = engine.decisions(&TenantId::new("local/default"));
    assert!(decisions.iter().any(|event| {
        event.gate == PolicyGate::UsageQuery
            && event.decision == PolicyDecisionKind::Deny
            && event.reason.contains("cross-tenant")
    }));
}

#[tokio::test]
async fn audit_read_requires_permission_and_is_tenant_scoped() {
    let mut profile = PermissionProfile {
        default_role: Some(PrincipalRole::Admin),
        ..PermissionProfile::default()
    };
    profile
        .principal_roles
        .insert(PrincipalId::new("local/user"), PrincipalRole::Viewer);
    let engine = Arc::new(InMemoryTenantPolicyEngine::new(TenantPolicy {
        permission_profile: Some(profile),
        ..TenantPolicy::default()
    }));
    let service = AppService::with_tenant_policy("tenant-policy-audit", engine.clone());

    // Viewer 无 AuditRead：拒绝，且拒绝本身记为 AuditQuery deny 事件。
    let err = service
        .audit_decisions(&IdentityContext::local())
        .unwrap_err();
    assert!(
        matches!(err, AppServiceError::Authorization(ref message) if message.contains("AuditRead")),
        "{err:?}"
    );

    // 同租户 Admin 读审计：能看到 Viewer 的拒绝事件（versioned + 脱敏）。
    let auditor = IdentityContext::new(
        TenantId::new("local/default"),
        PrincipalId::new("ops:auditor"),
    );
    let events = service
        .audit_decisions(&auditor)
        .expect("Admin may read audit");
    assert!(events.iter().any(|event| {
        event.gate == ApiGate::AuditQuery
            && event.decision == ApiDecisionKind::Deny
            && event.principal_id == "local/user"
    }));

    // deny-first：未知非 local/default 租户回落 Viewer，不可读审计。
    let unconfigured_auditor =
        IdentityContext::new(TenantId::new("acme"), PrincipalId::new("ops:auditor"));
    let err = service.audit_decisions(&unconfigured_auditor).unwrap_err();
    assert!(matches!(err, AppServiceError::Authorization(_)), "{err:?}");

    // 跨租户审计隔离：显式配置 acme 为 User（可读审计）后，看不到
    // local/default 的事件。
    engine.set_policy(
        TenantId::new("acme"),
        TenantPolicy {
            permission_profile: Some(PermissionProfile {
                default_role: Some(PrincipalRole::User),
                ..PermissionProfile::default()
            }),
            ..TenantPolicy::default()
        },
    );
    let acme_auditor = IdentityContext::new(TenantId::new("acme"), PrincipalId::new("ops:auditor"));
    let acme_events = service
        .audit_decisions(&acme_auditor)
        .expect("acme user may read audit");
    assert!(
        acme_events
            .iter()
            .all(|event| event.tenant_id == TenantId::new("acme")),
        "tenant acme must not observe local/default decisions: {acme_events:?}"
    );

    let local_canonical = service
        .canonical_audit_events(&auditor)
        .expect("admin may read canonical audit");
    let acme_canonical = service
        .canonical_audit_events(&acme_auditor)
        .expect("acme user may read canonical audit");
    assert!(local_canonical
        .iter()
        .all(|event| event.tenant_id == TenantId::new("local/default")));
    assert!(acme_canonical
        .iter()
        .all(|event| event.tenant_id == TenantId::new("acme")));
    assert!(local_canonical
        .iter()
        .any(|event| event.action == AuditAction::PolicyEvaluated));
}

#[tokio::test]
async fn policy_manage_gates_view_and_update() {
    let mut profile = PermissionProfile {
        default_role: Some(PrincipalRole::Admin),
        ..PermissionProfile::default()
    };
    profile
        .principal_roles
        .insert(PrincipalId::new("local/user"), PrincipalRole::Viewer);
    let engine = Arc::new(InMemoryTenantPolicyEngine::new(TenantPolicy {
        permission_profile: Some(profile),
        ..TenantPolicy::default()
    }));
    let service = AppService::with_tenant_policy("tenant-policy-manage", engine.clone());
    let tenant = TenantId::new("local/default");

    // Viewer 不可读 / 不可写策略。
    let err = service
        .tenant_policy_view(&IdentityContext::local(), &tenant)
        .unwrap_err();
    assert!(matches!(err, AppServiceError::Authorization(_)), "{err:?}");
    let err = service
        .set_tenant_policy(
            &IdentityContext::local(),
            tenant.clone(),
            TenantPolicy::default(),
        )
        .unwrap_err();
    assert!(matches!(err, AppServiceError::Authorization(_)), "{err:?}");

    // Admin 主体可读可写；每次更新版本递增。
    let admin = IdentityContext::new(tenant.clone(), PrincipalId::new("ops:admin"));
    let view = service
        .tenant_policy_view(&admin, &tenant)
        .expect("Admin may view");
    assert_eq!(view.version, 1);
    service
        .set_tenant_policy(
            &admin,
            tenant.clone(),
            TenantPolicy {
                max_concurrent_agents: Some(2),
                ..TenantPolicy::default()
            },
        )
        .expect("Admin may update");
    let view = service
        .tenant_policy_view(&admin, &tenant)
        .expect("Admin may view");
    assert_eq!(view.version, 2);
    assert_eq!(view.max_concurrent_agents, Some(2));
    let canonical = service
        .canonical_audit_events(&admin)
        .expect("admin may read canonical audit");
    assert!(
        canonical.iter().any(|event| {
            event.action == AuditAction::ConfigurationChanged
                && event.target_kind == AuditTargetKind::Configuration
                && event.reason_code == "tenant_policy_updated"
        }),
        "policy update must emit ConfigurationChanged: {canonical:?}"
    );
}

fn route_context() -> RouteContext {
    RouteContext {
        tenant_id: TenantId::new("local/default"),
        principal_id: PrincipalId::new("local/user"),
        session_id: SessionId::new("session-1"),
        agent_id: AgentId::new("agent-1"),
        model_id: ModelId::new("claude-3-5-sonnet"),
        ..RouteContext::default()
    }
}

fn route_candidate(provider: &str) -> RouteCandidate {
    RouteCandidate {
        account_id: AccountId::new("acct-a"),
        credential_id: CredentialId::new("cred-a"),
        provider_id: ProviderId::new(provider),
        model_id: ModelId::new("claude-3-5-sonnet"),
        priority: 0,
        weight: 1,
        capabilities: BTreeSet::new(),
        context_window_tokens: 128_000,
        max_output_tokens: 4_096,
        active_leases: 0,
        max_concurrency: 1,
    }
}

#[test]
fn routing_adapter_deny_first_records_route_decisions_and_cannot_be_overridden() {
    let engine = Arc::new(InMemoryTenantPolicyEngine::new(TenantPolicy {
        allowed_providers: Some(vec![ProviderId::new("openai")]),
        ..TenantPolicy::default()
    }));
    let gate = Arc::new(TenantPolicyGate::new(engine.clone()));
    let adapter = RoutingTenantPolicyAdapter::new(Arc::clone(&gate));
    let context = route_context();

    let err = adapter
        .allows(&context, &route_candidate("anthropic"))
        .unwrap_err();
    assert!(err.reason.contains("anthropic"), "{err:?}");
    adapter
        .allows(&context, &route_candidate("openai"))
        .expect("whitelisted provider must pass");
    let mut fallback = route_candidate("openai");
    fallback.priority = 1;
    adapter
        .allows(&context, &fallback)
        .expect("lower-priority whitelisted candidate is fallback");

    let decisions = engine.decisions(&TenantId::new("local/default"));
    assert!(decisions.iter().any(|event| {
        event.gate == PolicyGate::RouteCandidate && event.decision == PolicyDecisionKind::Deny
    }));
    assert!(decisions.iter().any(|event| {
        event.gate == PolicyGate::RouteCandidate && event.decision == PolicyDecisionKind::Allow
    }));
    assert!(decisions.iter().any(|event| {
        event.gate == PolicyGate::RouteCandidate && event.decision == PolicyDecisionKind::Fallback
    }));
    let canonical = gate
        .canonical_audit_events(&TenantId::new("local/default"))
        .unwrap();
    assert!(canonical.iter().any(|event| {
        event.action == AuditAction::RouteEvaluated && event.decision == AuditDecision::Deny
    }));
    assert!(canonical.iter().any(|event| {
        event.action == AuditAction::RouteEvaluated && event.decision == AuditDecision::Allow
    }));
    assert!(canonical.iter().any(|event| {
        event.action == AuditAction::RouteEvaluated
            && event.decision == AuditDecision::Fallback
            && event.reason_code == "route_candidate_fallback"
    }));

    // deny-first：Viewer 无 RouteCandidate 权限，即使 provider 命中白名单也拒绝；
    // adapter 无法覆盖 Core 的角色裁决。
    let engine2 = Arc::new(InMemoryTenantPolicyEngine::new(TenantPolicy {
        permission_profile: Some(PermissionProfile {
            default_role: Some(PrincipalRole::Viewer),
            ..PermissionProfile::default()
        }),
        allowed_providers: Some(vec![ProviderId::new("openai")]),
        ..TenantPolicy::default()
    }));
    let adapter2 = RoutingTenantPolicyAdapter::from_engine(engine2.clone());
    let err = adapter2
        .allows(&context, &route_candidate("openai"))
        .unwrap_err();
    assert!(err.reason.contains("RouteCandidate"), "{err:?}");
    assert!(engine2
        .decisions(&TenantId::new("local/default"))
        .iter()
        .any(|event| event.decision == PolicyDecisionKind::Deny));
}

#[test]
fn canonical_audit_export_is_tenant_scoped_redacted_and_records_export() {
    let engine = Arc::new(InMemoryTenantPolicyEngine::new(TenantPolicy {
        audit_export: Some(AuditExportPolicy {
            enabled: true,
            allowed_destinations: vec!["siem".into()],
        }),
        ..TenantPolicy::default()
    }));
    engine.set_policy(
        TenantId::new("tenant-b"),
        TenantPolicy {
            permission_profile: Some(PermissionProfile {
                default_role: Some(PrincipalRole::Admin),
                ..PermissionProfile::default()
            }),
            audit_export: Some(AuditExportPolicy {
                enabled: true,
                allowed_destinations: vec!["siem".into()],
            }),
            ..TenantPolicy::default()
        },
    );
    let service = AppService::with_tenant_policy("tenant-audit-export", engine);
    let tenant_a = IdentityContext::local();
    let tenant_b = IdentityContext::new(TenantId::new("tenant-b"), PrincipalId::new("ops:admin"));
    service.tenant_policy().record_control_event(
        &tenant_a,
        AuditAction::LeaseRebound,
        AuditTargetKind::Lease,
        AuditDecision::Observe,
        "lease_rebound",
        AuditDimensions::default(),
        3,
    );
    service.tenant_policy().record_control_event(
        &tenant_b,
        AuditAction::LeaseRebound,
        AuditTargetKind::Lease,
        AuditDecision::Observe,
        "lease_rebound",
        AuditDimensions::default(),
        3,
    );

    let a_events = service
        .canonical_audit_events(&tenant_a)
        .expect("tenant a admin may read");
    let b_events = service
        .canonical_audit_events(&tenant_b)
        .expect("tenant b admin may read");
    assert!(a_events
        .iter()
        .all(|event| event.tenant_id == tenant_a.tenant_id));
    assert!(b_events
        .iter()
        .all(|event| event.tenant_id == tenant_b.tenant_id));
    assert!(a_events
        .iter()
        .any(|event| event.action == AuditAction::LeaseRebound));
    assert!(b_events
        .iter()
        .any(|event| event.action == AuditAction::LeaseRebound));
    assert!(!a_events
        .iter()
        .any(|event| event.tenant_id == tenant_b.tenant_id));

    let exporter = InMemoryOtelExporter::default();
    let exported = service
        .export_canonical_audit(&tenant_a, "siem", &exporter)
        .expect("admin may export");
    // check_audit_export records one PolicyEvaluated event before the allowlist dump.
    assert_eq!(exported, a_events.len() + 1);
    let json = serde_json::to_string(&exporter.snapshot()).unwrap();
    for forbidden in [
        "prompt",
        "tool_output",
        "secret_ref",
        "protected_blob",
        "api_key",
    ] {
        assert!(!json.contains(forbidden), "export leaked {forbidden}");
    }
    assert!(exporter
        .snapshot()
        .iter()
        .all(|record| record.attributes.get("tenant_id").unwrap() == "local/default"));

    let after = service
        .canonical_audit_events(&tenant_a)
        .expect("admin may read after export");
    assert!(after.iter().any(|event| {
        event.action == AuditAction::AuditExported && event.reason_code == "audit_export_completed"
    }));
}

// ---------- P18-9 主审回归：闸口边界 / 生产 RunStart / lease / 预算 ----------

static NEXT_ID: AtomicU64 = AtomicU64::new(0);

fn unique(prefix: &str) -> String {
    format!("{prefix}-{}", NEXT_ID.fetch_add(1, Ordering::SeqCst))
}

fn local_tenant() -> TenantId {
    tenant_service::IdentityContext::local().tenant_id
}

fn command(
    source: CommandSource,
    identity: ActorIdentity,
    command: AppCommand,
) -> AppCommandEnvelope {
    AppCommandEnvelope {
        api_version: API_VERSION,
        command_id: CommandId::from(unique("policy-cmd")),
        source,
        identity,
        expected_revision: None,
        idempotency_key: None,
        issued_at: Timestamp::from_unix_millis(1),
        command,
    }
}

fn temp_workspace_dir() -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("pawork-tenant-policy-{}", unique("ws")));
    std::fs::create_dir_all(&path).expect("create temp workspace dir");
    path
}

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
            title: Some("policy".into()),
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

fn run_start(session_id: &SessionId, message: &str) -> AppCommandEnvelope {
    command(
        cli_source(),
        cli_identity(),
        AppCommand::RunStart {
            session_id: session_id.clone(),
            user_message: message.into(),
            model: None,
            profile: None,
        },
    )
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

fn deny_count(
    engine: &Arc<InMemoryTenantPolicyEngine>,
    tenant: &TenantId,
    gate: PolicyGate,
) -> usize {
    engine
        .decisions(tenant)
        .iter()
        .filter(|event| event.gate == gate && event.decision == PolicyDecisionKind::Deny)
        .count()
}

fn user_role() -> TenantPolicy {
    TenantPolicy {
        permission_profile: Some(PermissionProfile {
            default_role: Some(PrincipalRole::User),
            ..PermissionProfile::default()
        }),
        ..TenantPolicy::default()
    }
}

fn mock_provider() -> Arc<test_support::MockProvider> {
    Arc::new(
        test_support::MockProvider::new(test_support::MockScript::new().complete())
            .with_id(ProviderId::from("mock")),
    )
}

fn blocking_mock_provider() -> Arc<test_support::MockProvider> {
    Arc::new(
        test_support::MockProvider::new(test_support::MockScript::new().wait_for_cancellation())
            .with_id(ProviderId::from("mock")),
    )
}

/// 主审项：AppService 门面与 router() 公开入口共享同一个策略闸口。
/// 绕过门面直接调 `router().dispatch_query` 仍被拒绝，且单次调用只记一条
/// versioned 决策事件（不双记）。
#[tokio::test]
async fn facade_and_router_share_one_query_gate_without_double_record() {
    #[derive(Clone)]
    struct TenantAResolver;

    impl tenant_service::IdentityResolver for TenantAResolver {
        fn resolve(
            &self,
            principal: Option<&str>,
        ) -> Result<IdentityContext, tenant_service::IdentityError> {
            match principal {
                Some("authenticated_client:tenant-a") => Ok(IdentityContext::new(
                    TenantId::new("tenant-a"),
                    PrincipalId::new("principal-a"),
                )),
                Some(value) if !value.trim().is_empty() => Ok(IdentityContext::local()),
                _ => Err(tenant_service::IdentityError::MissingIdentity(
                    "no principal".into(),
                )),
            }
        }
    }

    let engine = Arc::new(InMemoryTenantPolicyEngine::new(TenantPolicy::default()));
    engine.set_policy(
        TenantId::new("tenant-a"),
        TenantPolicy {
            permission_profile: Some(PermissionProfile {
                default_role: Some(PrincipalRole::Service),
                ..PermissionProfile::default()
            }),
            ..TenantPolicy::default()
        },
    );
    let service = AppService::with_identity_resolver_and_tenant_policy(
        "tenant-policy-bypass",
        Arc::new(TenantAResolver),
        engine.clone(),
    );
    let envelope = query(
        gui_source(),
        ActorIdentity::AuthenticatedClient {
            actor_id: ActorId::from("actor-a"),
            subject: "tenant-a".into(),
        },
        AppQuery::SnapshotFetch,
    );

    let via_facade = service.dispatch_query(envelope.clone());
    assert_authorization(&via_facade, Some("SessionRead"));
    assert_eq!(
        deny_count(
            &engine,
            &TenantId::new("tenant-a"),
            PolicyGate::SessionQuery
        ),
        1,
        "facade 单次调用必须只记一条决策事件"
    );

    // 直接经 router() 公开入口：同一闸口、同一引擎，无法绕过。
    let via_router = service.router().dispatch_query(envelope);
    assert_authorization(&via_router, None);
    assert_eq!(
        deny_count(
            &engine,
            &TenantId::new("tenant-a"),
            PolicyGate::SessionQuery
        ),
        2,
        "router 公开入口走同一闸口，逐次只记一条"
    );
}

/// 生产 RunStart 真实调用链：AgentSpawn / model / provider 白名单都在
/// provider 调用前 fail-closed；放行后 run 才真实执行。
#[tokio::test]
async fn run_start_fails_closed_on_role_model_and_provider_before_provider_call() {
    let engine = Arc::new(InMemoryTenantPolicyEngine::new(TenantPolicy {
        permission_profile: Some(PermissionProfile {
            default_role: Some(PrincipalRole::Viewer),
            ..PermissionProfile::default()
        }),
        ..TenantPolicy::default()
    }));
    let router = CommandRouter::with_tenant_policy(
        RouterConfig::default(),
        Arc::new(LocalIdentityResolver),
        engine.clone(),
    );
    let provider = mock_provider();
    router.register_provider(provider.clone());
    let session_id = prepare_session(&router);

    // Viewer：AgentSpawn 拒绝，不建 run、不调用 provider。
    let denied = router.dispatch(run_start(&session_id, "hello"));
    assert_authorization(&denied, Some("AgentSpawn"));
    assert_eq!(router.supervisor().total(), 0);
    assert_eq!(router.aggregate().runs().len(), 0);
    assert!(provider.calls().is_empty());

    // User + model 白名单不命中：provider 调用前拒绝。
    engine.set_policy(
        local_tenant(),
        TenantPolicy {
            allowed_models: Some(vec![ModelId::new("blocked-model")]),
            ..user_role()
        },
    );
    let denied = router.dispatch(run_start(&session_id, "hello"));
    assert_authorization(&denied, Some("模型"));
    assert_eq!(router.supervisor().total(), 0);

    // User + provider 白名单不命中：provider 调用前拒绝。
    engine.set_policy(
        local_tenant(),
        TenantPolicy {
            allowed_providers: Some(vec![ProviderId::new("anthropic")]),
            ..user_role()
        },
    );
    let denied = router.dispatch(run_start(&session_id, "hello"));
    assert_authorization(&denied, Some("Provider"));
    assert_eq!(router.supervisor().total(), 0);
    assert!(provider.calls().is_empty());

    // 放行：User、白名单不限制 → Accepted，run 真实完成（provider 被调用）。
    engine.set_policy(local_tenant(), user_role());
    let accepted = router.dispatch(run_start(&session_id, "hello"));
    assert!(
        matches!(accepted.response, AppResponse::Accepted { .. }),
        "{:?}",
        accepted.response
    );
    let run_id = router.last_started_run().expect("run id");
    let completed = wait_until(
        || {
            router
                .aggregate()
                .get_run(&run_id, &local_tenant())
                .is_some_and(|run| run.state == RunState::Completed)
        },
        Duration::from_secs(5),
    )
    .await;
    assert!(completed, "放行后 run 应真实完成");
    assert!(!provider.calls().is_empty(), "放行后应实际调用 provider");
    assert_eq!(router.supervisor().total(), 1);
}

/// 主审项：租户请求并发在 dispatch 边界强制，`used == limit` 即拒绝。
#[tokio::test]
async fn run_start_enforces_request_concurrency_limit_at_boundary() {
    let engine = Arc::new(InMemoryTenantPolicyEngine::new(TenantPolicy {
        max_concurrent_requests: Some(0),
        ..user_role()
    }));
    let router = CommandRouter::with_tenant_policy(
        RouterConfig::default(),
        Arc::new(LocalIdentityResolver),
        engine.clone(),
    );
    let provider = mock_provider();
    router.register_provider(provider.clone());
    let session_id = prepare_session(&router);

    // 当前 0 个活动请求 >= 上限 0：used == limit 即拒绝。
    let denied = router.dispatch(run_start(&session_id, "hello"));
    assert_authorization(&denied, Some("请求并发"));
    assert_eq!(router.supervisor().total(), 0);
    assert_eq!(router.aggregate().runs().len(), 0);
    assert!(provider.calls().is_empty());
    assert!(deny_count(&engine, &local_tenant(), PolicyGate::RequestAdmission) >= 1);
}

/// 并发准入与任务登记必须是一个原子步骤：limit=1 时两个同时到达的
/// RunStart 恰好一个成功，另一个在 provider 调用前拒绝。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_run_start_has_exactly_one_winner_at_request_limit_one() {
    let engine = Arc::new(InMemoryTenantPolicyEngine::new(TenantPolicy {
        max_concurrent_requests: Some(1),
        ..user_role()
    }));
    let router = Arc::new(CommandRouter::with_tenant_policy(
        RouterConfig::default(),
        Arc::new(LocalIdentityResolver),
        engine,
    ));
    let provider = blocking_mock_provider();
    router.register_provider(provider.clone());
    let session_id = prepare_session(&router);
    let barrier = Arc::new(tokio::sync::Barrier::new(3));

    let mut tasks = Vec::new();
    for message in ["first", "second"] {
        let router = Arc::clone(&router);
        let session_id = session_id.clone();
        let barrier = Arc::clone(&barrier);
        tasks.push(tokio::spawn(async move {
            barrier.wait().await;
            router.dispatch(run_start(&session_id, message))
        }));
    }
    barrier.wait().await;
    let mut accepted = 0;
    let mut denied = 0;
    for task in tasks {
        match task.await.expect("dispatch task").response {
            AppResponse::Accepted { .. } => accepted += 1,
            AppResponse::Error(context) if context.category == ErrorCategory::Authorization => {
                assert!(context.message.contains("请求并发"), "{context:?}");
                denied += 1;
            }
            other => panic!("unexpected concurrent RunStart response: {other:?}"),
        }
    }
    assert_eq!((accepted, denied), (1, 1));
    assert!(
        wait_until(|| provider.calls().len() == 1, Duration::from_secs(5)).await,
        "the admitted run must reach the provider exactly once"
    );

    if let Some(run_id) = router.last_started_run() {
        let _ = router.dispatch(command(
            cli_source(),
            cli_identity(),
            AppCommand::RunCancel { run_id },
        ));
    }
}

/// Agent spawn 并发在 dispatch 边界强制，`used == limit` 即拒绝；原因必须
/// 可与请求并发区分，并记到 `PolicyGate::AgentSpawn`。
#[tokio::test]
async fn run_start_enforces_agent_concurrency_limit_at_boundary() {
    let engine = Arc::new(InMemoryTenantPolicyEngine::new(TenantPolicy {
        max_concurrent_agents: Some(0),
        ..user_role()
    }));
    let router = CommandRouter::with_tenant_policy(
        RouterConfig::default(),
        Arc::new(LocalIdentityResolver),
        engine.clone(),
    );
    let provider = mock_provider();
    router.register_provider(provider.clone());
    let session_id = prepare_session(&router);

    let denied = router.dispatch(run_start(&session_id, "hello"));
    assert_authorization(&denied, Some("agent 并发"));
    if let AppResponse::Error(context) = &denied.response {
        assert!(
            !context.message.contains("请求并发"),
            "agent 并发拒绝不得被记成请求并发: {:?}",
            context.message
        );
    }
    assert_eq!(router.supervisor().total(), 0);
    assert_eq!(router.aggregate().runs().len(), 0);
    assert!(provider.calls().is_empty());
    assert!(deny_count(&engine, &local_tenant(), PolicyGate::AgentSpawn) >= 1);
}

/// 并发准入与任务登记必须是一个原子步骤：agent limit=2 时 8 个同时到达的
/// RunStart 恰好两个成功，其余在 provider 调用前拒绝。
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn concurrent_run_start_has_exactly_two_winners_at_agent_limit_two() {
    let engine = Arc::new(InMemoryTenantPolicyEngine::new(TenantPolicy {
        max_concurrent_agents: Some(2),
        ..user_role()
    }));
    let router = Arc::new(CommandRouter::with_tenant_policy(
        RouterConfig::default(),
        Arc::new(LocalIdentityResolver),
        engine,
    ));
    let provider = blocking_mock_provider();
    router.register_provider(provider.clone());
    let session_id = prepare_session(&router);
    let barrier = Arc::new(tokio::sync::Barrier::new(9));

    let mut tasks = Vec::new();
    for index in 0..8 {
        let router = Arc::clone(&router);
        let session_id = session_id.clone();
        let barrier = Arc::clone(&barrier);
        tasks.push(tokio::spawn(async move {
            barrier.wait().await;
            router.dispatch(run_start(&session_id, &format!("run-{index}")))
        }));
    }
    barrier.wait().await;
    let mut accepted = 0;
    let mut denied = 0;
    let mut accepted_runs = Vec::new();
    for task in tasks {
        match task.await.expect("dispatch task").response {
            AppResponse::Accepted {
                run_id: Some(run_id),
                ..
            } => {
                accepted += 1;
                accepted_runs.push(run_id);
            }
            AppResponse::Error(context) if context.category == ErrorCategory::Authorization => {
                assert!(context.message.contains("agent 并发"), "{context:?}");
                denied += 1;
            }
            other => panic!("unexpected concurrent RunStart response: {other:?}"),
        }
    }
    assert_eq!((accepted, denied), (2, 6));
    assert!(
        router.supervisor().total() <= 2,
        "supervisor total must never exceed agent limit"
    );
    assert!(
        router.supervisor().stats().active <= 2,
        "supervisor active must never exceed agent limit"
    );
    assert!(
        router.supervisor().active_for_tenant(&local_tenant()) <= 2,
        "tenant occupancy must never exceed agent limit"
    );
    assert!(
        wait_until(|| provider.calls().len() == 2, Duration::from_secs(5)).await,
        "the admitted runs must reach the provider exactly twice"
    );
    assert_eq!(provider.calls().len(), 2);

    for run_id in accepted_runs {
        let _ = router.dispatch(command(
            cli_source(),
            cli_identity(),
            AppCommand::RunCancel { run_id },
        ));
    }
}

/// Retry 是新的 provider attempt，必须按当前策略重新准入，不能沿用首跑时
/// 已过期的 AgentSpawn / model / provider 许可。
#[tokio::test]
async fn retry_rechecks_current_tenant_policy_before_provider_call() {
    let engine = Arc::new(InMemoryTenantPolicyEngine::new(user_role()));
    let router = CommandRouter::with_tenant_policy(
        RouterConfig::default(),
        Arc::new(LocalIdentityResolver),
        engine.clone(),
    );
    let provider = blocking_mock_provider();
    router.register_provider(provider.clone());
    let session_id = prepare_session(&router);
    let started = router.dispatch(run_start(&session_id, "initial"));
    let run_id = match started.response {
        AppResponse::Accepted {
            run_id: Some(run_id),
            ..
        } => run_id,
        other => panic!("expected accepted run, got {other:?}"),
    };
    assert!(wait_until(|| !provider.calls().is_empty(), Duration::from_secs(5)).await);
    let _ = router.dispatch(command(
        cli_source(),
        cli_identity(),
        AppCommand::RunCancel {
            run_id: run_id.clone(),
        },
    ));
    assert!(
        wait_until(
            || router.supervisor().stats().cancelled >= 1,
            Duration::from_secs(5),
        )
        .await
    );

    engine.set_policy(
        local_tenant(),
        TenantPolicy {
            permission_profile: Some(PermissionProfile {
                default_role: Some(PrincipalRole::Viewer),
                ..PermissionProfile::default()
            }),
            ..TenantPolicy::default()
        },
    );
    let retry = router.dispatch(command(
        cli_source(),
        cli_identity(),
        AppCommand::RunRetry { run_id },
    ));
    assert_authorization(&retry, Some("AgentSpawn"));
    assert_eq!(
        provider.calls().len(),
        1,
        "denied retry must not call provider"
    );
}

#[test]
fn tenant_policy_view_preserves_unrestricted_vs_deny_all_whitelists() {
    let engine = Arc::new(InMemoryTenantPolicyEngine::new(TenantPolicy::default()));
    let service = AppService::with_tenant_policy("policy-view-options", engine.clone());
    let identity = IdentityContext::local();
    let unrestricted = service
        .tenant_policy_view(&identity, &local_tenant())
        .expect("admin may read local policy");
    assert_eq!(unrestricted.allowed_providers, None);
    assert_eq!(unrestricted.allowed_models, None);
    assert_eq!(unrestricted.allowed_accounts, None);

    engine.set_policy(
        local_tenant(),
        TenantPolicy {
            allowed_providers: Some(vec![]),
            allowed_models: Some(vec![]),
            allowed_accounts: Some(vec![]),
            ..TenantPolicy::default()
        },
    );
    let deny_all = service
        .tenant_policy_view(&identity, &local_tenant())
        .expect("admin may read updated local policy");
    assert_eq!(deny_all.allowed_providers, Some(vec![]));
    assert_eq!(deny_all.allowed_models, Some(vec![]));
    assert_eq!(deny_all.allowed_accounts, Some(vec![]));
}

/// 主审项：lease 取得后强制 LeaseAcquire / account 白名单；拒绝时释放
/// lease、run fail-closed、provider 从不被调用。
#[tokio::test]
async fn lease_account_whitelist_denies_after_acquire_and_releases_lease() {
    let engine = Arc::new(InMemoryTenantPolicyEngine::new(TenantPolicy {
        allowed_accounts: Some(vec![AccountId::new("other-account")]),
        ..user_role()
    }));
    let router = CommandRouter::with_tenant_policy(
        RouterConfig::default(),
        Arc::new(LocalIdentityResolver),
        engine.clone(),
    );
    let provider = mock_provider();
    router.register_provider(provider.clone());
    let pool = Arc::new(InMemoryCredentialPool::new(4));
    router.set_credential_pool(pool.clone());
    let session_id = prepare_session(&router);

    // RunStart 本身放行（AgentSpawn OK）；lease 取得后 account 白名单拒绝。
    let accepted = router.dispatch(run_start(&session_id, "hello"));
    assert!(
        matches!(accepted.response, AppResponse::Accepted { .. }),
        "{:?}",
        accepted.response
    );
    let run_id = router.last_started_run().expect("run id");
    let failed = wait_until(
        || {
            router
                .aggregate()
                .get_run(&run_id, &local_tenant())
                .is_some_and(|run| run.state == RunState::Failed)
        },
        Duration::from_secs(5),
    )
    .await;
    assert!(failed, "account 白名单拒绝后 run 应 Failed");
    assert!(provider.calls().is_empty(), "拒绝路径不得调用 provider");
    assert_eq!(
        pool.active_count(&AccountId::new("local/default")),
        0,
        "拒绝后必须释放 lease"
    );
    let lease_denies: Vec<_> = engine
        .decisions(&local_tenant())
        .into_iter()
        .filter(|event| {
            event.gate == PolicyGate::LeaseAcquire && event.decision == PolicyDecisionKind::Deny
        })
        .collect();
    assert!(!lease_denies.is_empty(), "必须记录 LeaseAcquire deny 事件");
    assert!(lease_denies
        .iter()
        .any(|event| event.reason.contains("账号")));
}

/// 主审项：预算 admission 使用唯一共享 UsageLedger；`used >= limit` 拒绝、
/// 释放 lease、provider 从不被调用。
#[tokio::test]
async fn budget_admission_uses_shared_ledger_rejects_at_limit_and_releases_lease() {
    let engine = Arc::new(InMemoryTenantPolicyEngine::new(TenantPolicy {
        daily_input_token_budget: Some(0),
        ..user_role()
    }));
    let router = CommandRouter::with_tenant_policy(
        RouterConfig::default(),
        Arc::new(LocalIdentityResolver),
        engine.clone(),
    );
    let provider = mock_provider();
    router.register_provider(provider.clone());
    let ledger: Arc<dyn UsageLedger> = Arc::new(InMemoryUsageLedger::default());
    let clock: Arc<dyn quota_service::service::QuotaClock> = Arc::new(MutableQuotaClock::at(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock")
            .as_millis() as u64,
    ));
    router.set_quota_runtime(QuotaRuntime::new(ledger, clock));
    let pool = Arc::new(InMemoryCredentialPool::new(4));
    router.set_credential_pool(pool.clone());
    let session_id = prepare_session(&router);

    // used(0) >= limit(0)：预算 admission 在 lease 取得后拒绝，释放 lease。
    let accepted = router.dispatch(run_start(&session_id, "hello"));
    assert!(
        matches!(accepted.response, AppResponse::Accepted { .. }),
        "{:?}",
        accepted.response
    );
    let run_id = router.last_started_run().expect("run id");
    let failed = wait_until(
        || {
            router
                .aggregate()
                .get_run(&run_id, &local_tenant())
                .is_some_and(|run| run.state == RunState::Failed)
        },
        Duration::from_secs(5),
    )
    .await;
    assert!(failed, "used == limit 应拒绝且 run Failed");
    assert!(provider.calls().is_empty(), "预算拒绝路径不得调用 provider");
    assert_eq!(
        pool.active_count(&AccountId::new("local/default")),
        0,
        "预算拒绝后必须释放 lease"
    );
    let admission_denies: Vec<_> = engine
        .decisions(&local_tenant())
        .into_iter()
        .filter(|event| {
            event.gate == PolicyGate::RequestAdmission && event.decision == PolicyDecisionKind::Deny
        })
        .collect();
    assert!(
        !admission_denies.is_empty(),
        "必须记录 RequestAdmission deny 事件"
    );
    assert!(admission_denies
        .iter()
        .any(|event| event.reason.contains("预算")));
}
