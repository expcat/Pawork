//! P18-9：租户策略闸口（tenant / role 强制入口）。
//!
//! [`TenantPolicyGate`] 是 app-service 层唯一的策略裁决入口：
//! - query 强制：Session / Usage / Audit 查询与 Audit 导出在 facade 层执行
//!   角色权限与同租户作用域检查（deny-first，任何 adapter / GUI / plugin
//!   都无法覆盖 Core policy）；
//! - 决策审计：所有裁决经 tenant-service 记录 versioned、脱敏的决策事件；
//! - [`RoutingTenantPolicyAdapter`] 复用 provider-control 既有的
//!   [`provider_control::routing::TenantPolicy`] 注入接口，不复制候选过滤链。
//!
//! 本模块不持有 Secret、不做网络 / 数据库访问；引擎由 tenant-service 提供。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use agent_domain::{AccountId, ModelId, ProviderId, TenantId};
use audit_log::{
    AuditAction, AuditDecision, AuditDimensions, AuditEventV1, AuditExporter, AuditSink,
    AuditStore, AuditTargetKind, InMemoryAuditStore,
};
use core_api::{
    AuditExportPolicyView, PermissionProfileView, PolicyDecisionEventView,
    PolicyDecisionKind as ApiDecisionKind, PolicyGate as ApiPolicyGate,
    PrincipalRole as ApiPrincipalRole, PrincipalRoleBinding, TenantPolicyView,
};
use provider_control::routing::{PolicyDenial, RouteCandidate, RouteContext};
use tenant_service::{
    decide_account, decide_audit_export, decide_model, decide_permission, decide_provider,
    decide_retention, IdentityContext, Permission, PolicyDecision, PolicyDecisionEvent,
    PolicyDecisionKind, PolicyGate, PrincipalId, PrincipalRole, TenantPolicyEngine,
    TenantPolicyError,
};

/// 闸口层错误：跨租户访问与策略拒绝（调用方归一为 `AppServiceError`）。
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum PolicyGateError {
    /// 策略拒绝（含角色权限 / 白名单 / 导出策略）。
    #[error("tenant policy denied: {0}")]
    Denied(String),
    /// 跨租户访问（Tenant A 不得观察 Tenant B 的任何数据）。
    #[error("cross-tenant access denied: {requester} -> {target}")]
    CrossTenant {
        /// 请求者租户。
        requester: String,
        /// 目标租户。
        target: String,
    },
}

/// 租户策略闸口：同步裁决 + 版本化决策记录。
pub struct TenantPolicyGate {
    engine: Arc<dyn TenantPolicyEngine>,
    canonical_audit: Arc<InMemoryAuditStore>,
    audit_sinks: Mutex<Vec<Arc<dyn AuditSink>>>,
    audit_sequence: AtomicU64,
}

impl TenantPolicyGate {
    /// 以注入的引擎构造闸口。
    pub fn new(engine: Arc<dyn TenantPolicyEngine>) -> Self {
        Self {
            engine,
            canonical_audit: Arc::new(InMemoryAuditStore::default()),
            audit_sinks: Mutex::new(Vec::new()),
            audit_sequence: AtomicU64::new(0),
        }
    }

    /// 底层引擎（宿主接入 routing / 管理接口时使用）。
    pub fn engine(&self) -> &Arc<dyn TenantPolicyEngine> {
        &self.engine
    }

    /// Adds a durable or external sink. The built-in tenant-scoped projection remains active,
    /// so attaching an exporter cannot disable local forensic queries.
    pub fn add_audit_sink(&self, sink: Arc<dyn AuditSink>) {
        let mut sinks = self
            .audit_sinks
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !sinks.iter().any(|existing| Arc::ptr_eq(existing, &sink)) {
            sinks.push(sink);
        }
    }

    /// Returns canonical events for exactly one tenant. Callers must perform RBAC first.
    pub fn canonical_audit_events(
        &self,
        tenant: &TenantId,
    ) -> Result<Vec<AuditEventV1>, audit_log::AuditError> {
        self.canonical_audit.query_tenant(tenant)
    }

    /// Exports exactly one tenant through the structural allowlist.
    pub fn export_canonical_audit(
        &self,
        tenant: &TenantId,
        exporter: &dyn AuditExporter,
    ) -> Result<usize, audit_log::AuditError> {
        audit_log::export_tenant(self.canonical_audit.as_ref(), tenant, exporter)
    }

    /// Records a canonical event into the local projection and every attached sink. Sink
    /// failures are diagnosed without changing the already-made policy decision; the local
    /// projection is always attempted first and event ids are unique within this host process.
    #[allow(clippy::too_many_arguments)]
    pub fn record_control_event(
        &self,
        identity: &IdentityContext,
        action: AuditAction,
        target_kind: AuditTargetKind,
        decision: AuditDecision,
        reason_code: &'static str,
        dimensions: AuditDimensions,
        decision_version: u64,
    ) {
        let at_ms = now_ms();
        let sequence = self.audit_sequence.fetch_add(1, Ordering::SeqCst);
        let event = AuditEventV1::new(
            agent_domain::EventId::new(format!("audit-{}-{at_ms}-{sequence}", std::process::id())),
            agent_domain::Timestamp::from_unix_millis(at_ms),
            identity.tenant_id.clone(),
            identity.principal_id.clone(),
            action,
            target_kind,
            decision,
            reason_code,
            decision_version,
        )
        .with_dimensions(dimensions);
        if let Err(error) = self.canonical_audit.append(event.clone()) {
            tracing::error!(error = %error, "canonical audit projection append failed");
        }
        let sinks = self
            .audit_sinks
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        for sink in sinks {
            if let Err(error) = sink.append(event.clone()) {
                tracing::error!(error = %error, "external canonical audit sink append failed");
            }
        }
    }

    /// 主体在该租户的有效角色。
    pub fn role(&self, tenant: &TenantId, principal: &PrincipalId) -> PrincipalRole {
        self.engine.principal_role(tenant, principal)
    }

    /// 同步权限裁决（deny-first；供同步 query facade 使用）。
    pub fn check_permission(
        &self,
        identity: &IdentityContext,
        permission: Permission,
    ) -> Result<(), TenantPolicyError> {
        let role = self.role(&identity.tenant_id, &identity.principal_id);
        match decide_permission(role, permission) {
            PolicyDecision::Allow => Ok(()),
            _ => Err(TenantPolicyError::PermissionDenied {
                principal: identity.principal_id.to_string(),
                permission,
            }),
        }
    }

    /// 查询作用域授权：目标租户必须等于请求者租户（跨租户观察一律拒绝）。
    pub fn authorize_scope(
        &self,
        requester: &IdentityContext,
        target: &TenantId,
    ) -> Result<(), PolicyGateError> {
        if requester.tenant_id == *target {
            Ok(())
        } else {
            Err(PolicyGateError::CrossTenant {
                requester: requester.tenant_id.to_string(),
                target: target.to_string(),
            })
        }
    }

    /// Provider 白名单裁决（同步）。
    pub fn check_provider(
        &self,
        tenant: &TenantId,
        provider: &ProviderId,
    ) -> Result<(), TenantPolicyError> {
        match decide_provider(
            provider,
            self.engine.policy(tenant).allowed_providers.as_deref(),
        ) {
            PolicyDecision::Allow => Ok(()),
            _ => Err(TenantPolicyError::ProviderNotAllowed {
                provider: provider.to_string(),
            }),
        }
    }

    /// 账号白名单裁决（同步）。
    pub fn check_account(
        &self,
        tenant: &TenantId,
        account: &AccountId,
    ) -> Result<(), TenantPolicyError> {
        match decide_account(
            account,
            self.engine.policy(tenant).allowed_accounts.as_deref(),
        ) {
            PolicyDecision::Allow => Ok(()),
            _ => Err(TenantPolicyError::AccountNotAllowed {
                account: account.to_string(),
            }),
        }
    }

    /// 模型白名单裁决（同步）。
    pub fn check_model(&self, tenant: &TenantId, model: &ModelId) -> Result<(), TenantPolicyError> {
        match decide_model(model, self.engine.policy(tenant).allowed_models.as_deref()) {
            PolicyDecision::Allow => Ok(()),
            _ => Err(TenantPolicyError::ModelNotAllowed {
                model: model.to_string(),
            }),
        }
    }

    /// Audit 导出裁决（角色 + 导出策略 + 目标白名单，deny-first）。
    pub fn check_audit_export(
        &self,
        identity: &IdentityContext,
        destination: &str,
    ) -> Result<(), TenantPolicyError> {
        let role = self.role(&identity.tenant_id, &identity.principal_id);
        let policy = self.engine.policy(&identity.tenant_id);
        match decide_audit_export(role, policy.audit_export.as_ref(), destination) {
            PolicyDecision::Allow => Ok(()),
            PolicyDecision::Deny { reason } => Err(TenantPolicyError::AuditExportDenied { reason }),
            PolicyDecision::Limit { reason } | PolicyDecision::Fallback { reason } => {
                Err(TenantPolicyError::AuditExportDenied { reason })
            }
        }
    }

    /// 保留期裁决（`None` 永久保留；超期返回 `Limit`，允许按保留期修剪）。
    pub fn retention_decision(&self, tenant: &TenantId, record_age_days: u64) -> PolicyDecision {
        decide_retention(record_age_days, self.engine.policy(tenant).retention_days)
    }

    /// 读取指定租户的策略视图（供管理接口输出）。
    pub fn policy_view(&self, tenant: &TenantId) -> TenantPolicyView {
        let policy = self.engine.policy(tenant);
        TenantPolicyView {
            tenant_id: tenant.clone(),
            version: self.engine.policy_version(tenant),
            max_concurrent_agents: policy.max_concurrent_agents,
            max_concurrent_requests: policy.max_concurrent_requests,
            daily_input_token_budget: policy.daily_input_token_budget,
            daily_output_token_budget: policy.daily_output_token_budget,
            daily_cost_micros_budget: policy.daily_cost_micros_budget,
            allowed_providers: policy.allowed_providers,
            allowed_models: policy.allowed_models,
            allowed_accounts: policy.allowed_accounts,
            permission_profile: policy
                .permission_profile
                .map(|profile| PermissionProfileView {
                    default_role: profile.default_role.map(to_api_role),
                    principal_roles: profile
                        .principal_roles
                        .into_iter()
                        .map(|(principal_id, role)| PrincipalRoleBinding {
                            principal_id: principal_id.to_string(),
                            role: to_api_role(role),
                        })
                        .collect(),
                }),
            retention_days: policy.retention_days,
            audit_export: policy.audit_export.map(|export| AuditExportPolicyView {
                enabled: export.enabled,
                allowed_destinations: export.allowed_destinations,
            }),
        }
    }

    /// 审计读取：要求请求者 `AuditRead` 权限，且只返回其租户自己的
    /// versioned、脱敏决策事件（跨租户观察一律拒绝）。
    pub fn query_decision_events(
        &self,
        requester: &IdentityContext,
    ) -> Result<Vec<PolicyDecisionEventView>, PolicyGateError> {
        if let Err(error) = self.check_permission(requester, Permission::AuditRead) {
            self.record_decision(
                requester,
                PolicyGate::AuditQuery,
                PolicyDecisionKind::Deny,
                error.to_string(),
            );
            return Err(PolicyGateError::Denied(error.to_string()));
        }
        self.record_decision(
            requester,
            PolicyGate::AuditQuery,
            PolicyDecisionKind::Allow,
            "audit 查询放行",
        );
        Ok(self
            .engine
            .decisions(&requester.tenant_id)
            .into_iter()
            .map(decision_event_to_view)
            .collect())
    }

    /// 记录一条版本化决策事件（reason 在 tenant-service 构造时脱敏）。
    pub fn record_decision(
        &self,
        identity: &IdentityContext,
        gate: PolicyGate,
        decision: PolicyDecisionKind,
        reason: impl Into<String>,
    ) {
        self.record_decision_scoped(identity, gate, decision, reason, AuditDimensions::default());
    }

    /// Records the tenant-service decision and its canonical audit projection with correlated
    /// provider/account/session/agent/client/trace identifiers.
    pub fn record_decision_scoped(
        &self,
        identity: &IdentityContext,
        gate: PolicyGate,
        decision: PolicyDecisionKind,
        reason: impl Into<String>,
        dimensions: AuditDimensions,
    ) {
        let policy_version = self.engine.policy_version(&identity.tenant_id);
        self.engine.record_decision(PolicyDecisionEvent::new(
            policy_version,
            identity,
            gate,
            decision,
            reason,
            now_ms(),
        ));
        let (action, target_kind) = match gate {
            PolicyGate::RouteCandidate => (AuditAction::RouteEvaluated, AuditTargetKind::Route),
            PolicyGate::LeaseAcquire => (AuditAction::LeaseAcquired, AuditTargetKind::Lease),
            _ => (AuditAction::PolicyEvaluated, AuditTargetKind::Policy),
        };
        self.record_control_event(
            identity,
            action,
            target_kind,
            to_audit_decision(decision),
            policy_reason_code(gate, decision),
            dimensions,
            policy_version,
        );
    }
}

/// 复用 provider-control 既有 routing `TenantPolicy` 注入接口的适配器
/// （P18-6 契约，P18-9 完整 RBAC 接线）：不复制候选过滤链，只实现租户
/// policy 的 `allows` 裁决。
pub struct RoutingTenantPolicyAdapter {
    gate: Arc<TenantPolicyGate>,
}

impl RoutingTenantPolicyAdapter {
    /// 以闸口构造。
    pub fn new(gate: Arc<TenantPolicyGate>) -> Self {
        Self { gate }
    }

    /// 以引擎构造（等价于 `Self::new(Arc::new(TenantPolicyGate::new(engine)))`）。
    pub fn from_engine(engine: Arc<dyn TenantPolicyEngine>) -> Self {
        Self::new(Arc::new(TenantPolicyGate::new(engine)))
    }
}

impl provider_control::routing::TenantPolicy for RoutingTenantPolicyAdapter {
    /// deny-first：角色 RouteCandidate → provider → account → model，任一
    /// 拒绝即拒绝候选并记录 versioned 决策事件；adapter / GUI / plugin
    /// 无法覆盖（本裁决在 Core 过滤链内执行）。
    fn allows(
        &self,
        context: &RouteContext,
        candidate: &RouteCandidate,
    ) -> Result<(), PolicyDenial> {
        let identity =
            IdentityContext::new(context.tenant_id.clone(), context.principal_id.clone());
        let deny = |reason: String| {
            self.gate.record_decision_scoped(
                &identity,
                PolicyGate::RouteCandidate,
                PolicyDecisionKind::Deny,
                reason.clone(),
                AuditDimensions {
                    provider_id: Some(candidate.provider_id.clone()),
                    account_id: Some(candidate.account_id.clone()),
                    ..AuditDimensions::default()
                },
            );
            PolicyDenial { reason }
        };

        if let Err(error) = self
            .gate
            .check_permission(&identity, Permission::RouteCandidate)
        {
            return Err(deny(error.to_string()));
        }
        if let Err(error) = self
            .gate
            .check_provider(&context.tenant_id, &candidate.provider_id)
        {
            return Err(deny(error.to_string()));
        }
        if let Err(error) = self
            .gate
            .check_account(&context.tenant_id, &candidate.account_id)
        {
            return Err(deny(error.to_string()));
        }
        if let Err(error) = self
            .gate
            .check_model(&context.tenant_id, &candidate.model_id)
        {
            return Err(deny(error.to_string()));
        }
        self.gate.record_decision_scoped(
            &identity,
            PolicyGate::RouteCandidate,
            PolicyDecisionKind::Allow,
            "route candidate 放行",
            AuditDimensions {
                provider_id: Some(candidate.provider_id.clone()),
                account_id: Some(candidate.account_id.clone()),
                ..AuditDimensions::default()
            },
        );
        Ok(())
    }
}

fn to_api_role(role: PrincipalRole) -> ApiPrincipalRole {
    match role {
        PrincipalRole::Admin => ApiPrincipalRole::Admin,
        PrincipalRole::User => ApiPrincipalRole::User,
        PrincipalRole::Service => ApiPrincipalRole::Service,
        PrincipalRole::Viewer => ApiPrincipalRole::Viewer,
    }
}

fn to_api_gate(gate: PolicyGate) -> ApiPolicyGate {
    match gate {
        PolicyGate::RouteCandidate => ApiPolicyGate::RouteCandidate,
        PolicyGate::LeaseAcquire => ApiPolicyGate::LeaseAcquire,
        PolicyGate::AgentSpawn => ApiPolicyGate::AgentSpawn,
        PolicyGate::RequestAdmission => ApiPolicyGate::RequestAdmission,
        PolicyGate::SessionQuery => ApiPolicyGate::SessionQuery,
        PolicyGate::UsageQuery => ApiPolicyGate::UsageQuery,
        PolicyGate::AuditQuery => ApiPolicyGate::AuditQuery,
        PolicyGate::AuditExport => ApiPolicyGate::AuditExport,
        PolicyGate::Retention => ApiPolicyGate::Retention,
    }
}

fn to_api_decision_kind(kind: PolicyDecisionKind) -> ApiDecisionKind {
    match kind {
        PolicyDecisionKind::Allow => ApiDecisionKind::Allow,
        PolicyDecisionKind::Deny => ApiDecisionKind::Deny,
        PolicyDecisionKind::Limit => ApiDecisionKind::Limit,
        PolicyDecisionKind::Fallback => ApiDecisionKind::Fallback,
    }
}

fn to_audit_decision(kind: PolicyDecisionKind) -> AuditDecision {
    match kind {
        PolicyDecisionKind::Allow => AuditDecision::Allow,
        PolicyDecisionKind::Deny => AuditDecision::Deny,
        PolicyDecisionKind::Limit => AuditDecision::Limit,
        PolicyDecisionKind::Fallback => AuditDecision::Fallback,
    }
}

fn policy_reason_code(gate: PolicyGate, decision: PolicyDecisionKind) -> &'static str {
    match (gate, decision) {
        (PolicyGate::RouteCandidate, PolicyDecisionKind::Allow) => "route_candidate_allow",
        (PolicyGate::RouteCandidate, PolicyDecisionKind::Deny) => "route_candidate_deny",
        (PolicyGate::RouteCandidate, PolicyDecisionKind::Limit) => "route_candidate_limit",
        (PolicyGate::RouteCandidate, PolicyDecisionKind::Fallback) => "route_candidate_fallback",
        (PolicyGate::LeaseAcquire, PolicyDecisionKind::Allow) => "lease_admission_allow",
        (PolicyGate::LeaseAcquire, PolicyDecisionKind::Deny) => "lease_admission_deny",
        (PolicyGate::LeaseAcquire, PolicyDecisionKind::Limit) => "lease_admission_limit",
        (PolicyGate::LeaseAcquire, PolicyDecisionKind::Fallback) => "lease_admission_fallback",
        (_, PolicyDecisionKind::Allow) => "policy_allow",
        (_, PolicyDecisionKind::Deny) => "policy_deny",
        (_, PolicyDecisionKind::Limit) => "policy_limit",
        (_, PolicyDecisionKind::Fallback) => "policy_fallback",
    }
}

fn decision_event_to_view(event: PolicyDecisionEvent) -> PolicyDecisionEventView {
    PolicyDecisionEventView {
        policy_version: event.policy_version,
        tenant_id: event.tenant_id,
        principal_id: event.principal_id.to_string(),
        gate: to_api_gate(event.gate),
        decision: to_api_decision_kind(event.decision),
        reason: event.reason,
        at_ms: event.at_ms,
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}
