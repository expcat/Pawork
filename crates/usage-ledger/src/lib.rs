//! # usage-ledger — 多维 Usage/Cost 账本（P18-8 最小契约）
//!
//! 不可变的、可按 tenant/principal/account/session/agent/provider/model
//! 多维归属的用量成本账本。供 Phase 12 编排做预算归属与成本核算。
//!
//! 当前阶段为内存追加存储（`InMemoryUsageLedger`），不接入网络 / 数据库 /
//! Secret；持久化属于完整 P18 工作。
//!
//! 类型命名保持英文，crate 文档使用中文。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// 记录 ID 自动生成计数器（`UsageRecord::default` 与账本补写共用）。
static NEXT_RECORD_ID: AtomicU64 = AtomicU64::new(0);

/// 重新导出领域 crate 的标识类型，供账本使用者统一引用。
pub use agent_domain::{AgentId, ModelId, PrincipalId, ProviderId, RunId, SessionId, TenantId};

/// 一条不可变的多维 usage/cost 记录。
///
/// `record_id` 为空时会由账本在 `record` 时以原子计数器补写；也可由
/// `UsageRecord::default` 直接生成。
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct UsageRecord {
    pub record_id: String,
    pub tenant_id: TenantId,
    pub principal_id: PrincipalId,
    pub account_id: String,
    pub session_id: SessionId,
    pub agent_id: AgentId,
    pub run_id: Option<RunId>,
    pub provider_id: ProviderId,
    pub model_id: ModelId,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub cost_micros: u64,
    pub currency: String,
    pub occurred_at_ms: u64,
}

impl Default for UsageRecord {
    /// 生成默认记录：`record_id` 由原子计数器补写，其余字段取默认空值。
    fn default() -> Self {
        Self {
            record_id: format!("rec-{}", NEXT_RECORD_ID.fetch_add(1, Ordering::Relaxed)),
            tenant_id: TenantId::default(),
            principal_id: PrincipalId::default(),
            account_id: String::new(),
            session_id: SessionId::default(),
            agent_id: AgentId::default(),
            run_id: None,
            provider_id: ProviderId::default(),
            model_id: ModelId::default(),
            input_tokens: 0,
            output_tokens: 0,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            cost_micros: 0,
            currency: String::new(),
            occurred_at_ms: 0,
        }
    }
}

/// 用量与成本聚合结果。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct UsageTotals {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub cost_micros: u64,
}

impl UsageTotals {
    /// 累加单条记录的用量与成本字段。
    pub fn add(&mut self, record: &UsageRecord) {
        self.input_tokens += record.input_tokens;
        self.output_tokens += record.output_tokens;
        self.cache_read_tokens += record.cache_read_tokens;
        self.cache_write_tokens += record.cache_write_tokens;
        self.cost_micros += record.cost_micros;
    }
}

impl std::ops::Add for UsageTotals {
    type Output = Self;

    fn add(mut self, rhs: Self) -> Self::Output {
        self += rhs;
        self
    }
}

impl std::ops::AddAssign for UsageTotals {
    fn add_assign(&mut self, rhs: Self) {
        self.input_tokens = self.input_tokens.saturating_add(rhs.input_tokens);
        self.output_tokens = self.output_tokens.saturating_add(rhs.output_tokens);
        self.cache_read_tokens = self.cache_read_tokens.saturating_add(rhs.cache_read_tokens);
        self.cache_write_tokens = self
            .cache_write_tokens
            .saturating_add(rhs.cache_write_tokens);
        self.cost_micros = self.cost_micros.saturating_add(rhs.cost_micros);
    }
}

/// 查询过滤维度，供 `UsageQuery` 的维度语义参考。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UsageFilterField {
    Tenant,
    Principal,
    Account,
    Session,
    Agent,
}

/// 多维查询条件：仅 `Some` 的维度参与过滤，全部满足才命中。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct UsageQuery {
    pub tenant_id: Option<TenantId>,
    pub principal_id: Option<PrincipalId>,
    pub account_id: Option<String>,
    pub session_id: Option<SessionId>,
    pub agent_id: Option<AgentId>,
}

impl UsageQuery {
    /// 仅按 tenant 过滤。
    pub fn by_tenant(tenant_id: TenantId) -> Self {
        Self {
            tenant_id: Some(tenant_id),
            ..Self::default()
        }
    }

    /// 仅按 session 过滤。
    pub fn by_session(session_id: SessionId) -> Self {
        Self {
            session_id: Some(session_id),
            ..Self::default()
        }
    }

    /// 仅按 agent 过滤。
    pub fn by_agent(agent_id: AgentId) -> Self {
        Self {
            agent_id: Some(agent_id),
            ..Self::default()
        }
    }
}

/// 账本操作错误。
#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum UsageLedgerError {
    /// 记录内容不合法（tenant、token 用量或时间戳校验失败）。
    #[error("invalid usage record: {reason}")]
    InvalidRecord { reason: String },
}

/// 只读多维用量账本接口。
#[async_trait]
pub trait UsageLedger: Send + Sync {
    /// 写入一条记录；校验失败返回 `InvalidRecord`。
    async fn record(&self, record: UsageRecord) -> Result<(), UsageLedgerError>;

    /// 按查询条件返回全部命中的记录。
    async fn query(&self, query: &UsageQuery) -> Vec<UsageRecord>;

    /// 按查询条件聚合用量与成本。
    async fn aggregate(&self, query: &UsageQuery) -> UsageTotals;
}

/// 内存追加存储实现：`record` 顺序追加，`query` / `aggregate` 线性过滤与求和。
#[derive(Debug)]
pub struct InMemoryUsageLedger {
    records: Arc<Mutex<Vec<UsageRecord>>>,
}

impl InMemoryUsageLedger {
    /// 新建空账本。
    pub fn new() -> Self {
        Self {
            records: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

impl Default for InMemoryUsageLedger {
    fn default() -> Self {
        Self::new()
    }
}

impl InMemoryUsageLedger {
    fn validate(record: &UsageRecord) -> Result<(), UsageLedgerError> {
        if record.tenant_id.as_str().is_empty() {
            return Err(UsageLedgerError::InvalidRecord {
                reason: "tenant_id 不能为空".to_string(),
            });
        }
        let total_tokens = record
            .input_tokens
            .saturating_add(record.output_tokens)
            .saturating_add(record.cache_read_tokens)
            .saturating_add(record.cache_write_tokens);
        if total_tokens == 0 {
            return Err(UsageLedgerError::InvalidRecord {
                reason: "input/output/cache token 总量必须大于 0".to_string(),
            });
        }
        if record.occurred_at_ms == 0 {
            return Err(UsageLedgerError::InvalidRecord {
                reason: "occurred_at_ms 必须大于 0".to_string(),
            });
        }
        Ok(())
    }

    fn matches(record: &UsageRecord, query: &UsageQuery) -> bool {
        if let Some(ref tenant_id) = query.tenant_id {
            if record.tenant_id != *tenant_id {
                return false;
            }
        }
        if let Some(ref principal_id) = query.principal_id {
            if record.principal_id != *principal_id {
                return false;
            }
        }
        if let Some(ref account_id) = query.account_id {
            if record.account_id.as_str() != account_id.as_str() {
                return false;
            }
        }
        if let Some(ref session_id) = query.session_id {
            if record.session_id != *session_id {
                return false;
            }
        }
        if let Some(ref agent_id) = query.agent_id {
            if record.agent_id != *agent_id {
                return false;
            }
        }
        true
    }
}

#[async_trait]
impl UsageLedger for InMemoryUsageLedger {
    async fn record(&self, mut record: UsageRecord) -> Result<(), UsageLedgerError> {
        Self::validate(&record)?;
        if record.record_id.is_empty() {
            record.record_id = format!("rec-{}", NEXT_RECORD_ID.fetch_add(1, Ordering::Relaxed));
        }
        // std Mutex 同步锁不跨 await，整段操作保持同步。
        let mut records = self.records.lock().expect("usage ledger mutex poisoned");
        records.push(record.clone());
        tracing::debug!(
            record_id = %record.record_id,
            tenant_id = %record.tenant_id,
            "usage record recorded"
        );
        Ok(())
    }

    async fn query(&self, query: &UsageQuery) -> Vec<UsageRecord> {
        let records = self.records.lock().expect("usage ledger mutex poisoned");
        records
            .iter()
            .filter(|record| Self::matches(record, query))
            .cloned()
            .collect()
    }

    async fn aggregate(&self, query: &UsageQuery) -> UsageTotals {
        let mut totals = UsageTotals::default();
        for record in self.query(query).await {
            totals.add(&record);
        }
        totals
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_record(
        record_id: &str,
        tenant: &str,
        principal: &str,
        account: &str,
        session: &str,
        agent: &str,
    ) -> UsageRecord {
        UsageRecord {
            record_id: record_id.to_string(),
            tenant_id: TenantId::new(tenant),
            principal_id: PrincipalId::new(principal),
            account_id: account.to_string(),
            session_id: SessionId::new(session),
            agent_id: AgentId::new(agent),
            run_id: None,
            provider_id: ProviderId::new("openai"),
            model_id: ModelId::new("gpt-4o"),
            input_tokens: 100,
            output_tokens: 50,
            cache_read_tokens: 10,
            cache_write_tokens: 5,
            cost_micros: 1_250,
            currency: "USD".to_string(),
            occurred_at_ms: 1_700_000_000_000,
        }
    }

    #[tokio::test]
    async fn record_and_query_roundtrip() {
        let ledger = InMemoryUsageLedger::new();

        // 显式 record_id：roundtrip 应原样保留。
        let explicit = make_record(
            "rec-explicit",
            "tenant-a",
            "principal-1",
            "account-x",
            "session-1",
            "agent-1",
        );
        ledger.record(explicit.clone()).await.unwrap();

        // 空 record_id：账本应以原子计数器补写。
        let generated = make_record(
            "",
            "tenant-b",
            "principal-2",
            "account-y",
            "session-2",
            "agent-2",
        );
        ledger.record(generated.clone()).await.unwrap();

        let records = ledger.query(&UsageQuery::default()).await;
        assert_eq!(records.len(), 2);

        let returned_explicit = records
            .iter()
            .find(|r| r.record_id == "rec-explicit")
            .expect("explicit record present");
        assert_eq!(returned_explicit, &explicit);

        let returned_generated = records
            .iter()
            .find(|r| r.tenant_id == TenantId::new("tenant-b"))
            .expect("generated record present");
        assert!(!returned_generated.record_id.is_empty());
        assert_ne!(returned_generated.record_id, generated.record_id);
    }

    #[tokio::test]
    async fn aggregate_sums_tokens_and_cost() {
        let ledger = InMemoryUsageLedger::new();
        ledger
            .record(make_record(
                "r1",
                "tenant-a",
                "principal-1",
                "account-x",
                "session-1",
                "agent-1",
            ))
            .await
            .unwrap();
        ledger
            .record(make_record(
                "r2",
                "tenant-a",
                "principal-1",
                "account-x",
                "session-1",
                "agent-1",
            ))
            .await
            .unwrap();
        ledger
            .record(make_record(
                "r3",
                "tenant-a",
                "principal-1",
                "account-x",
                "session-1",
                "agent-1",
            ))
            .await
            .unwrap();

        let totals = ledger.aggregate(&UsageQuery::default()).await;
        assert_eq!(totals.input_tokens, 300);
        assert_eq!(totals.output_tokens, 150);
        assert_eq!(totals.cache_read_tokens, 30);
        assert_eq!(totals.cache_write_tokens, 15);
        assert_eq!(totals.cost_micros, 3_750);
    }

    #[tokio::test]
    async fn filter_by_tenant_excludes_other_tenants() {
        let ledger = InMemoryUsageLedger::new();
        ledger
            .record(make_record(
                "r1",
                "tenant-a",
                "principal-1",
                "account-x",
                "session-1",
                "agent-1",
            ))
            .await
            .unwrap();
        ledger
            .record(make_record(
                "r2",
                "tenant-b",
                "principal-1",
                "account-x",
                "session-1",
                "agent-1",
            ))
            .await
            .unwrap();

        let records = ledger
            .query(&UsageQuery::by_tenant(TenantId::new("tenant-a")))
            .await;
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].tenant_id, TenantId::new("tenant-a"));
    }

    #[tokio::test]
    async fn filter_by_session() {
        let ledger = InMemoryUsageLedger::new();
        ledger
            .record(make_record(
                "r1",
                "tenant-a",
                "principal-1",
                "account-x",
                "session-1",
                "agent-1",
            ))
            .await
            .unwrap();
        ledger
            .record(make_record(
                "r2",
                "tenant-a",
                "principal-1",
                "account-x",
                "session-2",
                "agent-1",
            ))
            .await
            .unwrap();

        let records = ledger
            .query(&UsageQuery::by_session(SessionId::new("session-2")))
            .await;
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].session_id, SessionId::new("session-2"));
    }

    #[tokio::test]
    async fn filter_by_agent() {
        let ledger = InMemoryUsageLedger::new();
        ledger
            .record(make_record(
                "r1",
                "tenant-a",
                "principal-1",
                "account-x",
                "session-1",
                "agent-1",
            ))
            .await
            .unwrap();
        ledger
            .record(make_record(
                "r2",
                "tenant-a",
                "principal-1",
                "account-x",
                "session-1",
                "agent-2",
            ))
            .await
            .unwrap();

        let records = ledger
            .query(&UsageQuery::by_agent(AgentId::new("agent-2")))
            .await;
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].agent_id, AgentId::new("agent-2"));
    }

    #[tokio::test]
    async fn cross_tenant_isolation() {
        let ledger = InMemoryUsageLedger::new();
        for i in 0..3 {
            ledger
                .record(make_record(
                    &format!("r{i}"),
                    "tenant-a",
                    "principal-1",
                    "account-x",
                    "session-1",
                    "agent-1",
                ))
                .await
                .unwrap();
        }

        // tenant B 无法看到 tenant A 的任何记录。
        let records = ledger
            .query(&UsageQuery::by_tenant(TenantId::new("tenant-b")))
            .await;
        assert!(records.is_empty());

        let totals = ledger
            .aggregate(&UsageQuery::by_tenant(TenantId::new("tenant-b")))
            .await;
        assert_eq!(totals, UsageTotals::default());
    }

    #[tokio::test]
    async fn invalid_record_rejected() {
        let ledger = InMemoryUsageLedger::new();

        // 空 tenant_id 被拒绝。
        let bad_tenant = make_record(
            "bad",
            "",
            "principal-1",
            "account-x",
            "session-1",
            "agent-1",
        );
        let err = ledger.record(bad_tenant).await.unwrap_err();
        assert!(matches!(err, UsageLedgerError::InvalidRecord { .. }));

        // 完全没有 token 用量的记录被拒绝。
        let mut zero_tokens = make_record(
            "zero",
            "tenant-a",
            "principal-1",
            "account-x",
            "session-1",
            "agent-1",
        );
        zero_tokens.input_tokens = 0;
        zero_tokens.output_tokens = 0;
        zero_tokens.cache_read_tokens = 0;
        zero_tokens.cache_write_tokens = 0;
        let err = ledger.record(zero_tokens).await.unwrap_err();
        assert!(matches!(err, UsageLedgerError::InvalidRecord { .. }));

        // occurred_at_ms 为 0 被拒绝。
        let mut bad_time = make_record(
            "time",
            "tenant-a",
            "principal-1",
            "account-x",
            "session-1",
            "agent-1",
        );
        bad_time.occurred_at_ms = 0;
        let err = ledger.record(bad_time).await.unwrap_err();
        assert!(matches!(err, UsageLedgerError::InvalidRecord { .. }));

        // 校验失败的记录不应被写入。
        assert!(ledger.query(&UsageQuery::default()).await.is_empty());
    }
}
