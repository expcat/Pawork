//! Pawork 控制面：tenant / identity、usage ledger、audit JSONL。
//!
//! 合并自 V1 `tenant-service` + `usage-ledger` + `audit-log`，并收回
//! `app-database` 的 identity 注册表迁移。关键 trait（[`UsageLedger`] /
//! [`TenantPolicyEngine`] / [`AuditSink`] / [`AuditStore`]）保持 V1 签名，
//! 供波 C 注入。`sqlite` feature（默认开启）门控 [`SqliteUsageLedger`] 与
//! identity 迁移；OTel exporter 类型随迁，装配链未接通。

pub mod audit;
pub mod decision;
pub mod identity;
pub mod rbac;
pub mod tenant;
pub mod usage;

#[cfg(feature = "sqlite")]
pub mod identity_schema;

pub use audit::{
    export_tenant, is_safe_audit_label, ALLOWED_EXPORT_ATTRIBUTES, AUDIT_SCHEMA_VERSION,
    AuditAction, AuditDecision, AuditDimensions, AuditError, AuditEventV1, AuditExporter,
    AuditSink, AuditStore, AuditTargetKind, ExportRecord, FileAuditStore, InMemoryAuditStore,
    InMemoryOtelExporter, OtelAuditExporter, TracingAuditExporter,
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
pub use identity_schema::{
    backfill_legacy_default_identity, identity_tenant, migrate as migrate_identity,
    schema_version as identity_schema_version, IdentityTenant, CURRENT_IDENTITY_SCHEMA_VERSION,
    IDENTITY_MIGRATIONS_TABLE, LEGACY_PRINCIPAL, LEGACY_TENANT,
};
#[cfg(feature = "sqlite")]
pub use usage::{SqliteUsageLedger, SCHEMA_VERSION};
