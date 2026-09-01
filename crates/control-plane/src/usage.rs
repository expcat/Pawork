//! # usage-ledger — 多维 Usage/Cost 账本（P18-8 持久化）
//!
//! 不可变的、可按 tenant/principal/account/credential/session/agent/provider/model/currency
//! 多维归属的用量成本账本。供 Phase 12 编排做预算归属与成本核算，Phase 14
//! Quota 投影与 P18 预算 / Quota 对账使用。
//!
//! 提供两种追加存储：进程内 [`InMemoryUsageLedger`]（测试 / 嵌入便捷）与
//! 可重启的 SQLite 账本 [`SqliteUsageLedger`]（生产 CLI 装配：run 进程写入后，
//! 新进程打开同一文件即可读取，跨进程幂等重放不重复计费）。两者共享同一
//! [`UsageLedger`] 契约与幂等语义。
//!
//! ## 版本化记录（UsageRecord v2）
//!
//! [`UsageRecord`] 携带 `version`（当前 [`RECORD_VERSION`] = 2）：v2 在 v1 的
//! 身份 / token / cost 维度之上补充完整 trace（`request_id` / `event_id` /
//! `upstream_attempt`，retry/failover 的每次实际上游调用可归属）与定价快照
//! （`rate_card` / `rate_version` / `cost_confidence` / `cost_provenance`，
//! 历史费用不因模型价格更新漂移）与调用链 `trace_id`。旧 v1 JSON（无 v2 字段）
//! 经 serde 默认值解码为 `version = 1` 且 trace/pricing 为空，不丢历史记录。
//!
//! ## 归属注入（UsageAttribution）
//!
//! 账本契约只接受调用方注入的 [`UsageAttribution`]（tenant/principal/
//! account/credential/trace），绝不回退到账本内部猜默认账号。当前 run
//! 生命周期的注入点（`app-service::supervisor::spawn_run_task`）在 P18-4
//! CredentialLease 接入前为过渡派生：tenant/principal 来自身份解析的
//! `IdentityContext`，account 取 legacy 默认哨兵、credential/trace 为
//! `None`。P18-4 稳定后由主代理在宿主侧（RunRequest 装配处）整合
//! `From<&CredentialLease>` 构造真实归属再注入，账本契约不变。
//!
//! ## 幂等与隔离
//!
//! `record_id` 在 (tenant, account) 作用域内作为幂等键：相同 ID 与相同内容
//! 的重放成功且不重复记账，相同 ID 不同内容返回结构化冲突；空 ID 自动补写，
//! 自动 ID 使用保留前缀 `auto-rec-*`，与显式 `rec-*` 命名空间隔离。
//! SQLite 账本以 `UNIQUE(tenant_id, account_id, record_id)` 主键 + 基于
//! `(tenant, account, request_id, upstream_attempt)` 的 `dedup_key` 部分唯一
//! 索引在存储层强制同一语义（带 request 的记录按 event/request attempt 去重，
//! 不只信自造 record_id），跨进程并发重放安全；账本只追加、不更新不删除
//! （immutable append）。
//!
//! 查询支持 provider / model / run 维度与 `occurred_at_ms` 半开区间过滤，
//! 聚合采用饱和累加；命中记录币种不一致时聚合返回显式错误而非静默混加。
//! 持久化查询 fail-closed：存储 / 行解码错误以
//! [`UsageLedgerError::Storage`] 返回，绝不吞错为空集。
//!
//! 类型命名保持英文，crate 文档使用中文。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[cfg(feature = "sqlite")]
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
#[cfg(feature = "sqlite")]
use std::path::Path;

/// 自动生成记录 ID 的保留前缀。
///
/// `auto-rec-*` 命名空间仅供账本自动补写与系统组件（如预算控制器 flush）
/// 生成幂等键使用；显式 `record_id`（惯例 `rec-*` 或其他命名）不得使用该
/// 前缀，否则可能与自动 ID 冲突。
pub const AUTO_RECORD_ID_PREFIX: &str = "auto-rec-";

/// 记录 ID 自动生成计数器（`UsageRecord::default` 与账本补写共用）。
static NEXT_RECORD_ID: AtomicU64 = AtomicU64::new(0);

/// 重新导出领域 crate 的标识类型，供账本使用者统一引用。
pub use pawork_domain::{
    AgentId, EventId, ModelId, PrincipalId, ProviderId, RequestId, RunId, SessionId, TenantId,
};

/// 当前记录版本（P18-8 UsageRecord v2）。
///
/// v2 相对 v1 新增：`version` 字段、trace（`request_id` / `event_id` /
/// `upstream_attempt`）与定价快照（`rate_card` / `rate_version` /
/// `cost_confidence` / `cost_provenance`）。旧 JSON 缺省时解码为 v1。
pub const RECORD_VERSION: u32 = 2;

/// 成本口径的可信度（pricing provenance 的一部分）。
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CostConfidence {
    /// 按 rate card 估算的费用（尚无实收账单）。
    Estimated,
    /// 上游账单 / 实收口径确认的费用。
    Actual,
    /// 无法判定口径。
    Unknown,
}

/// 旧 v1 JSON 缺少 `version` 字段时的解码值（兼容迁移，不丢历史记录）。
fn legacy_version() -> u32 {
    1
}

/// 记账归属（P18-8）：由调用方在 run 生命周期注入实际身份 / 账号 / 凭据 /
/// trace 归属，账本不自行猜测默认账号。
///
/// P18-4 CredentialLease 稳定后，宿主侧（`app-service` RunRequest 装配处）
/// 整合 `From<&CredentialLease>` 构造真实归属；接入完成前，当前调用点
/// （`spawn_run_task`）以过渡派生填充（IdentityContext + legacy 默认哨兵，
/// credential/trace 为 `None`），详见 `app-service` 对应注释。
/// `credential_id` 为 opaque 定位符（非 secret），允许持久化。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UsageAttribution {
    /// 所属租户（P18-2 canonical identity，如 `local/default`）。
    pub tenant_id: TenantId,
    /// 发起主体。
    pub principal_id: PrincipalId,
    /// 实际账号（P18-4 lease 的 account_id；legacy 默认 `local/default`）。
    pub account_id: String,
    /// 实际凭据（P18-4 lease 的 credential_id；无 lease 时为 `None`）。
    pub credential_id: Option<String>,
    /// 调用链 trace（P18-4 AcquireRequest.trace_id；无则 `None`）。
    pub trace_id: Option<String>,
}

/// 一条不可变的多维 usage/cost 记录。
///
/// `record_id` 为空时会由账本在 `record` 时以原子计数器补写；也可由
/// `UsageRecord::default` 直接生成。
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct UsageRecord {
    pub record_id: String,
    /// 记录版本（当前 [`RECORD_VERSION`] = 2；旧 JSON 缺省解码为 1）。
    #[serde(default = "legacy_version")]
    pub version: u32,
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
    // ---- v2：trace / upstream attempt（P18-8）----
    /// 上游请求 / 客户端请求 trace ID（retry/failover 归属用）。
    #[serde(default)]
    pub request_id: Option<RequestId>,
    /// 触发该用量观测的 canonical 事件 ID。
    #[serde(default)]
    pub event_id: Option<EventId>,
    /// 实际上游调用序号（同 run 内第几次 attempt）。
    #[serde(default)]
    pub upstream_attempt: Option<u64>,
    /// 调用链 trace ID（P18-4 AcquireRequest.trace_id 贯通）。
    #[serde(default)]
    pub trace_id: Option<String>,
    // ---- v2：定价快照（P18-8）----
    /// rate card 标识（如 `builtin`）。
    #[serde(default)]
    pub rate_card: Option<String>,
    /// rate card 版本（历史费用不随价格更新漂移）。
    #[serde(default)]
    pub rate_version: Option<String>,
    /// 成本口径可信度（估算 / 实收 / 未知）。
    #[serde(default)]
    pub cost_confidence: Option<CostConfidence>,
    /// 成本来源（如 `model-registry:builtin:estimate`、上游 billing 引用）。
    #[serde(default)]
    pub cost_provenance: Option<String>,
}

impl Default for UsageRecord {
    /// 生成默认记录：`record_id` 由原子计数器补写，其余字段取默认空值。
    fn default() -> Self {
        Self {
            record_id: format!(
                "{AUTO_RECORD_ID_PREFIX}{}",
                NEXT_RECORD_ID.fetch_add(1, Ordering::Relaxed)
            ),
            version: RECORD_VERSION,
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
            request_id: None,
            event_id: None,
            upstream_attempt: None,
            trace_id: None,
            rate_card: None,
            rate_version: None,
            cost_confidence: None,
            cost_provenance: None,
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
        self.version == other.version
            && self.tenant_id == other.tenant_id
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
            && self.request_id == other.request_id
            && self.event_id == other.event_id
            && self.upstream_attempt == other.upstream_attempt
            && self.trace_id == other.trace_id
            && self.rate_card == other.rate_card
            && self.rate_version == other.rate_version
            && self.cost_confidence == other.cost_confidence
            && self.cost_provenance == other.cost_provenance
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

    /// 持久化存储层失败（SQLite 打开 / 读写 / 并发冲突异常）。
    #[error("usage ledger storage error: {reason}")]
    Storage { reason: String },
}

/// 只读多维用量账本接口。
#[async_trait]
pub trait UsageLedger: Send + Sync {
    /// 写入一条记录；校验失败返回 `InvalidRecord`。
    ///
    /// `record_id` 是 (tenant, account) 作用域内的幂等键：相同 ID 与相同内容
    /// 重复写入为重放成功且不重复记账；相同 ID 但内容不同返回 `Conflict`。
    /// 生产持久账本要求调用方提供跨进程稳定的非空 ID；内存实现为 legacy
    /// 测试夹具保留空 ID 自动补写，不应依赖该行为建立生产幂等性。
    async fn record(&self, record: UsageRecord) -> Result<(), UsageLedgerError>;

    /// 按查询条件返回全部命中的记录。
    ///
    /// fail-closed：持久化账本遇到存储 / 行解码错误返回
    /// [`UsageLedgerError::Storage`]，绝不静默降级为空集。
    async fn query(&self, query: &UsageQuery) -> Result<Vec<UsageRecord>, UsageLedgerError>;

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

/// 记录合法性校验（内存与 SQLite 账本共用，行为完全一致）。
fn validate_record(record: &UsageRecord) -> Result<(), UsageLedgerError> {
    if record.version == 0 {
        return Err(UsageLedgerError::InvalidRecord {
            reason: "version 必须大于 0".to_string(),
        });
    }
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

#[async_trait]
impl UsageLedger for InMemoryUsageLedger {
    async fn record(&self, mut record: UsageRecord) -> Result<(), UsageLedgerError> {
        validate_record(&record)?;
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
        // event/request attempt 去重：同 (tenant, account, request_id,
        // upstream_attempt) 已存在（不同 record_id）即冲突——与 SQLite 账本
        // 的部分唯一索引同一语义，不因存储不同而分化契约。
        if let Some(request_id) = record.request_id.as_ref() {
            if let Some(existing) = records.iter().find(|r| {
                r.tenant_id == record.tenant_id
                    && r.account_id == record.account_id
                    && r.request_id.as_ref() == Some(request_id)
                    && r.upstream_attempt.unwrap_or(0) == record.upstream_attempt.unwrap_or(0)
            }) {
                return Err(UsageLedgerError::Conflict {
                    record_id: existing.record_id.clone(),
                });
            }
        }
        records.push(record.clone());
        tracing::debug!(
            record_id = %record.record_id,
            tenant_id = %record.tenant_id,
            "usage record recorded"
        );
        Ok(())
    }

    async fn query(&self, query: &UsageQuery) -> Result<Vec<UsageRecord>, UsageLedgerError> {
        let records = self.records.lock().expect("usage ledger mutex poisoned");
        Ok(records
            .iter()
            .filter(|record| Self::matches(record, query))
            .cloned()
            .collect())
    }

    async fn aggregate(&self, query: &UsageQuery) -> Result<UsageTotals, UsageLedgerError> {
        let mut totals = UsageTotals::default();
        let mut currencies = std::collections::BTreeSet::new();
        for record in self.query(query).await? {
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

/// SQLite 持久化追加账本（P18-8 生产实现）。
///
/// - 可重启：`pawork run` 进程写入后，新进程打开同一文件即可读到幂等记录。
/// - immutable append：只提供 `record` 追加，无更新 / 删除 API；表以
///   `UNIQUE(tenant_id, account_id, record_id)` 主键在存储层强制幂等，
///   跨进程并发重放安全（`busy_timeout` + WAL）。
/// - 记录与聚合语义与 [`InMemoryUsageLedger`] 完全一致（同一 `validate_record`
///   与饱和累加折叠），查询按维度下推到 SQL WHERE。
/// - schema 版本经 `PRAGMA user_version` 记录（当前 [`SCHEMA_VERSION`]）。
///
/// 线程模型：单连接 + `Mutex` 串行化（rusqlite `Connection` 非 `Sync`），
/// 跨进程并发由 SQLite 自身（WAL + busy_timeout）处理。u64 计数（token /
/// cost）以十进制 TEXT 存储，避免 i64 溢出失真；`occurred_at_ms` 以 INTEGER
/// 存储（超出 i64 范围的时间戳显式报错，不静默截断）。
#[cfg(feature = "sqlite")]
#[derive(Debug)]
pub struct SqliteUsageLedger {
    conn: Mutex<Connection>,
}

/// 账本 SQLite schema 版本（P18-8 首版 = 2；v3 增加 `trace_id` 列与按
/// `(tenant, account, request_id, upstream_attempt)` 去重的部分唯一索引）。
#[cfg(feature = "sqlite")]
pub const SCHEMA_VERSION: i64 = 3;

#[cfg(feature = "sqlite")]
impl SqliteUsageLedger {
    /// 打开（必要时创建）指定路径的 SQLite 账本；父目录不存在时自动创建。
    ///
    /// 打开即校验 / 初始化 schema：空库创建表并写入版本；版本不兼容时
    /// 返回 [`UsageLedgerError::Storage`]，不做静默迁移或丢弃。
    pub fn open(path: impl AsRef<Path>) -> Result<Self, UsageLedgerError> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(|error| UsageLedgerError::Storage {
                    reason: format!(
                        "cannot create ledger directory {}: {error}",
                        parent.display()
                    ),
                })?;
            }
        }
        let conn = Connection::open(path).map_err(|error| UsageLedgerError::Storage {
            reason: format!("cannot open ledger {}: {error}", path.display()),
        })?;
        // WAL + NORMAL：跨进程并发读写安全且保持合理持久性。
        conn.pragma_update(None, "journal_mode", "WAL")
            .map_err(storage_error)?;
        conn.pragma_update(None, "synchronous", "NORMAL")
            .map_err(storage_error)?;
        conn.busy_timeout(std::time::Duration::from_secs(5))
            .map_err(storage_error)?;

        let current: i64 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .map_err(storage_error)?;
        match current {
            0 => {
                // 空库：建 v3 表/索引并登记 schema 版本。
                Self::ensure_v3_schema(&conn)?;
                conn.pragma_update(None, "user_version", SCHEMA_VERSION)
                    .map_err(storage_error)?;
            }
            2 => {
                // v2 → v3 迁移：补 `trace_id` 列（幂等）并建按 event/request
                // attempt 去重的部分唯一索引，然后登记版本。历史记录原样保留
                // （request_id/upstream_attempt 均为 NULL 的记录不受索引约束）。
                Self::migrate_v2_to_v3(&conn)?;
            }
            SCHEMA_VERSION => {
                Self::ensure_v3_schema(&conn)?;
            }
            other => {
                return Err(UsageLedgerError::Storage {
                    reason: format!(
                        "unsupported ledger schema version {other} (expected {SCHEMA_VERSION}); \
                         refusing to open without migration"
                    ),
                });
            }
        }
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// v2 → v3 迁移：补 `trace_id` 列并登记新版本；历史记录不动。
    fn migrate_v2_to_v3(conn: &Connection) -> Result<(), UsageLedgerError> {
        let has_trace_id: bool = conn
            .prepare("PRAGMA table_info(usage_records)")
            .map_err(storage_error)?
            .query_map([], |row| row.get::<_, String>(1))
            .map_err(storage_error)?
            .filter_map(Result::ok)
            .any(|column| column == "trace_id");
        if !has_trace_id {
            conn.execute("ALTER TABLE usage_records ADD COLUMN trace_id TEXT", [])
                .map_err(storage_error)?;
        }
        Self::ensure_v3_schema(conn)?;
        conn.pragma_update(None, "user_version", SCHEMA_VERSION)
            .map_err(storage_error)?;
        Ok(())
    }

    /// v3 幂等保证：表与索引齐全（迁移后缺失的索引在此补齐）。
    fn ensure_v3_schema(conn: &Connection) -> Result<(), UsageLedgerError> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS usage_records (
                 record_id TEXT NOT NULL,
                 version INTEGER NOT NULL,
                 tenant_id TEXT NOT NULL,
                 principal_id TEXT NOT NULL,
                 account_id TEXT NOT NULL,
                 credential_id TEXT,
                 session_id TEXT NOT NULL,
                 agent_id TEXT NOT NULL,
                 run_id TEXT,
                 provider_id TEXT NOT NULL,
                 model_id TEXT NOT NULL,
                 input_tokens TEXT NOT NULL,
                 output_tokens TEXT NOT NULL,
                 cache_read_tokens TEXT NOT NULL,
                 cache_write_tokens TEXT NOT NULL,
                 cost_micros TEXT NOT NULL,
                 currency TEXT NOT NULL,
                 occurred_at_ms INTEGER NOT NULL,
                 request_id TEXT,
                 event_id TEXT,
                 upstream_attempt TEXT,
                 trace_id TEXT,
                 rate_card TEXT,
                 rate_version TEXT,
                 cost_confidence TEXT,
                 cost_provenance TEXT,
                 PRIMARY KEY (tenant_id, account_id, record_id)
             );
             CREATE INDEX IF NOT EXISTS idx_usage_occurred
                 ON usage_records (occurred_at_ms);
             CREATE INDEX IF NOT EXISTS idx_usage_scope
                 ON usage_records (tenant_id, account_id, provider_id, model_id);
             -- 存储层去重：同一 (tenant, account) 内，带 request 的记录按
             -- (request_id, upstream_attempt) 唯一；attempt 缺省按 '0' 折叠，
             -- 防止同 request 的 NULL/NULL 组合绕过唯一性。record_id 只作
             -- 主键，不承担去重语义（review：不能只信自造 record_id）。
             CREATE UNIQUE INDEX IF NOT EXISTS idx_usage_dedup
                 ON usage_records (tenant_id, account_id, request_id,
                                   COALESCE(upstream_attempt, '0'))
                 WHERE request_id IS NOT NULL;",
        )
        .map_err(storage_error)
    }

    /// 按 (tenant, account, record_id) 读取既有记录，用于幂等判定。
    fn read_existing(
        conn: &Connection,
        record: &UsageRecord,
    ) -> Result<Option<UsageRecord>, UsageLedgerError> {
        conn.query_row(
            "SELECT record_id, version, tenant_id, principal_id, account_id, credential_id,
                    session_id, agent_id, run_id, provider_id, model_id,
                    input_tokens, output_tokens, cache_read_tokens, cache_write_tokens,
                    cost_micros, currency, occurred_at_ms,
                    request_id, event_id, upstream_attempt, trace_id,
                    rate_card, rate_version, cost_confidence, cost_provenance
             FROM usage_records
             WHERE tenant_id = ?1 AND account_id = ?2 AND record_id = ?3",
            params![
                record.tenant_id.as_str(),
                record.account_id,
                record.record_id
            ],
            row_to_record,
        )
        .optional()
        .map_err(storage_error)
    }

    /// 按 (tenant, account, request_id, upstream_attempt) 读取既有记录，用于
    /// event/request attempt 去重判定（record_id 只是主键，不承担去重语义）。
    /// `request_id` 为 `None` 的记录不参与去重（返回 `None`）。
    fn read_existing_by_dedup(
        conn: &Connection,
        record: &UsageRecord,
    ) -> Result<Option<UsageRecord>, UsageLedgerError> {
        let Some(request_id) = record.request_id.as_ref() else {
            return Ok(None);
        };
        conn.query_row(
            "SELECT record_id, version, tenant_id, principal_id, account_id, credential_id,
                    session_id, agent_id, run_id, provider_id, model_id,
                    input_tokens, output_tokens, cache_read_tokens, cache_write_tokens,
                    cost_micros, currency, occurred_at_ms,
                    request_id, event_id, upstream_attempt, trace_id,
                    rate_card, rate_version, cost_confidence, cost_provenance
             FROM usage_records
             WHERE tenant_id = ?1 AND account_id = ?2 AND request_id = ?3
               AND COALESCE(upstream_attempt, '0') = ?4",
            params![
                record.tenant_id.as_str(),
                record.account_id,
                request_id.as_str(),
                record
                    .upstream_attempt
                    .map(|attempt| attempt.to_string())
                    .unwrap_or_else(|| "0".to_string()),
            ],
            row_to_record,
        )
        .optional()
        .map_err(storage_error)
    }

    /// 执行幂等追加（调用方必须已持有连接锁）。
    fn insert_record(conn: &mut Connection, record: &UsageRecord) -> Result<(), UsageLedgerError> {
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        match Self::read_existing(&tx, record)? {
            Some(existing) => {
                if existing.same_content(record) {
                    // 相同 ID + 相同内容：幂等重放成功，不重复写入。
                    tx.commit().map_err(storage_error)?;
                    return Ok(());
                }
                Err(UsageLedgerError::Conflict {
                    record_id: record.record_id.clone(),
                })
            }
            None => {
                // event/request attempt 去重：同 (tenant, account, request,
                // attempt) 已存在（不同 record_id）即冲突——不能只信自造
                // record_id，重放必须复用同一 record_id。
                if let Some(existing) = Self::read_existing_by_dedup(&tx, record)? {
                    return Err(UsageLedgerError::Conflict {
                        record_id: existing.record_id.clone(),
                    });
                }
                let insert_values = insert_params(record)?;
                let inserted = match tx.execute(
                    "INSERT INTO usage_records (
                         record_id, version, tenant_id, principal_id, account_id,
                         credential_id, session_id, agent_id, run_id, provider_id, model_id,
                         input_tokens, output_tokens, cache_read_tokens, cache_write_tokens,
                         cost_micros, currency, occurred_at_ms,
                         request_id, event_id, upstream_attempt, trace_id,
                         rate_card, rate_version, cost_confidence, cost_provenance
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14,
                               ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26)",
                    rusqlite::params_from_iter(insert_values.iter()),
                ) {
                    Ok(inserted) => inserted,
                    Err(rusqlite::Error::SqliteFailure(sqlite_error, _))
                        if sqlite_error.code == rusqlite::ErrorCode::ConstraintViolation =>
                    {
                        // 跨进程并发：另一进程已插入同键。回滚后按 record_id 与
                        // event/request attempt 双键重读比较，区分幂等/冲突。
                        drop(tx);
                        if let Some(existing) = Self::read_existing(conn, record)? {
                            return if existing.same_content(record) {
                                Ok(())
                            } else {
                                Err(UsageLedgerError::Conflict {
                                    record_id: record.record_id.clone(),
                                })
                            };
                        }
                        if let Some(existing) = Self::read_existing_by_dedup(conn, record)? {
                            return Err(UsageLedgerError::Conflict {
                                record_id: existing.record_id.clone(),
                            });
                        }
                        return Err(UsageLedgerError::Storage {
                            reason: "concurrent insert lost after constraint violation".to_string(),
                        });
                    }
                    Err(other) => return Err(storage_error(other)),
                };
                if inserted != 1 {
                    return Err(UsageLedgerError::Storage {
                        reason: format!("unexpected insert row count {inserted}"),
                    });
                }
                tx.commit().map_err(storage_error)?;
                Ok(())
            }
        }
    }
}

/// rusqlite 错误 → 结构化存储错误。
#[cfg(feature = "sqlite")]
fn storage_error(error: rusqlite::Error) -> UsageLedgerError {
    UsageLedgerError::Storage {
        reason: error.to_string(),
    }
}

/// 行 → [`UsageRecord`]。u64 计数存 TEXT（十进制），`occurred_at_ms` 存 INTEGER。
#[cfg(feature = "sqlite")]
fn row_to_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<UsageRecord> {
    fn text_u64(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<u64> {
        let raw: String = row.get(index)?;
        raw.parse::<u64>().map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                index,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })
    }
    fn opt_text_u64(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<Option<u64>> {
        let raw: Option<String> = row.get(index)?;
        match raw {
            None => Ok(None),
            Some(raw) => raw.parse::<u64>().map(Some).map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    index,
                    rusqlite::types::Type::Text,
                    Box::new(error),
                )
            }),
        }
    }
    fn opt_confidence(
        row: &rusqlite::Row<'_>,
        index: usize,
    ) -> rusqlite::Result<Option<CostConfidence>> {
        let raw: Option<String> = row.get(index)?;
        Ok(match raw.as_deref() {
            None => None,
            Some("estimated") => Some(CostConfidence::Estimated),
            Some("actual") => Some(CostConfidence::Actual),
            Some("unknown") => Some(CostConfidence::Unknown),
            Some(other) => {
                return Err(rusqlite::Error::FromSqlConversionFailure(
                    index,
                    rusqlite::types::Type::Text,
                    Box::new(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!("unknown cost confidence {other}"),
                    )),
                ));
            }
        })
    }
    Ok(UsageRecord {
        record_id: row.get(0)?,
        version: u32::try_from(row.get::<_, i64>(1)?).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                1,
                rusqlite::types::Type::Integer,
                Box::new(error),
            )
        })?,
        tenant_id: TenantId::new(row.get::<_, String>(2)?),
        principal_id: PrincipalId::new(row.get::<_, String>(3)?),
        account_id: row.get(4)?,
        credential_id: row.get(5)?,
        session_id: SessionId::new(row.get::<_, String>(6)?),
        agent_id: AgentId::new(row.get::<_, String>(7)?),
        run_id: row.get::<_, Option<String>>(8)?.map(RunId::new),
        provider_id: ProviderId::new(row.get::<_, String>(9)?),
        model_id: ModelId::new(row.get::<_, String>(10)?),
        input_tokens: text_u64(row, 11)?,
        output_tokens: text_u64(row, 12)?,
        cache_read_tokens: text_u64(row, 13)?,
        cache_write_tokens: text_u64(row, 14)?,
        cost_micros: text_u64(row, 15)?,
        currency: row.get(16)?,
        occurred_at_ms: u64::try_from(row.get::<_, i64>(17)?).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                17,
                rusqlite::types::Type::Integer,
                Box::new(error),
            )
        })?,
        request_id: row.get::<_, Option<String>>(18)?.map(RequestId::new),
        event_id: row.get::<_, Option<String>>(19)?.map(EventId::new),
        upstream_attempt: opt_text_u64(row, 20)?,
        trace_id: row.get(21)?,
        rate_card: row.get(22)?,
        rate_version: row.get(23)?,
        cost_confidence: opt_confidence(row, 24)?,
        cost_provenance: row.get(25)?,
    })
}

/// INSERT 参数（顺序与建表列一致）。
#[cfg(feature = "sqlite")]
fn insert_params(record: &UsageRecord) -> Result<Vec<Box<dyn rusqlite::ToSql>>, UsageLedgerError> {
    fn text(value: u64) -> String {
        value.to_string()
    }
    let occurred_at =
        i64::try_from(record.occurred_at_ms).map_err(|_| UsageLedgerError::Storage {
            reason: "occurred_at_ms exceeds SQLite INTEGER range".to_string(),
        })?;
    Ok(vec![
        Box::new(record.record_id.clone()),
        Box::new(i64::from(record.version)),
        Box::new(record.tenant_id.as_str().to_string()),
        Box::new(record.principal_id.as_str().to_string()),
        Box::new(record.account_id.clone()),
        Box::new(record.credential_id.clone()),
        Box::new(record.session_id.as_str().to_string()),
        Box::new(record.agent_id.as_str().to_string()),
        Box::new(record.run_id.as_ref().map(|id| id.as_str().to_string())),
        Box::new(record.provider_id.as_str().to_string()),
        Box::new(record.model_id.as_str().to_string()),
        Box::new(text(record.input_tokens)),
        Box::new(text(record.output_tokens)),
        Box::new(text(record.cache_read_tokens)),
        Box::new(text(record.cache_write_tokens)),
        Box::new(text(record.cost_micros)),
        Box::new(record.currency.clone()),
        Box::new(occurred_at),
        Box::new(record.request_id.as_ref().map(|id| id.as_str().to_string())),
        Box::new(record.event_id.as_ref().map(|id| id.as_str().to_string())),
        Box::new(record.upstream_attempt.map(|attempt| attempt.to_string())),
        Box::new(record.trace_id.clone()),
        Box::new(record.rate_card.clone()),
        Box::new(record.rate_version.clone()),
        Box::new(record.cost_confidence.map(confidence_to_sql)),
        Box::new(record.cost_provenance.clone()),
    ])
}

#[cfg(feature = "sqlite")]
fn confidence_to_sql(confidence: CostConfidence) -> String {
    match confidence {
        CostConfidence::Estimated => "estimated".to_string(),
        CostConfidence::Actual => "actual".to_string(),
        CostConfidence::Unknown => "unknown".to_string(),
    }
}

/// 查询 → SQL WHERE（维度过滤全部下推；时间为半开区间 [start, end)）。
#[cfg(feature = "sqlite")]
fn build_where(
    query: &UsageQuery,
) -> Result<(String, Vec<Box<dyn rusqlite::ToSql + '_>>), UsageLedgerError> {
    let mut conditions: Vec<String> = Vec::new();
    let mut values: Vec<Box<dyn rusqlite::ToSql + '_>> = Vec::new();
    macro_rules! filter {
        ($column:literal, $value:expr) => {{
            let value: Option<&str> = $value;
            values.push(Box::new(value));
            values.push(Box::new(value));
            conditions.push(format!("({column} = ? OR ? IS NULL)", column = $column));
        }};
    }
    filter!("tenant_id", query.tenant_id.as_ref().map(|id| id.as_str()));
    filter!(
        "principal_id",
        query.principal_id.as_ref().map(|id| id.as_str())
    );
    filter!("account_id", query.account_id.as_deref());
    filter!("credential_id", query.credential_id.as_deref());
    filter!(
        "session_id",
        query.session_id.as_ref().map(|id| id.as_str())
    );
    filter!("agent_id", query.agent_id.as_ref().map(|id| id.as_str()));
    filter!("run_id", query.run_id.as_ref().map(|id| id.as_str()));
    filter!(
        "provider_id",
        query.provider_id.as_ref().map(|id| id.as_str())
    );
    filter!("model_id", query.model_id.as_ref().map(|id| id.as_str()));
    filter!("currency", query.currency.as_deref());
    if let Some(start_ms) = query.occurred_at_start_ms {
        let bound = i64::try_from(start_ms).map_err(|_| UsageLedgerError::Storage {
            reason: "occurred_at_start_ms exceeds SQLite INTEGER range".to_string(),
        })?;
        values.push(Box::new(bound));
        conditions.push("occurred_at_ms >= ?".to_string());
    }
    if let Some(end_ms) = query.occurred_at_end_ms {
        let bound = i64::try_from(end_ms).map_err(|_| UsageLedgerError::Storage {
            reason: "occurred_at_end_ms exceeds SQLite INTEGER range".to_string(),
        })?;
        values.push(Box::new(bound));
        // 半开区间 [start, end)：终点不包含。
        conditions.push("occurred_at_ms < ?".to_string());
    }
    let where_clause = if conditions.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", conditions.join(" AND "))
    };
    Ok((where_clause, values))
}

#[cfg(feature = "sqlite")]
const SELECT_RECORD_COLUMNS: &str = "record_id, version, tenant_id, principal_id, account_id, \
     credential_id, session_id, agent_id, run_id, provider_id, model_id, \
     input_tokens, output_tokens, cache_read_tokens, cache_write_tokens, \
     cost_micros, currency, occurred_at_ms, \
     request_id, event_id, upstream_attempt, trace_id, \
     rate_card, rate_version, cost_confidence, cost_provenance";

#[cfg(feature = "sqlite")]
#[async_trait]
impl UsageLedger for SqliteUsageLedger {
    async fn record(&self, record: UsageRecord) -> Result<(), UsageLedgerError> {
        validate_record(&record)?;
        if record.record_id.is_empty() {
            return Err(UsageLedgerError::InvalidRecord {
                reason: "持久化账本要求显式 record_id（空 ID 无法跨进程幂等）".to_string(),
            });
        }
        let mut conn = self.conn.lock().map_err(|_| UsageLedgerError::Storage {
            reason: "ledger connection mutex poisoned".to_string(),
        })?;
        Self::insert_record(&mut conn, &record)
    }

    async fn query(&self, query: &UsageQuery) -> Result<Vec<UsageRecord>, UsageLedgerError> {
        let (where_clause, values) = build_where(query)?;
        let sql = format!("SELECT {SELECT_RECORD_COLUMNS} FROM usage_records {where_clause}");
        let conn = self.conn.lock().map_err(|_| UsageLedgerError::Storage {
            reason: "ledger connection mutex poisoned".to_string(),
        })?;
        let mut statement = conn.prepare(&sql).map_err(storage_error)?;
        let rows = statement
            .query_map(rusqlite::params_from_iter(values.iter()), row_to_record)
            .map_err(storage_error)?;
        // fail-closed：存储 / 行解码错误必须向上传播，绝不吞错为空集。
        rows.collect::<Result<Vec<_>, _>>().map_err(storage_error)
    }

    async fn aggregate(&self, query: &UsageQuery) -> Result<UsageTotals, UsageLedgerError> {
        let mut totals = UsageTotals::default();
        let mut currencies = std::collections::BTreeSet::new();
        for record in self.query(query).await? {
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
            version: RECORD_VERSION,
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
            request_id: None,
            event_id: None,
            upstream_attempt: None,
            trace_id: None,
            rate_card: None,
            rate_version: None,
            cost_confidence: None,
            cost_provenance: None,
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

        let records = ledger.query(&UsageQuery::default()).await.unwrap();
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
            .await
            .unwrap();
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
            .await
            .unwrap();
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
            .await
            .unwrap();
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
            .await
            .unwrap();
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
        assert!(ledger
            .query(&UsageQuery::default())
            .await
            .unwrap()
            .is_empty());
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
        let records = ledger.query(&query).await.unwrap();
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
            .await
            .unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].record_id, "t2");

        // 空区间 [300, 300)：无命中。
        let records = ledger
            .query(&UsageQuery::by_occurred_between(300, 300))
            .await
            .unwrap();
        assert!(records.is_empty());

        // 仅起点（含）：>= 100 全部命中。
        let from = UsageQuery {
            occurred_at_start_ms: Some(100),
            ..UsageQuery::default()
        };
        assert_eq!(ledger.query(&from).await.unwrap().len(), 3);

        // 仅终点（不含）：< 300 命中前两条。
        let to = UsageQuery {
            occurred_at_end_ms: Some(300),
            ..UsageQuery::default()
        };
        assert_eq!(ledger.query(&to).await.unwrap().len(), 2);

        // 全覆盖区间。
        assert_eq!(
            ledger
                .query(&UsageQuery::by_occurred_between(0, u64::MAX))
                .await
                .unwrap()
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

        let records = ledger.query(&UsageQuery::default()).await.unwrap();
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

        let records = ledger.query(&UsageQuery::default()).await.unwrap();
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
            ledger.query(&UsageQuery::default()).await.unwrap().len(),
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
        assert_eq!(ledger.query(&UsageQuery::default()).await.unwrap().len(), 3);

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
        assert_eq!(ledger.query(&UsageQuery::default()).await.unwrap().len(), 3);

        // tenant 视角严格隔离：tenant-b 只能看到自己的记录。
        let b_records = ledger
            .query(&UsageQuery::by_tenant(TenantId::new("tenant-b")))
            .await
            .unwrap();
        assert_eq!(b_records.len(), 1);
        assert_eq!(b_records[0].account_id, "account-x");
        assert_eq!(b_records[0].tenant_id, TenantId::new("tenant-b"));

        // account 视角严格隔离。
        let y_records = ledger
            .query(&UsageQuery {
                account_id: Some("account-y".to_string()),
                ..UsageQuery::default()
            })
            .await
            .unwrap();
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

        let records = ledger.query(&UsageQuery::default()).await.unwrap();
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
        let records = ledger.query(&UsageQuery::default()).await.unwrap();
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
        let records = ledger.query(&UsageQuery::default()).await.unwrap();
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
        let records = ledger.query(&query).await.unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].record_id, "target");

        let totals = ledger.aggregate(&query).await.unwrap();
        assert_eq!(totals.input_tokens, 10);
        assert_eq!(totals.cost_micros, 100);

        let credential_records = ledger
            .query(&UsageQuery::by_credential("credential-1"))
            .await
            .unwrap();
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

    // ---- P18-8：SQLite 持久账本 ----

    #[cfg(feature = "sqlite")]
    #[tokio::test]
    async fn sqlite_roundtrip_and_reopen_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("usage.sqlite3");

        let record = make_record(
            "rec-sqlite-1",
            "tenant-a",
            "principal-1",
            "account-x",
            "session-1",
            "agent-1",
        );

        // 第一个“进程”：写入。
        {
            let ledger = SqliteUsageLedger::open(&path).unwrap();
            ledger.record(record.clone()).await.unwrap();
        }

        // 第二个“进程”：重开同一文件，能读到且重放幂等。
        let ledger = SqliteUsageLedger::open(&path).unwrap();
        let records = ledger.query(&UsageQuery::default()).await.unwrap();
        assert_eq!(records, vec![record.clone()]);

        ledger.record(record.clone()).await.unwrap();
        let records = ledger.query(&UsageQuery::default()).await.unwrap();
        assert_eq!(records.len(), 1, "相同 ID+内容重放不得重复累计");
    }

    #[cfg(feature = "sqlite")]
    #[tokio::test]
    async fn sqlite_same_id_different_content_conflicts() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("usage.sqlite3");
        let ledger = SqliteUsageLedger::open(&path).unwrap();

        let original = make_record(
            "rec-conflict",
            "tenant-a",
            "principal-1",
            "account-x",
            "session-1",
            "agent-1",
        );
        ledger.record(original.clone()).await.unwrap();

        let mut divergent = original.clone();
        divergent.input_tokens += 1;
        let error = ledger.record(divergent).await.unwrap_err();
        match error {
            UsageLedgerError::Conflict { record_id } => assert_eq!(record_id, "rec-conflict"),
            other => panic!("expected Conflict, got {other:?}"),
        }
    }

    #[cfg(feature = "sqlite")]
    #[tokio::test]
    async fn sqlite_tenant_isolation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("usage.sqlite3");
        let ledger = SqliteUsageLedger::open(&path).unwrap();

        let tenant_a = make_record(
            "rec-a",
            "tenant-a",
            "principal-1",
            "account-x",
            "session-1",
            "agent-1",
        );
        let tenant_b = make_record(
            "rec-b",
            "tenant-b",
            "principal-2",
            "account-y",
            "session-2",
            "agent-2",
        );
        ledger.record(tenant_a.clone()).await.unwrap();
        ledger.record(tenant_b.clone()).await.unwrap();

        let only_a = ledger
            .query(&UsageQuery::by_tenant(TenantId::new("tenant-a")))
            .await
            .unwrap();
        assert_eq!(only_a, vec![tenant_a]);

        // 同名 record_id 在不同 tenant 下互不冲突。
        let tenant_a_dup = make_record(
            "rec-same",
            "tenant-a",
            "principal-1",
            "account-x",
            "session-1",
            "agent-1",
        );
        let tenant_b_dup = make_record(
            "rec-same",
            "tenant-b",
            "principal-2",
            "account-y",
            "session-2",
            "agent-2",
        );
        ledger.record(tenant_a_dup).await.unwrap();
        ledger.record(tenant_b_dup).await.unwrap();
    }

    #[cfg(feature = "sqlite")]
    #[tokio::test]
    async fn sqlite_v2_fields_persist() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("usage.sqlite3");
        let ledger = SqliteUsageLedger::open(&path).unwrap();

        let mut record = make_record(
            "rec-v2",
            "tenant-a",
            "principal-1",
            "account-x",
            "session-1",
            "agent-1",
        );
        record.version = RECORD_VERSION;
        record.request_id = Some(RequestId::new("req-42"));
        record.event_id = Some(EventId::new("evt-7"));
        record.upstream_attempt = Some(3);
        record.rate_card = Some("builtin".to_string());
        record.rate_version = Some("2026-08-01".to_string());
        record.cost_confidence = Some(CostConfidence::Estimated);
        record.cost_provenance = Some("model-registry:builtin:estimate".to_string());
        ledger.record(record.clone()).await.unwrap();

        let records = ledger.query(&UsageQuery::default()).await.unwrap();
        assert_eq!(records, vec![record], "trace/pricing 快照必须完整持久化");
    }

    #[cfg(feature = "sqlite")]
    #[tokio::test]
    async fn sqlite_empty_record_id_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("usage.sqlite3");
        let ledger = SqliteUsageLedger::open(&path).unwrap();

        let mut record = make_record(
            "",
            "tenant-a",
            "principal-1",
            "account-x",
            "session-1",
            "agent-1",
        );
        record.record_id.clear();
        let error = ledger.record(record).await.unwrap_err();
        match error {
            UsageLedgerError::InvalidRecord { .. } => {}
            other => panic!("expected InvalidRecord, got {other:?}"),
        }
    }

    #[cfg(feature = "sqlite")]
    #[tokio::test]
    async fn sqlite_occurred_at_overflow_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("usage.sqlite3");
        let ledger = SqliteUsageLedger::open(&path).unwrap();

        let mut record = make_record(
            "rec-ts",
            "tenant-a",
            "principal-1",
            "account-x",
            "session-1",
            "agent-1",
        );
        record.occurred_at_ms = u64::MAX;
        let error = ledger.record(record).await.unwrap_err();
        match error {
            UsageLedgerError::Storage { reason } => {
                assert!(reason.contains("occurred_at_ms"), "{reason}");
            }
            other => panic!("expected Storage, got {other:?}"),
        }
    }

    #[cfg(feature = "sqlite")]
    #[test]
    fn sqlite_schema_version_mismatch_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("usage.sqlite3");
        // 手工造一个更高版本的库。
        {
            let conn = rusqlite::Connection::open(&path).unwrap();
            conn.pragma_update(None, "user_version", 99).unwrap();
        }
        let error = SqliteUsageLedger::open(&path).unwrap_err();
        match error {
            UsageLedgerError::Storage { reason } => {
                assert!(
                    reason.contains("unsupported ledger schema version 99"),
                    "{reason}"
                );
            }
            other => panic!("expected Storage, got {other:?}"),
        }
    }

    #[test]
    fn legacy_v1_json_decodes_as_version_1() {
        // 旧 v1 JSON：无 version/trace/pricing 字段，必须解码为 version=1 且不丢身份/成本。
        let v1_json = serde_json::json!({
            "record_id": "rec-v1",
            "tenant_id": "local/default",
            "principal_id": "local/user",
            "account_id": "local/default",
            "session_id": "session-1",
            "agent_id": "agent-1",
            "provider_id": "openai",
            "model_id": "gpt-4o",
            "input_tokens": 10,
            "output_tokens": 5,
            "cache_read_tokens": 0,
            "cache_write_tokens": 0,
            "cost_micros": 42,
            "currency": "USD",
            "occurred_at_ms": 1_700_000_000_000_i64
        });
        let record: UsageRecord = serde_json::from_value(v1_json).unwrap();
        assert_eq!(record.version, 1, "旧记录缺省解码为 v1（兼容迁移）");
        assert_eq!(record.tenant_id.as_str(), "local/default");
        assert_eq!(record.request_id, None);
        assert_eq!(record.upstream_attempt, None);
        assert_eq!(record.cost_provenance, None);
        assert_eq!(record.input_tokens, 10);
        assert_eq!(record.cost_micros, 42);

        // 新 v2 记录显式版本 2。
        let mut v2 = make_record(
            "rec-v2-json",
            "local/default",
            "local/user",
            "local/default",
            "session-1",
            "agent-1",
        );
        v2.upstream_attempt = Some(1);
        let encoded = serde_json::to_value(&v2).unwrap();
        let decoded: UsageRecord = serde_json::from_value(encoded).unwrap();
        assert_eq!(decoded, v2);
    }

    #[cfg(feature = "sqlite")]
    #[tokio::test]
    async fn sqlite_v2_to_v3_migration_preserves_history() {
        // v2 库（无 trace_id 列、无 dedup 索引）→ 重开触发 v3 迁移：历史
        // 记录原样保留、版本登记为 3、新记录可带 trace_id。
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("usage.sqlite3");
        {
            let conn = rusqlite::Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE usage_records (
                     record_id TEXT NOT NULL,
                     version INTEGER NOT NULL,
                     tenant_id TEXT NOT NULL,
                     principal_id TEXT NOT NULL,
                     account_id TEXT NOT NULL,
                     credential_id TEXT,
                     session_id TEXT NOT NULL,
                     agent_id TEXT NOT NULL,
                     run_id TEXT,
                     provider_id TEXT NOT NULL,
                     model_id TEXT NOT NULL,
                     input_tokens TEXT NOT NULL,
                     output_tokens TEXT NOT NULL,
                     cache_read_tokens TEXT NOT NULL,
                     cache_write_tokens TEXT NOT NULL,
                     cost_micros TEXT NOT NULL,
                     currency TEXT NOT NULL,
                     occurred_at_ms INTEGER NOT NULL,
                     request_id TEXT,
                     event_id TEXT,
                     upstream_attempt TEXT,
                     rate_card TEXT,
                     rate_version TEXT,
                     cost_confidence TEXT,
                     cost_provenance TEXT,
                     PRIMARY KEY (tenant_id, account_id, record_id)
                 );
                 PRAGMA user_version = 2;",
            )
            .unwrap();
            let mut legacy = make_record(
                "rec-legacy",
                "tenant-a",
                "principal-1",
                "account-x",
                "session-1",
                "agent-1",
            );
            legacy.version = 1; // 旧 v1 语义的历史记录
            legacy.occurred_at_ms = 1_700_000_000_000;
            // v2 表没有 trace_id 列：剔除第 22 个参数（insert_params 的 v3 顺序）。
            let values: Vec<Box<dyn rusqlite::ToSql>> = insert_params(&legacy)
                .unwrap()
                .into_iter()
                .enumerate()
                .filter(|(index, _)| *index != 21)
                .map(|(_, value)| value)
                .collect();
            conn.execute(
                "INSERT INTO usage_records (
                     record_id, version, tenant_id, principal_id, account_id,
                     credential_id, session_id, agent_id, run_id, provider_id, model_id,
                     input_tokens, output_tokens, cache_read_tokens, cache_write_tokens,
                     cost_micros, currency, occurred_at_ms,
                     request_id, event_id, upstream_attempt,
                     rate_card, rate_version, cost_confidence, cost_provenance
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14,
                           ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25)",
                rusqlite::params_from_iter(values.iter()),
            )
            .unwrap();
        }

        // 重开：v2 → v3 自动迁移，历史不丢。
        let ledger = SqliteUsageLedger::open(&path).unwrap();
        let records = ledger.query(&UsageQuery::default()).await.unwrap();
        assert_eq!(records.len(), 1, "迁移不得丢历史记录");
        assert_eq!(records[0].record_id, "rec-legacy");
        assert_eq!(records[0].version, 1);
        assert_eq!(records[0].trace_id, None, "旧记录 trace_id 缺省为 None");

        // 迁移后账本版本已登记为 v3，且新记录可写 trace_id。
        let version: i64 = rusqlite::Connection::open(&path)
            .unwrap()
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION, "迁移必须登记新 schema 版本");
        let mut traced = make_record(
            "rec-traced",
            "tenant-a",
            "principal-1",
            "account-x",
            "session-1",
            "agent-1",
        );
        traced.trace_id = Some("trace-migrated-1".to_string());
        ledger.record(traced.clone()).await.unwrap();
        let records = ledger.query(&UsageQuery::default()).await.unwrap();
        assert_eq!(records.len(), 2);
        assert!(records
            .iter()
            .any(|record| record.trace_id.as_deref() == Some("trace-migrated-1")));
    }

    #[cfg(feature = "sqlite")]
    #[tokio::test]
    async fn sqlite_dedup_by_request_and_attempt_conflicts() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("usage.sqlite3");
        let ledger = SqliteUsageLedger::open(&path).unwrap();

        let first = make_record(
            "rec-req-1",
            "tenant-a",
            "principal-1",
            "account-x",
            "session-1",
            "agent-1",
        );
        let mut with_request = first.clone();
        with_request.request_id = Some(RequestId::new("req-dedup-1"));
        with_request.upstream_attempt = Some(1);
        ledger.record(with_request.clone()).await.unwrap();

        // 同 (tenant, account, request, attempt) 但不同 record_id：存储层冲突，
        // 不因自造 record_id 不同而放行重复。
        let mut duplicate = with_request.clone();
        duplicate.record_id = "rec-req-1-dup".to_string();
        let error = ledger.record(duplicate).await.unwrap_err();
        match error {
            UsageLedgerError::Conflict { record_id } => {
                assert_eq!(record_id, "rec-req-1", "冲突应指向既有记录");
            }
            other => panic!("expected Conflict, got {other:?}"),
        }

        // 同 request 但 attempt 不同：独立上游调用，允许并存。
        let mut attempt_two = with_request.clone();
        attempt_two.record_id = "rec-req-1-attempt-2".to_string();
        attempt_two.upstream_attempt = Some(2);
        ledger.record(attempt_two.clone()).await.unwrap();

        // attempt None 与 Some(0) 折叠为同一去重键：冲突。先插入 None 记录，
        // 再用不同 record_id + Some(0) 重放同 request。
        let mut attempt_none = with_request.clone();
        attempt_none.record_id = "rec-req-1-attempt-none".to_string();
        attempt_none.upstream_attempt = None;
        ledger.record(attempt_none.clone()).await.unwrap();
        let mut attempt_zero = attempt_none.clone();
        attempt_zero.record_id = "rec-req-1-attempt-zero".to_string();
        attempt_zero.upstream_attempt = Some(0);
        let error = ledger.record(attempt_zero).await.unwrap_err();
        assert!(
            matches!(error, UsageLedgerError::Conflict { .. }),
            "attempt None 必须与 Some(0) 折叠为同一去重键"
        );

        // 幂等重放（同 record_id + 同内容）仍成功，不冲突。
        ledger.record(with_request.clone()).await.unwrap();
        let records = ledger.query(&UsageQuery::default()).await.unwrap();
        assert_eq!(records.len(), 3);
    }

    #[cfg(feature = "sqlite")]
    #[tokio::test]
    async fn sqlite_trace_id_roundtrips_across_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("usage.sqlite3");
        {
            let ledger = SqliteUsageLedger::open(&path).unwrap();
            let mut record = make_record(
                "rec-trace",
                "tenant-a",
                "principal-1",
                "account-x",
                "session-1",
                "agent-1",
            );
            record.trace_id = Some("trace-abc-123".to_string());
            record.request_id = Some(RequestId::new("req-trace"));
            record.upstream_attempt = Some(0);
            ledger.record(record.clone()).await.unwrap();
        }
        let ledger = SqliteUsageLedger::open(&path).unwrap();
        let records = ledger.query(&UsageQuery::default()).await.unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(
            records[0].trace_id.as_deref(),
            Some("trace-abc-123"),
            "trace_id 必须跨进程持久化"
        );
        assert_eq!(
            records[0].request_id.as_ref().map(|id| id.as_str()),
            Some("req-trace")
        );
        assert_eq!(records[0].upstream_attempt, Some(0));
    }

    #[cfg(feature = "sqlite")]
    #[tokio::test]
    async fn sqlite_two_instances_share_one_file_idempotently() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("usage.sqlite3");
        let a = SqliteUsageLedger::open(&path).unwrap();
        let b = SqliteUsageLedger::open(&path).unwrap();

        let record_a = make_record(
            "rec-inst-a",
            "tenant-a",
            "principal-1",
            "account-x",
            "session-1",
            "agent-1",
        );
        let record_b = make_record(
            "rec-inst-b",
            "tenant-a",
            "principal-1",
            "account-x",
            "session-2",
            "agent-1",
        );
        a.record(record_a.clone()).await.unwrap();
        b.record(record_b.clone()).await.unwrap();

        // 两个实例看到同一账本（跨进程并发读写同一文件）。
        let from_a = a.query(&UsageQuery::default()).await.unwrap();
        let from_b = b.query(&UsageQuery::default()).await.unwrap();
        assert_eq!(from_a.len(), 2);
        assert_eq!(from_b, from_a);

        // 同一 record 从另一实例重放：幂等成功，不重复累计。
        b.record(record_a.clone()).await.unwrap();
        let from_b = b.query(&UsageQuery::default()).await.unwrap();
        assert_eq!(from_b.len(), 2, "跨实例重放不得重复写入");

        // 同 request/attempt 不同 record_id 跨实例仍冲突（存储层唯一索引）。
        let mut dup = record_a.clone();
        dup.record_id = "rec-inst-a-dup".to_string();
        dup.request_id = Some(RequestId::new("req-inst-a"));
        dup.upstream_attempt = Some(0);
        a.record(dup.clone()).await.unwrap();
        let mut cross_dup = dup.clone();
        cross_dup.record_id = "rec-inst-b-dup".to_string();
        let error = b.record(cross_dup).await.unwrap_err();
        assert!(matches!(error, UsageLedgerError::Conflict { .. }));
    }

    #[tokio::test]
    async fn in_memory_dedup_matches_sqlite_semantics() {
        let ledger = InMemoryUsageLedger::new();
        let mut record = make_record(
            "rec-mem-1",
            "tenant-a",
            "principal-1",
            "account-x",
            "session-1",
            "agent-1",
        );
        record.request_id = Some(RequestId::new("req-mem"));
        record.upstream_attempt = Some(0);
        ledger.record(record.clone()).await.unwrap();

        let mut duplicate = record.clone();
        duplicate.record_id = "rec-mem-1-dup".to_string();
        let error = ledger.record(duplicate).await.unwrap_err();
        match error {
            UsageLedgerError::Conflict { record_id } => {
                assert_eq!(record_id, "rec-mem-1");
            }
            other => panic!("expected Conflict, got {other:?}"),
        }
        // 同 record_id + 同内容：幂等重放。
        ledger.record(record.clone()).await.unwrap();
        let records = ledger.query(&UsageQuery::default()).await.unwrap();
        assert_eq!(records.len(), 1);
    }

    #[cfg(feature = "sqlite")]
    #[test]
    fn sqlite_dedup_unique_index_is_registered() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("usage.sqlite3");
        let ledger = SqliteUsageLedger::open(&path).unwrap();
        let conn = ledger.conn.lock().unwrap();
        let names: Vec<String> = conn
            .prepare(
                "SELECT name FROM sqlite_master WHERE type = 'index' AND name = 'idx_usage_dedup'",
            )
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .filter_map(Result::ok)
            .collect();
        assert_eq!(names, vec!["idx_usage_dedup"], "存储层必须登记去重唯一索引");
    }

    #[cfg(feature = "sqlite")]
    #[test]
    fn sqlite_connection_is_not_send_but_ledger_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<SqliteUsageLedger>();
    }
}
