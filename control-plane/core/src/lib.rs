//! Pawork 控制面：tenant / identity、usage ledger、audit JSONL。
//!
//! 合并自 V1 `tenant-service` + `usage-ledger` + `audit-log`。关键 trait
//! （[`UsageLedger`] / [`TenantPolicyEngine`] / [`AuditSink`] / [`AuditStore`]）
//! 保持 V1 签名，供波 C 注入。`sqlite` feature（默认开启）门控
//! [`SqliteUsageLedger`]。

pub mod audit;
pub mod decision;
pub mod identity;
pub mod rbac;
pub mod tenant;
pub mod usage;

pub use audit::{
    AUDIT_SCHEMA_VERSION, AuditAction, AuditDecision, AuditDimensions, AuditError, AuditEventV1,
    AuditSink, AuditStore, AuditTargetKind, FileAuditStore, InMemoryAuditStore,
};
pub use decision::{sanitize_reason, PolicyDecisionEvent, PolicyDecisionKind, PolicyGate};
pub use identity::{
    default_principal, default_tenant, IdentityContext, IdentityError, IdentityResolver,
    LocalIdentityResolver, DEFAULT_PRINCIPAL, DEFAULT_TENANT,
};
pub use pawork_domain::{
    AccountId, AgentId, EventId, ModelId, PrincipalId, ProviderId, RequestId, RunId, SessionId,
    TenantId, Timestamp,
};
pub use rbac::{AuditExportPolicy, Permission, PermissionProfile, PrincipalRole};
pub use tenant::{
    decide_account, decide_agent_concurrency, decide_audit_export, decide_budget, decide_model,
    decide_permission, decide_provider, decide_request_concurrency, decide_retention,
    BudgetDimension, ConcurrencyKind, InMemoryTenantPolicyEngine, PolicyDecision, TenantPolicy,
    TenantPolicyEngine, TenantPolicyError,
};
pub use usage::{
    CostConfidence, InMemoryUsageLedger, UsageAttribution, UsageFilterField, UsageLedger,
    UsageLedgerError, UsageQuery, UsageRecord, UsageTotals, AUTO_RECORD_ID_PREFIX, RECORD_VERSION,
};

#[cfg(feature = "sqlite")]
pub use usage::{SqliteUsageLedger, SCHEMA_VERSION};
