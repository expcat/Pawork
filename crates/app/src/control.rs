//! S11 波 D：控制面运行时（usage ledger + LocalLedger quota + audit）。
//!
//! 生产打开实例目录下的 SQLite 账本与 JSONL audit；测试 / `from_parts`
//! 使用内存后端。查询路径只打 LocalLedger，不访问厂商 billing API。

use std::path::Path;
use std::sync::Arc;

use pawork_control_plane::{
    default_principal, default_tenant, AuditAction, AuditDecision, AuditEventV1, AuditSink,
    AuditTargetKind, CostConfidence, FileAuditStore, InMemoryAuditStore, InMemoryUsageLedger,
    SqliteUsageLedger, UsageLedger, UsageQuery, UsageRecord, UsageTotals, RECORD_VERSION,
};
use pawork_domain::{
    AgentId, EventId, ModelId, ProviderId, RequestId, RunId, SessionId, TenantId, TokenUsage,
};
use pawork_protocol::DEFAULT_QUOTA_TENANT;
use pawork_control_plane::credential::{CredentialPool, InMemoryCredentialPool};
use pawork_control_plane::quota::service::{ScopeMatch, SystemQuotaClock, WindowRead};
use pawork_control_plane::quota::{
    LedgerQuotaAdapter, QuotaClock, QuotaMeasure, QuotaScope, QuotaService, QuotaUnit, QuotaWindow,
};
use pawork_control_plane::{InMemoryTenantPolicyEngine, TenantPolicyEngine};
use serde::Serialize;

use crate::AppError;

/// 账本 / 租约默认账号（控制面 `local/default`，不是 quota 哨兵 `local`）。
pub const LEDGER_ACCOUNT: &str = "local/default";

/// 宿主侧控制面：同一份 ledger 供入账、quota 投影与 supervisor budget 共用。
pub struct ControlPlaneRuntime {
    pub ledger: Arc<dyn UsageLedger>,
    pub quota: QuotaService,
    pub audit: Arc<dyn AuditSink>,
    pub policy: Arc<dyn TenantPolicyEngine>,
    pub pool: Arc<dyn CredentialPool>,
}

impl ControlPlaneRuntime {
    pub fn in_memory() -> Self {
        let ledger: Arc<dyn UsageLedger> = Arc::new(InMemoryUsageLedger::new());
        Self::assemble(ledger, Arc::new(InMemoryAuditStore::default()))
    }

    pub fn persistent(dir: impl AsRef<Path>) -> Result<Self, AppError> {
        let dir = dir.as_ref();
        std::fs::create_dir_all(dir)?;
        let ledger: Arc<dyn UsageLedger> = Arc::new(
            SqliteUsageLedger::open(dir.join("usage-ledger.sqlite3"))
                .map_err(|error| AppError::ControlPlane(error.to_string()))?,
        );
        let audit: Arc<dyn AuditSink> = Arc::new(
            FileAuditStore::open(dir.join("audit.jsonl"))
                .map_err(|error| AppError::ControlPlane(error.to_string()))?,
        );
        Ok(Self::assemble(ledger, audit))
    }

    fn assemble(ledger: Arc<dyn UsageLedger>, audit: Arc<dyn AuditSink>) -> Self {
        let clock: Arc<dyn QuotaClock> = Arc::new(SystemQuotaClock);
        let adapter = Arc::new(LedgerQuotaAdapter::new(Arc::clone(&ledger), clock.clone()));
        let quota = QuotaService::new(clock);
        quota.register(ScopeMatch::any(), adapter.clone());
        quota.set_ledger_reconciler(adapter);
        Self {
            ledger,
            quota,
            audit,
            policy: Arc::new(InMemoryTenantPolicyEngine::default()),
            pool: Arc::new(InMemoryCredentialPool::new(4)),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct UsageOverview {
    pub provider_id: String,
    pub session: Option<SessionUsageLine>,
    pub ledger: LedgerTotals,
    pub windows: Vec<QuotaWindowLine>,
}

#[derive(Clone, Debug, Serialize)]
pub struct SessionUsageLine {
    pub session_id: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct LedgerTotals {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct QuotaWindowLine {
    pub window: String,
    pub used: String,
    pub limit: String,
    pub remaining: String,
    pub confidence: String,
}

impl From<UsageTotals> for LedgerTotals {
    fn from(totals: UsageTotals) -> Self {
        Self {
            input_tokens: totals.input_tokens,
            output_tokens: totals.output_tokens,
            cache_read_tokens: totals.cache_read_tokens,
            cache_write_tokens: totals.cache_write_tokens,
        }
    }
}

/// quota 查询 tenant：`local` 与 `local/default` 都映射到账本租户 `local/default`。
pub fn ledger_tenant(query_tenant: Option<&TenantId>) -> TenantId {
    match query_tenant.map(TenantId::as_str) {
        None | Some(DEFAULT_QUOTA_TENANT) | Some("local/default") => default_tenant(),
        Some(other) => TenantId::new(other),
    }
}

pub fn usage_record(
    session_id: &SessionId,
    run_id: &RunId,
    request_id: &RequestId,
    provider_id: &ProviderId,
    model_id: &ModelId,
    usage: &TokenUsage,
    cost_micros: u64,
    currency: &str,
) -> UsageRecord {
    UsageRecord {
        record_id: format!("rec-{}", run_id.as_str()),
        version: RECORD_VERSION,
        tenant_id: default_tenant(),
        principal_id: default_principal(),
        account_id: LEDGER_ACCOUNT.to_string(),
        credential_id: None,
        session_id: session_id.clone(),
        agent_id: AgentId::new("agent-host"),
        run_id: Some(run_id.clone()),
        provider_id: provider_id.clone(),
        model_id: model_id.clone(),
        input_tokens: usage.input_tokens,
        output_tokens: usage.output_tokens,
        cache_read_tokens: usage.cache_read_tokens,
        cache_write_tokens: usage.cache_write_tokens,
        cost_micros,
        currency: priced_currency(currency),
        occurred_at_ms: pawork_engine::now_timestamp().as_unix_millis(),
        request_id: Some(request_id.clone()),
        event_id: None,
        upstream_attempt: Some(1),
        trace_id: None,
        rate_card: None,
        rate_version: None,
        cost_confidence: if is_iso_currency(currency) {
            Some(CostConfidence::Estimated)
        } else {
            Some(CostConfidence::Unknown)
        },
        cost_provenance: Some(
            if is_iso_currency(currency) {
                "model-registry:estimate"
            } else {
                "unpriced"
            }
            .into(),
        ),
    }
}

fn is_iso_currency(currency: &str) -> bool {
    currency.len() == 3 && currency.bytes().all(|byte| byte.is_ascii_uppercase())
}

/// 无定价或非法币种用 ISO 4217 `XXX`（未指定），禁止静默标 USD。
fn priced_currency(currency: &str) -> String {
    if is_iso_currency(currency) {
        currency.to_string()
    } else {
        "XXX".into()
    }
}

pub fn quota_scope(provider_id: &ProviderId) -> QuotaScope {
    QuotaScope::new(
        default_tenant(),
        pawork_control_plane::quota::AccountId::new(LEDGER_ACCOUNT),
        provider_id.clone(),
        None,
    )
}

pub async fn ledger_totals(
    ledger: &dyn UsageLedger,
    provider_id: &ProviderId,
    session_id: Option<&SessionId>,
) -> Result<UsageTotals, AppError> {
    let query = UsageQuery {
        tenant_id: Some(ledger_tenant(None)),
        account_id: Some(LEDGER_ACCOUNT.to_string()),
        provider_id: Some(provider_id.clone()),
        session_id: session_id.cloned(),
        ..UsageQuery::default()
    };
    ledger
        .aggregate(&query)
        .await
        .map_err(|error| AppError::ControlPlane(error.to_string()))
}

pub async fn quota_windows(
    quota: &QuotaService,
    provider_id: &ProviderId,
) -> Result<Vec<QuotaWindowLine>, AppError> {
    let scope = quota_scope(provider_id);
    let windows = [
        QuotaWindow::Overall,
        QuotaWindow::Rolling5h,
        QuotaWindow::Weekly,
        QuotaWindow::Monthly,
    ];
    let overview = quota
        .overview(
            &scope,
            &windows,
            &QuotaUnit::Token,
            &pawork_domain::CancellationToken::new(),
        )
        .await;
    let mut lines = Vec::new();
    for window in windows {
        let name = match window {
            QuotaWindow::Overall => "overall",
            QuotaWindow::Rolling5h => "rolling_5h",
            QuotaWindow::Weekly => "weekly",
            QuotaWindow::Monthly => "monthly",
        };
        let line = match overview.windows.get(&window) {
            Some(WindowRead::Ok(read)) => QuotaWindowLine {
                window: name.into(),
                used: format_measure(read.snapshot.values.used),
                limit: format_measure(read.snapshot.values.limit),
                remaining: format_measure(read.snapshot.values.remaining),
                confidence: format!("{:?}", read.snapshot.confidence).to_ascii_lowercase(),
            },
            Some(WindowRead::Failed { failures }) => QuotaWindowLine {
                window: name.into(),
                used: "error".into(),
                limit: "error".into(),
                remaining: "error".into(),
                confidence: failures
                    .first()
                    .map(|failure| failure.error.to_string())
                    .unwrap_or_else(|| "failed".into()),
            },
            None => QuotaWindowLine {
                window: name.into(),
                used: "unknown".into(),
                limit: "unknown".into(),
                remaining: "unknown".into(),
                confidence: "none".into(),
            },
        };
        lines.push(line);
    }
    Ok(lines)
}

fn format_measure(measure: QuotaMeasure) -> String {
    match measure {
        QuotaMeasure::Exact(value) => value.to_string(),
        QuotaMeasure::Infinite => "inf".into(),
        QuotaMeasure::Unknown => "unknown".into(),
    }
}

pub fn append_audit(
    audit: &dyn AuditSink,
    action: AuditAction,
    target: AuditTargetKind,
    decision: AuditDecision,
    reason_code: &str,
) {
    let event = AuditEventV1::new(
        EventId::from(format!(
            "aud-{}",
            pawork_engine::now_timestamp().as_unix_millis()
        )),
        pawork_engine::now_timestamp(),
        default_tenant(),
        default_principal(),
        action,
        target,
        decision,
        reason_code,
        1,
    );
    if let Err(error) = audit.append(event) {
        tracing::warn!(error = %error, "audit append failed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pawork_domain::{ModelId, ProviderId, RequestId, RunId, SessionId, TokenUsage};

    #[test]
    fn unpriced_usage_uses_xxx_not_usd() {
        let record = usage_record(
            &SessionId::from("s"),
            &RunId::from("r"),
            &RequestId::from("req"),
            &ProviderId::from("p"),
            &ModelId::from("m"),
            &TokenUsage {
                input_tokens: 1,
                ..TokenUsage::default()
            },
            0,
            "",
        );
        assert_eq!(record.currency, "XXX");
        assert_eq!(record.cost_confidence, Some(CostConfidence::Unknown));
        assert_eq!(record.cost_provenance.as_deref(), Some("unpriced"));
        assert_ne!(record.currency, "USD");
    }
}
