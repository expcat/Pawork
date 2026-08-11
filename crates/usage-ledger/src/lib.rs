//! # usage-ledger — 多维 Usage/Cost 账本（P18-8 最小契约）
//!
//! 不可变的、可按 tenant/principal/account/credential/session/agent/provider/model/currency
//! 多维归属的用量成本账本。供 Phase 12 编排做预算归属与成本核算。
//!
//! 当前阶段为内存追加存储（`InMemoryUsageLedger`），不接入网络 / 数据库 /
//! Secret；持久化属于完整 P18 工作。
//!
//! `record_id` 在 (tenant, account) 作用域内作为幂等键：相同 ID 与相同内容
//! 的重放成功且不重复记账，相同 ID 不同内容返回结构化冲突；空 ID 自动补写，
//! 自动 ID 使用保留前缀 `auto-rec-*`，与显式 `rec-*` 命名空间隔离。
//! 查询支持 provider / model / run 维度与 `occurred_at_ms` 半开区间过滤，
//! 聚合采用饱和累加；命中记录币种不一致时聚合返回显式错误而非静默混加。
//!
//! 类型命名保持英文，crate 文档使用中文。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// 自动生成记录 ID 的保留前缀。
///
/// `auto-rec-*` 命名空间仅供账本自动补写与系统组件（如预算控制器 flush）
/// 生成幂等键使用；显式 `record_id`（惯例 `rec-*` 或其他命名）不得使用该
/// 前缀，否则可能与自动 ID 冲突。
pub const AUTO_RECORD_ID_PREFIX: &str = "auto-rec-";

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
    #[serde(default)]
    pub credential_id: Option<String>,
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
            record_id: format!(
                "{AUTO_RECORD_ID_PREFIX}{}",
                NEXT_RECORD_ID.fetch_add(1, Ordering::Relaxed)
            ),
            tenant_id: TenantId::default(),
            principal_id: PrincipalId::default(),
            account_id: String::new(),
            credential_id: None,
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
    /// 累加单条记录的用量与成本字段（饱和累加，不溢出）。
    pub fn add(&mut self, record: &UsageRecord) {
        self.input_tokens = self.input_tokens.saturating_add(record.input_tokens);
        self.output_tokens = self.output_tokens.saturating_add(record.output_tokens);
        self.cache_read_tokens = self
            .cache_read_tokens
            .saturating_add(record.cache_read_tokens);
        self.cache_write_tokens = self
            .cache_write_tokens
            .saturating_add(record.cache_write_tokens);
        self.cost_micros = self.cost_micros.saturating_add(record.cost_micros);
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

impl UsageRecord {
    /// 比较除 `record_id` 外的全部内容字段，用于幂等重放判定。
    fn same_content(&self, other: &Self) -> bool {
        self.tenant_id == other.tenant_id
            && self.principal_id == other.principal_id
            && self.account_id == other.account_id
            && self.credential_id == other.credential_id
            && self.session_id == other.session_id
            && self.agent_id == other.agent_id
            && self.run_id == other.run_id
            && self.provider_id == other.provider_id
            && self.model_id == other.model_id
            && self.input_tokens == other.input_tokens
            && self.output_tokens == other.output_tokens
            && self.cache_read_tokens == other.cache_read_tokens
            && self.cache_write_tokens == other.cache_write_tokens
            && self.cost_micros == other.cost_micros
            && self.currency == other.currency
            && self.occurred_at_ms == other.occurred_at_ms
    }
}

/// 查询过滤维度，供 `UsageQuery` 的维度语义参考。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UsageFilterField {
    Tenant,
    Principal,
    Account,
    Credential,
    Session,
    Agent,
    Run,
    Provider,
    Model,
    Currency,
    OccurredAt,
}

/// 多维查询条件：仅 `Some` 的维度参与过滤，全部满足才命中。
///
/// `occurred_at_start_ms` / `occurred_at_end_ms` 构成半开区间 `[start, end)`：
/// 含起点、不含终点；仅设置一端时为一侧开区间。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct UsageQuery {
    pub tenant_id: Option<TenantId>,
    pub principal_id: Option<PrincipalId>,
    pub account_id: Option<String>,
    pub credential_id: Option<String>,
    pub session_id: Option<SessionId>,
    pub agent_id: Option<AgentId>,
    pub run_id: Option<RunId>,
    pub provider_id: Option<ProviderId>,
    pub model_id: Option<ModelId>,
    pub currency: Option<String>,
    pub occurred_at_start_ms: Option<u64>,
    pub occurred_at_end_ms: Option<u64>,
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

    /// 仅按 credential 过滤（记录 `credential_id` 必须匹配，`None` 记录不命中）。
    pub fn by_credential(credential_id: impl Into<String>) -> Self {
        Self {
            credential_id: Some(credential_id.into()),
            ..Self::default()
        }
    }

    /// 仅按 run 过滤（记录 `run_id` 必须匹配，`None` 记录不命中）。
    pub fn by_run(run_id: RunId) -> Self {
        Self {
            run_id: Some(run_id),
            ..Self::default()
        }
    }

    /// 仅按 provider 过滤。
    pub fn by_provider(provider_id: ProviderId) -> Self {
        Self {
            provider_id: Some(provider_id),
            ..Self::default()
        }
    }

    /// 仅按 model 过滤。
    pub fn by_model(model_id: ModelId) -> Self {
        Self {
            model_id: Some(model_id),
            ..Self::default()
        }
    }

    /// 仅按币种过滤。
    pub fn by_currency(currency: impl Into<String>) -> Self {
        Self {
            currency: Some(currency.into()),
            ..Self::default()
        }
    }

    /// 仅按发生时间半开区间 `[start_ms, end_ms)` 过滤（含起点、不含终点）。
    pub fn by_occurred_between(start_ms: u64, end_ms: u64) -> Self {
        Self {
            occurred_at_start_ms: Some(start_ms),
            occurred_at_end_ms: Some(end_ms),
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

    /// 幂等冲突：相同 (tenant, account) 作用域内 `record_id` 已存在但内容不同。
    #[error("usage record id conflict: {record_id}")]
    Conflict { record_id: String },

    /// 聚合命中多条记录且币种不一致，成本（cost_micros）不可混加。
    #[error("aggregate spans multiple currencies: {currencies:?}")]
    MixedCurrencies { currencies: Vec<String> },
}

/// 只读多维用量账本接口。
#[async_trait]
pub trait UsageLedger: Send + Sync {
    /// 写入一条记录；校验失败返回 `InvalidRecord`。
    ///
    /// `record_id` 是 (tenant, account) 作用域内的幂等键：相同 ID 与相同内容
    /// 重复写入为重放成功且不重复记账；相同 ID 但内容不同返回 `Conflict`；
    /// 空 ID 由账本自动补写。
    async fn record(&self, record: UsageRecord) -> Result<(), UsageLedgerError>;

    /// 按查询条件返回全部命中的记录。
    async fn query(&self, query: &UsageQuery) -> Vec<UsageRecord>;

    /// 按查询条件聚合用量与成本。
    ///
    /// 命中记录币种不一致（且查询未按币种过滤）时返回 `MixedCurrencies`；
    /// 单币种与空集返回正常聚合结果。
    async fn aggregate(&self, query: &UsageQuery) -> Result<UsageTotals, UsageLedgerError>;
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
        if record.account_id.is_empty() {
            return Err(UsageLedgerError::InvalidRecord {
                reason: "account_id 不能为空".to_string(),
            });
        }
        if record.provider_id.as_str().is_empty() {
            return Err(UsageLedgerError::InvalidRecord {
                reason: "provider_id 不能为空".to_string(),
            });
        }
        if record.model_id.as_str().is_empty() {
            return Err(UsageLedgerError::InvalidRecord {
                reason: "model_id 不能为空".to_string(),
            });
        }
        if record.currency.len() != 3
            || !record
                .currency
                .bytes()
                .all(|byte| byte.is_ascii_uppercase())
        {
            return Err(UsageLedgerError::InvalidRecord {
                reason: "currency 必须为 3 位大写 ASCII 字母".to_string(),
            });
        }
        let total_tokens = record
            .input_tokens
            .saturating_add(record.output_tokens)
            .saturating_add(record.cache_read_tokens)
            .saturating_add(record.cache_write_tokens);
        if total_tokens == 0 && record.cost_micros == 0 {
            return Err(UsageLedgerError::InvalidRecord {
                reason: "token 用量与成本（cost_micros）必须至少一项大于 0".to_string(),
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
        if let Some(ref credential_id) = query.credential_id {
            if record.credential_id.as_ref() != Some(credential_id) {
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
        if let Some(ref run_id) = query.run_id {
            if record.run_id.as_ref() != Some(run_id) {
                return false;
            }
        }
        if let Some(ref provider_id) = query.provider_id {
            if record.provider_id != *provider_id {
                return false;
            }
        }
        if let Some(ref model_id) = query.model_id {
            if record.model_id != *model_id {
                return false;
            }
        }
        if let Some(ref currency) = query.currency {
            if record.currency != *currency {
                return false;
            }
        }
        if let Some(start_ms) = query.occurred_at_start_ms {
            if record.occurred_at_ms < start_ms {
                return false;
            }
        }
        if let Some(end_ms) = query.occurred_at_end_ms {
            // 半开区间：终点不包含。
            if record.occurred_at_ms >= end_ms {
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
            record.record_id = format!(
                "{AUTO_RECORD_ID_PREFIX}{}",
                NEXT_RECORD_ID.fetch_add(1, Ordering::Relaxed)
            );
        }
        // std Mutex 同步锁不跨 await，整段操作保持同步。
        let mut records = self.records.lock().expect("usage ledger mutex poisoned");
        // 幂等键作用域为 (tenant_id, account_id, record_id)：跨 tenant/account
        // 的同 ID 记录相互独立，互不构成冲突（严格隔离）。
        let existing = records.iter().find(|r| {
            r.tenant_id == record.tenant_id
                && r.account_id == record.account_id
                && r.record_id == record.record_id
        });
        if let Some(existing) = existing {
            if existing.same_content(&record) {
                // 相同 ID + 相同内容：幂等重放成功，不重复写入。
                return Ok(());
            }
            return Err(UsageLedgerError::Conflict {
                record_id: record.record_id,
            });
        }
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

    async fn aggregate(&self, query: &UsageQuery) -> Result<UsageTotals, UsageLedgerError> {
        let mut totals = UsageTotals::default();
        let mut currencies = std::collections::BTreeSet::new();
        for record in self.query(query).await {
            currencies.insert(record.currency.clone());
            totals.add(&record);
        }
        if currencies.len() > 1 {
            return Err(UsageLedgerError::MixedCurrencies {
                currencies: currencies.into_iter().collect(),
            });
        }
        Ok(totals)
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
            credential_id: None,
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

        let totals = ledger.aggregate(&UsageQuery::default()).await.unwrap();
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
            .await
            .unwrap();
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

        // account/provider/model 都必须非空。
        let mut bad_account = make_record(
            "account",
            "tenant-a",
            "principal-1",
            "account-x",
            "session-1",
            "agent-1",
        );
        bad_account.account_id.clear();
        let err = ledger.record(bad_account).await.unwrap_err();
        assert!(matches!(err, UsageLedgerError::InvalidRecord { .. }));

        let mut bad_provider = make_record(
            "provider",
            "tenant-a",
            "principal-1",
            "account-x",
            "session-1",
            "agent-1",
        );
        bad_provider.provider_id = ProviderId::new("");
        let err = ledger.record(bad_provider).await.unwrap_err();
        assert!(matches!(err, UsageLedgerError::InvalidRecord { .. }));

        let mut bad_model = make_record(
            "model",
            "tenant-a",
            "principal-1",
            "account-x",
            "session-1",
            "agent-1",
        );
        bad_model.model_id = ModelId::new("");
        let err = ledger.record(bad_model).await.unwrap_err();
        assert!(matches!(err, UsageLedgerError::InvalidRecord { .. }));

        // currency 必须恰好为 3 位大写 ASCII 字母。
        for (index, currency) in ["usd", "US", "US1", "USDD", "€UR"].into_iter().enumerate() {
            let mut bad_currency = make_record(
                &format!("currency-{index}"),
                "tenant-a",
                "principal-1",
                "account-x",
                "session-1",
                "agent-1",
            );
            bad_currency.currency = currency.to_string();
            let err = ledger.record(bad_currency).await.unwrap_err();
            assert!(matches!(err, UsageLedgerError::InvalidRecord { .. }));
        }

        // token 用量与成本（cost_micros）皆为 0 的记录被拒绝。
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
        zero_tokens.cost_micros = 0;
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

    #[tokio::test]
    async fn cost_only_and_token_only_records_accepted() {
        let ledger = InMemoryUsageLedger::new();

        // cost-only：无 token 但有成本，可写入（预算 flush 的 cost-only 场景）。
        let mut cost_only = make_record(
            "cost-only",
            "tenant-a",
            "principal-1",
            "account-x",
            "session-1",
            "agent-1",
        );
        cost_only.input_tokens = 0;
        cost_only.output_tokens = 0;
        cost_only.cache_read_tokens = 0;
        cost_only.cache_write_tokens = 0;
        ledger.record(cost_only).await.unwrap();

        // token-only：有 token 但成本为 0，同样可写入。
        let mut token_only = make_record(
            "token-only",
            "tenant-a",
            "principal-1",
            "account-x",
            "session-1",
            "agent-1",
        );
        token_only.cost_micros = 0;
        ledger.record(token_only).await.unwrap();

        let totals = ledger.aggregate(&UsageQuery::default()).await.unwrap();
        assert_eq!(totals.input_tokens, 100, "token-only 记录的 token 应计入");
        assert_eq!(totals.output_tokens, 50);
        assert_eq!(totals.cost_micros, 1_250, "cost-only 记录的成本应计入");
    }

    #[tokio::test]
    async fn filter_by_all_dimensions_and_occurred_range() {
        let ledger = InMemoryUsageLedger::new();

        // 目标记录：全维度命中。
        let mut target = make_record(
            "r1",
            "tenant-a",
            "principal-1",
            "account-x",
            "session-1",
            "agent-1",
        );
        target.run_id = Some(RunId::new("run-1"));
        target.credential_id = Some("credential-1".to_string());
        target.provider_id = ProviderId::new("anthropic");
        target.model_id = ModelId::new("claude-3.5-sonnet");
        target.occurred_at_ms = 2_000;
        ledger.record(target.clone()).await.unwrap();

        // 干扰记录：每个维度各差一项。
        let mut wrong_provider = make_record(
            "r2",
            "tenant-a",
            "principal-1",
            "account-x",
            "session-1",
            "agent-1",
        );
        wrong_provider.run_id = Some(RunId::new("run-1"));
        wrong_provider.credential_id = Some("credential-1".to_string());
        wrong_provider.provider_id = ProviderId::new("openai");
        wrong_provider.model_id = ModelId::new("claude-3.5-sonnet");
        wrong_provider.occurred_at_ms = 2_000;
        ledger.record(wrong_provider).await.unwrap();

        let mut wrong_run = make_record(
            "r3",
            "tenant-a",
            "principal-1",
            "account-x",
            "session-1",
            "agent-1",
        );
        wrong_run.run_id = Some(RunId::new("run-2"));
        wrong_run.credential_id = Some("credential-1".to_string());
        wrong_run.provider_id = ProviderId::new("anthropic");
        wrong_run.model_id = ModelId::new("claude-3.5-sonnet");
        wrong_run.occurred_at_ms = 2_000;
        ledger.record(wrong_run).await.unwrap();

        let mut wrong_model = make_record(
            "r4",
            "tenant-a",
            "principal-1",
            "account-x",
            "session-1",
            "agent-1",
        );
        wrong_model.run_id = Some(RunId::new("run-1"));
        wrong_model.credential_id = Some("credential-1".to_string());
        wrong_model.provider_id = ProviderId::new("anthropic");
        wrong_model.model_id = ModelId::new("gpt-4o");
        wrong_model.occurred_at_ms = 2_000;
        ledger.record(wrong_model).await.unwrap();

        // 时间区间外的同维度记录。
        let mut outside_range = make_record(
            "r5",
            "tenant-a",
            "principal-1",
            "account-x",
            "session-1",
            "agent-1",
        );
        outside_range.run_id = Some(RunId::new("run-1"));
        outside_range.credential_id = Some("credential-1".to_string());
        outside_range.provider_id = ProviderId::new("anthropic");
        outside_range.model_id = ModelId::new("claude-3.5-sonnet");
        outside_range.occurred_at_ms = 4_000;
        ledger.record(outside_range).await.unwrap();

        // run_id 为 None 的同维度记录（run 过滤不应命中）。
        let mut no_run = make_record(
            "r6",
            "tenant-a",
            "principal-1",
            "account-x",
            "session-1",
            "agent-1",
        );
        no_run.credential_id = Some("credential-1".to_string());
        no_run.provider_id = ProviderId::new("anthropic");
        no_run.model_id = ModelId::new("claude-3.5-sonnet");
        no_run.occurred_at_ms = 2_000;
        ledger.record(no_run).await.unwrap();

        let mut wrong_credential = target.clone();
        wrong_credential.record_id = "r7".to_string();
        wrong_credential.credential_id = Some("credential-2".to_string());
        ledger.record(wrong_credential).await.unwrap();

        let mut wrong_currency = target.clone();
        wrong_currency.record_id = "r8".to_string();
        wrong_currency.currency = "EUR".to_string();
        ledger.record(wrong_currency).await.unwrap();

        let query = UsageQuery {
            tenant_id: Some(TenantId::new("tenant-a")),
            principal_id: Some(PrincipalId::new("principal-1")),
            account_id: Some("account-x".to_string()),
            credential_id: Some("credential-1".to_string()),
            session_id: Some(SessionId::new("session-1")),
            agent_id: Some(AgentId::new("agent-1")),
            run_id: Some(RunId::new("run-1")),
            provider_id: Some(ProviderId::new("anthropic")),
            model_id: Some(ModelId::new("claude-3.5-sonnet")),
            currency: Some("USD".to_string()),
            occurred_at_start_ms: Some(2_000),
            occurred_at_end_ms: Some(3_000),
        };
        let records = ledger.query(&query).await;
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].record_id, "r1");

        let totals = ledger.aggregate(&query).await.unwrap();
        assert_eq!(totals.input_tokens, 100);
        assert_eq!(totals.cost_micros, 1_250);
    }

    #[tokio::test]
    async fn occurred_at_half_open_boundaries() {
        let ledger = InMemoryUsageLedger::new();
        for (id, occurred_at_ms) in [("t1", 100u64), ("t2", 200), ("t3", 300)] {
            let mut record = make_record(
                id,
                "tenant-a",
                "principal-1",
                "account-x",
                "session-1",
                "agent-1",
            );
            record.occurred_at_ms = occurred_at_ms;
            ledger.record(record).await.unwrap();
        }

        // [200, 300)：含起点、不含终点。
        let records = ledger
            .query(&UsageQuery::by_occurred_between(200, 300))
            .await;
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].record_id, "t2");

        // 空区间 [300, 300)：无命中。
        let records = ledger
            .query(&UsageQuery::by_occurred_between(300, 300))
            .await;
        assert!(records.is_empty());

        // 仅起点（含）：>= 100 全部命中。
        let from = UsageQuery {
            occurred_at_start_ms: Some(100),
            ..UsageQuery::default()
        };
        assert_eq!(ledger.query(&from).await.len(), 3);

        // 仅终点（不含）：< 300 命中前两条。
        let to = UsageQuery {
            occurred_at_end_ms: Some(300),
            ..UsageQuery::default()
        };
        assert_eq!(ledger.query(&to).await.len(), 2);

        // 全覆盖区间。
        assert_eq!(
            ledger
                .query(&UsageQuery::by_occurred_between(0, u64::MAX))
                .await
                .len(),
            3
        );
    }

    #[tokio::test]
    async fn replay_same_id_same_content_is_idempotent() {
        let ledger = InMemoryUsageLedger::new();
        let record = make_record(
            "rec-replay",
            "tenant-a",
            "principal-1",
            "account-x",
            "session-1",
            "agent-1",
        );

        ledger.record(record.clone()).await.unwrap();
        let replay = ledger.record(record).await;
        assert!(replay.is_ok(), "相同 ID + 相同内容应重放成功");

        let records = ledger.query(&UsageQuery::default()).await;
        assert_eq!(records.len(), 1, "重放不得重复写入");
        let totals = ledger.aggregate(&UsageQuery::default()).await.unwrap();
        assert_eq!(totals.input_tokens, 100, "聚合只计一次");
    }

    #[tokio::test]
    async fn replay_same_id_different_content_conflicts() {
        let ledger = InMemoryUsageLedger::new();
        let original = make_record(
            "rec-conflict",
            "tenant-a",
            "principal-1",
            "account-x",
            "session-1",
            "agent-1",
        );
        ledger.record(original.clone()).await.unwrap();

        let mut changed = original.clone();
        changed.credential_id = Some("credential-2".to_string());
        let err = ledger.record(changed).await.unwrap_err();
        assert!(
            matches!(&err, UsageLedgerError::Conflict { record_id } if record_id == "rec-conflict")
        );
        assert_eq!(
            err.to_string(),
            "usage record id conflict: rec-conflict",
            "冲突应为结构化错误"
        );

        let records = ledger.query(&UsageQuery::default()).await;
        assert_eq!(records.len(), 1, "冲突记录不得写入");
        assert_eq!(records[0], original);
    }

    #[tokio::test]
    async fn record_id_namespace_isolated_across_tenant_and_account() {
        let ledger = InMemoryUsageLedger::new();

        // 相同 record_id 在不同 (tenant, account) 下是独立记录，不冲突。
        ledger
            .record(make_record(
                "dup",
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
                "dup",
                "tenant-b",
                "principal-1",
                "account-x",
                "session-1",
                "agent-1",
            ))
            .await
            .unwrap();
        ledger
            .record(make_record(
                "dup",
                "tenant-a",
                "principal-1",
                "account-y",
                "session-1",
                "agent-1",
            ))
            .await
            .unwrap();
        assert_eq!(
            ledger.query(&UsageQuery::default()).await.len(),
            3,
            "跨 tenant/account 的同 ID 各自独立落账"
        );

        // 相同 (tenant, account) 内同 ID + 同内容仍是幂等重放。
        let replay = ledger
            .record(make_record(
                "dup",
                "tenant-a",
                "principal-1",
                "account-x",
                "session-1",
                "agent-1",
            ))
            .await;
        assert!(replay.is_ok());
        assert_eq!(ledger.query(&UsageQuery::default()).await.len(), 3);

        // 相同 (tenant, account) 内同 ID + 不同内容冲突。
        let mut conflicting = make_record(
            "dup",
            "tenant-a",
            "principal-1",
            "account-x",
            "session-1",
            "agent-1",
        );
        conflicting.input_tokens = 999;
        let err = ledger.record(conflicting).await.unwrap_err();
        assert!(matches!(err, UsageLedgerError::Conflict { .. }));
        assert_eq!(ledger.query(&UsageQuery::default()).await.len(), 3);

        // tenant 视角严格隔离：tenant-b 只能看到自己的记录。
        let b_records = ledger
            .query(&UsageQuery::by_tenant(TenantId::new("tenant-b")))
            .await;
        assert_eq!(b_records.len(), 1);
        assert_eq!(b_records[0].account_id, "account-x");
        assert_eq!(b_records[0].tenant_id, TenantId::new("tenant-b"));

        // account 视角严格隔离。
        let y_records = ledger
            .query(&UsageQuery {
                account_id: Some("account-y".to_string()),
                ..UsageQuery::default()
            })
            .await;
        assert_eq!(y_records.len(), 1);
        assert_eq!(y_records[0].tenant_id, TenantId::new("tenant-a"));
    }

    #[tokio::test]
    async fn empty_record_id_generates_distinct_ids() {
        let ledger = InMemoryUsageLedger::new();
        let first = make_record(
            "",
            "tenant-a",
            "principal-1",
            "account-x",
            "session-1",
            "agent-1",
        );
        let second = make_record(
            "",
            "tenant-b",
            "principal-2",
            "account-y",
            "session-2",
            "agent-2",
        );
        ledger.record(first).await.unwrap();
        ledger.record(second).await.unwrap();

        let records = ledger.query(&UsageQuery::default()).await;
        assert_eq!(records.len(), 2);
        assert_ne!(
            records[0].record_id, records[1].record_id,
            "空 ID 自动补写必须互不相同"
        );
        assert!(records.iter().all(|r| !r.record_id.is_empty()));
    }

    #[tokio::test]
    async fn auto_generated_ids_use_reserved_prefix() {
        let ledger = InMemoryUsageLedger::new();

        // 空 ID 自动补写：必须落在保留前缀命名空间。
        ledger
            .record(make_record(
                "",
                "tenant-a",
                "principal-1",
                "account-x",
                "session-1",
                "agent-1",
            ))
            .await
            .unwrap();
        let records = ledger.query(&UsageQuery::default()).await;
        assert!(records[0].record_id.starts_with(AUTO_RECORD_ID_PREFIX));

        // `UsageRecord::default` 同样生成保留前缀 ID。
        assert!(UsageRecord::default()
            .record_id
            .starts_with(AUTO_RECORD_ID_PREFIX));

        // 显式 rec-* ID（旧计数器可能生成的数值形式）与自动 ID 互不冲突。
        let explicit = make_record(
            "rec-0",
            "tenant-b",
            "principal-1",
            "account-x",
            "session-1",
            "agent-1",
        );
        ledger.record(explicit).await.unwrap();
        let records = ledger.query(&UsageQuery::default()).await;
        assert_eq!(records.len(), 2);
        assert!(records.iter().any(|r| r.record_id == "rec-0"));
        assert!(records
            .iter()
            .any(|r| r.record_id.starts_with(AUTO_RECORD_ID_PREFIX)));
    }

    #[tokio::test]
    async fn aggregate_empty_set_returns_default_totals() {
        let ledger = InMemoryUsageLedger::new();
        assert_eq!(
            ledger.aggregate(&UsageQuery::default()).await.unwrap(),
            UsageTotals::default()
        );

        // 无命中（过滤条件不匹配）同样返回空聚合。
        ledger
            .record(make_record(
                "only",
                "tenant-a",
                "principal-1",
                "account-x",
                "session-1",
                "agent-1",
            ))
            .await
            .unwrap();
        assert_eq!(
            ledger
                .aggregate(&UsageQuery::by_tenant(TenantId::new("tenant-b")))
                .await
                .unwrap(),
            UsageTotals::default()
        );
    }

    #[tokio::test]
    async fn aggregate_mixed_currencies_rejected_explicitly() {
        let ledger = InMemoryUsageLedger::new();

        let mut usd = make_record(
            "m1",
            "tenant-a",
            "principal-1",
            "account-x",
            "session-1",
            "agent-1",
        );
        usd.cost_micros = 1_000;
        ledger.record(usd).await.unwrap();

        let mut eur = make_record(
            "m2",
            "tenant-a",
            "principal-1",
            "account-x",
            "session-1",
            "agent-1",
        );
        eur.currency = "EUR".to_string();
        eur.cost_micros = 2_000;
        ledger.record(eur).await.unwrap();

        // 未按币种过滤的聚合必须显式报错，不得静默混加成本。
        let err = ledger.aggregate(&UsageQuery::default()).await.unwrap_err();
        match &err {
            UsageLedgerError::MixedCurrencies { currencies } => {
                assert_eq!(
                    currencies,
                    &vec!["EUR".to_string(), "USD".to_string()],
                    "币种列表应有序且完整"
                );
            }
            other => panic!("expected MixedCurrencies, got {other:?}"),
        }
        assert!(err.to_string().contains("EUR"));
        assert!(err.to_string().contains("USD"));

        // 单币种过滤后聚合保持可用。
        let usd_totals = ledger
            .aggregate(&UsageQuery::by_currency("USD"))
            .await
            .unwrap();
        assert_eq!(usd_totals.cost_micros, 1_000);
        assert_eq!(usd_totals.input_tokens, 100);
    }

    #[test]
    fn credential_id_defaults_when_deserializing_legacy_record() {
        let record = make_record(
            "legacy",
            "tenant-a",
            "principal-1",
            "account-x",
            "session-1",
            "agent-1",
        );
        let mut json = serde_json::to_value(record).unwrap();
        json.as_object_mut()
            .expect("usage record serializes as object")
            .remove("credential_id");

        let decoded: UsageRecord = serde_json::from_value(json).unwrap();
        assert_eq!(decoded.credential_id, None);
    }

    #[tokio::test]
    async fn credential_account_tenant_filters_are_strictly_isolated() {
        let ledger = InMemoryUsageLedger::new();

        let mut target = make_record(
            "target",
            "tenant-a",
            "principal-1",
            "account-x",
            "session-1",
            "agent-1",
        );
        target.credential_id = Some("credential-1".to_string());
        target.input_tokens = 10;
        target.cost_micros = 100;
        ledger.record(target).await.unwrap();

        let mut other_credential = make_record(
            "other-credential",
            "tenant-a",
            "principal-1",
            "account-x",
            "session-1",
            "agent-1",
        );
        other_credential.credential_id = Some("credential-2".to_string());
        other_credential.input_tokens = 20;
        other_credential.cost_micros = 200;
        ledger.record(other_credential).await.unwrap();

        let mut other_account = make_record(
            "other-account",
            "tenant-a",
            "principal-1",
            "account-y",
            "session-1",
            "agent-1",
        );
        other_account.credential_id = Some("credential-1".to_string());
        other_account.input_tokens = 30;
        other_account.cost_micros = 300;
        ledger.record(other_account).await.unwrap();

        let mut other_tenant = make_record(
            "other-tenant",
            "tenant-b",
            "principal-1",
            "account-x",
            "session-1",
            "agent-1",
        );
        other_tenant.credential_id = Some("credential-1".to_string());
        other_tenant.input_tokens = 40;
        other_tenant.cost_micros = 400;
        ledger.record(other_tenant).await.unwrap();

        let query = UsageQuery {
            tenant_id: Some(TenantId::new("tenant-a")),
            account_id: Some("account-x".to_string()),
            credential_id: Some("credential-1".to_string()),
            ..UsageQuery::default()
        };
        let records = ledger.query(&query).await;
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].record_id, "target");

        let totals = ledger.aggregate(&query).await.unwrap();
        assert_eq!(totals.input_tokens, 10);
        assert_eq!(totals.cost_micros, 100);

        let credential_records = ledger
            .query(&UsageQuery::by_credential("credential-1"))
            .await;
        assert_eq!(credential_records.len(), 3);
        assert!(credential_records
            .iter()
            .all(|record| record.credential_id.as_deref() == Some("credential-1")));
    }

    #[tokio::test]
    async fn currency_filter_prevents_mixed_currency_aggregation() {
        let ledger = InMemoryUsageLedger::new();

        let mut usd = make_record(
            "usd",
            "tenant-a",
            "principal-1",
            "account-x",
            "session-1",
            "agent-1",
        );
        usd.credential_id = Some("credential-1".to_string());
        usd.input_tokens = 100;
        usd.cost_micros = 1_250;
        ledger.record(usd).await.unwrap();

        let mut eur = make_record(
            "eur",
            "tenant-a",
            "principal-1",
            "account-x",
            "session-1",
            "agent-1",
        );
        eur.credential_id = Some("credential-1".to_string());
        eur.input_tokens = 200;
        eur.cost_micros = 2_500;
        eur.currency = "EUR".to_string();
        ledger.record(eur).await.unwrap();

        let usd_totals = ledger
            .aggregate(&UsageQuery::by_currency("USD"))
            .await
            .unwrap();
        assert_eq!(usd_totals.input_tokens, 100);
        assert_eq!(usd_totals.cost_micros, 1_250);

        let eur_totals = ledger
            .aggregate(&UsageQuery::by_currency("EUR"))
            .await
            .unwrap();
        assert_eq!(eur_totals.input_tokens, 200);
        assert_eq!(eur_totals.cost_micros, 2_500);
    }

    #[tokio::test]
    async fn aggregate_saturates_instead_of_overflowing() {
        let ledger = InMemoryUsageLedger::new();
        for id in ["max-1", "max-2"] {
            let mut record = make_record(
                id,
                "tenant-a",
                "principal-1",
                "account-x",
                "session-1",
                "agent-1",
            );
            record.input_tokens = u64::MAX;
            record.cost_micros = u64::MAX;
            ledger.record(record).await.unwrap();
        }

        let totals = ledger.aggregate(&UsageQuery::default()).await.unwrap();
        assert_eq!(totals.input_tokens, u64::MAX, "聚合必须 saturating");
        assert_eq!(totals.cost_micros, u64::MAX);
        assert_eq!(totals.output_tokens, 100);
    }
}
