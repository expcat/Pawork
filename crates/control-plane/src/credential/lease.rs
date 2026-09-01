//! Versioned、evented 的 credential-lease 状态机（P18-4，ADR-033）。
//!
//! Canonical 生命周期（单向、不可回退）：
//!
//! ```text
//! Requested → Acquired → Released | Expired → Reclaimed
//! ```
//!
//! - [`LeaseRecord`] 是唯一的 canonical lease 实体：每次状态转换 `version` 自增 1，
//!   并产生一条 [`LeaseEvent`]，支持崩溃恢复重放与审计。
//! - [`LeaseRecord`] **绝不包含任何 secret 字段**——只携带定位/归属/期限信息；
//!   明文 API Key 由 `CredentialResolver`（ADR-014）在 lease 之外短生命周期解析。
//! - 本模块是纯领域逻辑（无 I/O、无 await），可独立单测与 property 测试；
//!   [`LeaseProjection`] 是可选的对象安全持久化 sink，由宿主组合层注入。
//!
//! 不依赖 `account-control-v1` feature：时钟与投影类型在本模块自洽定义，
//! 保证关闭控制面 v1 时 P18-4 CredentialPool 仍可独立工作。
//!
//! 依赖方向：`provider-control → agent-domain`（仅引用 opaque ID 与 [`Timestamp`]）。

use std::collections::HashMap;
use std::fmt;

use async_trait::async_trait;
use pawork_domain::{
    AccountId, AgentId, CredentialId, PrincipalId, ProviderId, SessionId, TenantId, Timestamp,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::credential::{AcquireRequest, LeaseId, LeaseOutcome, CONTROL_PLANE_SCHEMA_VERSION};

/// 公开（无 secret）的 lease 视图，由 [`LeaseRecord`] 投影得到，供 Agent/Client 持有。
pub use crate::credential::CredentialLease;

/// Lease 实体 schema 版本（与 `app-database` 的 `credential_leases` 迁移对齐）。
///
/// 所有 lease 行携带该版本字段，支持版本化迁移与重放（ADR-016/ADR-033）。
pub const LEASE_SCHEMA_VERSION: u32 = CONTROL_PLANE_SCHEMA_VERSION;

/// Lease 生命周期状态：canonical 单向状态机。
///
/// 转换路径固定为 `Requested → Acquired → Released | Expired → Reclaimed`，
/// 由 [`LeaseRecord`] 的转换方法强制；非法转换返回 [`LeaseTransitionError`]。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LeaseState {
    /// 已请求，准入校验通过但尚未物化为活跃 lease（瞬时态）。
    #[default]
    Requested,
    /// 已授予：占用一个并发额度，持有 credential 定位与期限。
    Acquired,
    /// 已显式释放（caller 主动 `release`）：并发额度已归还。
    Released,
    /// 已过期：TTL 到期仍未释放（崩溃 / 孤儿 lease）：并发额度由回收扫描归还。
    Expired,
    /// 已回收：终态，审计闭环完成，记录可被 GC。
    Reclaimed,
}

impl LeaseState {
    /// 冻结的持久化字符串（与 `app-database` `credential_leases.state` 列对齐）。
    pub fn as_db_str(self) -> &'static str {
        match self {
            Self::Requested => "requested",
            Self::Acquired => "acquired",
            Self::Released => "released",
            Self::Expired => "expired",
            Self::Reclaimed => "reclaimed",
        }
    }

    /// 由持久化字符串反解；未知值返回 `None`。
    pub fn from_db_str(value: &str) -> Option<Self> {
        match value {
            "requested" => Some(Self::Requested),
            "acquired" => Some(Self::Acquired),
            "released" => Some(Self::Released),
            "expired" => Some(Self::Expired),
            "reclaimed" => Some(Self::Reclaimed),
            _ => None,
        }
    }

    /// 是否为活跃态（计入并发额度）。
    pub const fn holds_slot(self) -> bool {
        matches!(self, Self::Acquired)
    }

    /// 是否为「已释放/过期/回收」——release 幂等判定：再次释放视为 `already_released`。
    pub const fn is_settled(self) -> bool {
        matches!(self, Self::Released | Self::Expired | Self::Reclaimed)
    }

    /// 是否为终态（不可再转换）。
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Reclaimed)
    }
}

impl fmt::Display for LeaseState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_db_str())
    }
}

/// 版本化、evented 的 canonical lease 记录（**不含任何 secret**）。
///
/// `version` 在每次合法状态转换后自增 1，用于乐观并发、重放幂等与审计对账。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeaseRecord {
    /// lease 唯一标识。
    pub lease_id: LeaseId,
    /// 实体 schema 版本。
    pub schema_version: u32,
    /// 当前生命周期状态。
    pub state: LeaseState,
    /// 乐观并发版本号；`Requested` 为 1，此后每次转换 +1。
    pub version: u64,
    /// 所属租户。
    pub tenant_id: TenantId,
    /// 被占用的账号。
    pub account_id: AccountId,
    /// 使用的 Provider。
    pub provider_id: ProviderId,
    /// 本次 lease 绑定的凭据（caller 可据此 resolve 短生命周期 secret）。
    pub credential_id: CredentialId,
    /// 发起主体（ownership：谁拥有本次 lease）。
    pub principal_id: PrincipalId,
    /// 持有该 lease 的 Agent。
    pub agent_id: AgentId,
    /// 持有该 lease 的会话。
    pub session_id: SessionId,
    /// 授予时刻（Unix 毫秒）。
    pub acquired_at: Timestamp,
    /// lease TTL（毫秒）；到期未释放由回收扫描标记 Expired。
    pub ttl_ms: u64,
    /// 过期时刻（`acquired_at + ttl_ms`）。
    pub expires_at: Timestamp,
    /// 释放结果分类；`Acquired`/`Requested`/`Expired` 态为 `None`。
    pub outcome: Option<LeaseOutcome>,
    /// 可选追踪标识，便于日志关联。
    pub trace_id: Option<String>,
}

impl LeaseRecord {
    /// 物化一个新 lease：`Requested(v1) → Acquired(v2)`，返回记录与两条审计事件。
    ///
    /// 准入（并发额度检查）由调用方（池）在持锁临界区内完成；本方法只负责构造
    /// canonical 记录与事件，不做并发判定。
    pub fn open(
        req: &AcquireRequest,
        lease_id: LeaseId,
        credential_id: CredentialId,
        clock: &dyn LeaseClock,
        ttl_ms: u64,
    ) -> (Self, LeaseEvent, LeaseEvent) {
        let acquired_at = clock.now();
        let expires_at =
            Timestamp::from_unix_millis(acquired_at.as_unix_millis().saturating_add(ttl_ms));
        let account_id = req
            .account_id
            .clone()
            .unwrap_or_else(|| AccountId::new("local/default"));
        let provider_id = req
            .provider_id
            .clone()
            .unwrap_or_else(|| ProviderId::new("default"));
        let record = Self {
            lease_id,
            schema_version: LEASE_SCHEMA_VERSION,
            state: LeaseState::Acquired,
            version: 2,
            tenant_id: req.tenant_id.clone(),
            account_id,
            provider_id,
            credential_id,
            principal_id: req.principal_id.clone(),
            agent_id: req.agent_id.clone(),
            session_id: req.session_id.clone(),
            acquired_at,
            ttl_ms,
            expires_at,
            outcome: None,
            trace_id: req.trace_id.clone(),
        };
        let requested = LeaseEvent::Requested {
            lease_id: record.lease_id.clone(),
            version: 1,
            tenant_id: record.tenant_id.clone(),
            account_id: record.account_id.clone(),
            agent_id: record.agent_id.clone(),
            at_ms: acquired_at.as_unix_millis(),
        };
        let acquired = LeaseEvent::Acquired {
            lease_id: record.lease_id.clone(),
            version: 2,
            credential_id: record.credential_id.clone(),
            acquired_at_ms: acquired_at.as_unix_millis(),
            expires_at_ms: expires_at.as_unix_millis(),
        };
        (record, requested, acquired)
    }

    /// `Acquired → Released`，携带结果分类。仅 `Acquired` 态可释放。
    pub fn release(
        mut self,
        outcome: LeaseOutcome,
        clock: &dyn LeaseClock,
    ) -> Result<(Self, LeaseEvent), LeaseTransitionError> {
        if self.state != LeaseState::Acquired {
            return Err(LeaseTransitionError::InvalidRelease {
                lease_id: self.lease_id.clone(),
                from: self.state,
            });
        }
        self.version = self.version.saturating_add(1);
        self.state = LeaseState::Released;
        self.outcome = Some(outcome);
        let event = LeaseEvent::Released {
            lease_id: self.lease_id.clone(),
            version: self.version,
            outcome,
            at_ms: clock.now().as_unix_millis(),
        };
        Ok((self, event))
    }

    /// `Acquired → Expired`：TTL 到期未释放（崩溃 / 孤儿）。仅 `Acquired` 态可过期。
    pub fn expire(
        mut self,
        clock: &dyn LeaseClock,
    ) -> Result<(Self, LeaseEvent), LeaseTransitionError> {
        if self.state != LeaseState::Acquired {
            return Err(LeaseTransitionError::InvalidExpire {
                lease_id: self.lease_id.clone(),
                from: self.state,
            });
        }
        self.version = self.version.saturating_add(1);
        self.state = LeaseState::Expired;
        let event = LeaseEvent::Expired {
            lease_id: self.lease_id.clone(),
            version: self.version,
            at_ms: clock.now().as_unix_millis(),
        };
        Ok((self, event))
    }

    /// `Released | Expired → Reclaimed`：终态回收（审计闭环 / GC）。已 `Reclaimed` 报错。
    pub fn reclaim(
        mut self,
        clock: &dyn LeaseClock,
    ) -> Result<(Self, LeaseEvent), LeaseTransitionError> {
        if self.state.is_terminal() {
            return Err(LeaseTransitionError::AlreadyTerminal {
                lease_id: self.lease_id.clone(),
            });
        }
        if !self.state.is_settled() {
            return Err(LeaseTransitionError::InvalidReclaim {
                lease_id: self.lease_id.clone(),
                from: self.state,
            });
        }
        self.version = self.version.saturating_add(1);
        self.state = LeaseState::Reclaimed;
        let event = LeaseEvent::Reclaimed {
            lease_id: self.lease_id.clone(),
            version: self.version,
            at_ms: clock.now().as_unix_millis(),
        };
        Ok((self, event))
    }

    /// 是否已过 TTL（仅 `Acquired` 态有意义）。
    pub fn is_past_ttl(&self, now: Timestamp) -> bool {
        self.state == LeaseState::Acquired
            && now.as_unix_millis() >= self.expires_at.as_unix_millis()
    }
}

impl LeaseRecord {
    /// 投影为公开（无 secret）的 [`CredentialLease`] 视图。
    ///
    /// [`CredentialLease`] 与 canonical 记录字段一一对应，但**不暴露** state/事件
    /// 等内部状态（caller 只能获得 lease，不能驱动状态机或读取 secret）。
    pub fn to_public_lease(&self) -> CredentialLease {
        CredentialLease {
            lease_id: self.lease_id.clone(),
            schema_version: self.schema_version,
            credential_id: self.credential_id.clone(),
            account_id: self.account_id.clone(),
            provider_id: self.provider_id.clone(),
            agent_id: self.agent_id.clone(),
            session_id: self.session_id.clone(),
            principal_id: self.principal_id.clone(),
            tenant_id: self.tenant_id.clone(),
            acquired_at_ms: self.acquired_at.as_unix_millis(),
            expires_at_ms: self.expires_at.as_unix_millis(),
            version: self.version,
        }
    }
}

/// 状态转换产生的 canonical 事件（versioned，支持 ADR-016 重放与审计）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum LeaseEvent {
    /// 准入请求已记录（version 1）。
    Requested {
        lease_id: LeaseId,
        version: u64,
        tenant_id: TenantId,
        account_id: AccountId,
        agent_id: AgentId,
        at_ms: u64,
    },
    /// lease 已授予（version 2）。
    Acquired {
        lease_id: LeaseId,
        version: u64,
        credential_id: CredentialId,
        acquired_at_ms: u64,
        expires_at_ms: u64,
    },
    /// 显式释放。
    Released {
        lease_id: LeaseId,
        version: u64,
        outcome: LeaseOutcome,
        at_ms: u64,
    },
    /// TTL 到期。
    Expired {
        lease_id: LeaseId,
        version: u64,
        at_ms: u64,
    },
    /// 终态回收。
    Reclaimed {
        lease_id: LeaseId,
        version: u64,
        at_ms: u64,
    },
}

impl LeaseEvent {
    /// 事件对应的 lease 标识。
    pub fn lease_id(&self) -> &LeaseId {
        match self {
            Self::Requested { lease_id, .. }
            | Self::Acquired { lease_id, .. }
            | Self::Released { lease_id, .. }
            | Self::Expired { lease_id, .. }
            | Self::Reclaimed { lease_id, .. } => lease_id,
        }
    }

    /// 事件携带的 version（转换后的新 version）。
    pub fn version(&self) -> u64 {
        match self {
            Self::Requested { version, .. }
            | Self::Acquired { version, .. }
            | Self::Released { version, .. }
            | Self::Expired { version, .. }
            | Self::Reclaimed { version, .. } => *version,
        }
    }
}

/// 状态机转换错误。`Display` 不含 secret（lease 记录本身无 secret）。
#[derive(Debug, Error)]
pub enum LeaseTransitionError {
    /// `release` 只能从 `Acquired` 触发。
    #[error("lease {lease_id}: invalid release from state {from:?}")]
    InvalidRelease { lease_id: LeaseId, from: LeaseState },
    /// `expire` 只能从 `Acquired` 触发。
    #[error("lease {lease_id}: invalid expire from state {from:?}")]
    InvalidExpire { lease_id: LeaseId, from: LeaseState },
    /// `reclaim` 只能从 `Released`/`Expired` 触发。
    #[error("lease {lease_id}: invalid reclaim from state {from:?}")]
    InvalidReclaim { lease_id: LeaseId, from: LeaseState },
    /// 已是终态，不可再转换。
    #[error("lease {lease_id}: already terminal")]
    AlreadyTerminal { lease_id: LeaseId },
}

/// Lease 时钟抽象：注入式以支持确定性过期/回收测试。
///
/// 独立于 `account-control-v1` 的 `Clock`，保证 lease 模块在 feature 关闭时仍可用。
/// 宿主组合层可在两侧 clock 间适配（cross-wiring）。
pub trait LeaseClock: Send + Sync {
    /// 当前时间（Unix 毫秒）。
    fn now(&self) -> Timestamp;
}

/// 生产时钟：读取 OS 墙钟。
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemLeaseClock;

impl LeaseClock for SystemLeaseClock {
    fn now(&self) -> Timestamp {
        Timestamp::from_unix_millis(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0),
        )
    }
}

/// 固定时钟（测试用）：始终返回构造时给定的时间。
#[derive(Clone, Copy, Debug)]
pub struct FixedLeaseClock(Timestamp);

impl FixedLeaseClock {
    /// 以固定时间构造。
    pub const fn new(timestamp: Timestamp) -> Self {
        Self(timestamp)
    }
}

impl LeaseClock for FixedLeaseClock {
    fn now(&self) -> Timestamp {
        self.0
    }
}

/// 回收扫描报告（`reclaim_expired` 的结果，便于断言「无永久泄漏」）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReclaimReport {
    /// 本次因 TTL 到期被标记 `Expired` 的 lease 数。
    pub expired: u64,
    /// 本次回收到终态 `Reclaimed`（含刚 Expired 与历史 Released）的 lease 数。
    pub reclaimed: u64,
    /// 被回收的 lease 标识（审计用）。
    pub lease_ids: Vec<LeaseId>,
    /// 持久化失败计数（P18-4 主审：reclaim/restore 不得吞 projection error）。
    pub persist_errors: u64,
}

/// Lease 投影错误（真实后端可在 `.await` 挂起并失败；错误必须可传播，不再被
/// 旧「同步单次 poll」契约吞掉）。
#[derive(Debug, thiserror::Error)]
pub enum LeaseProjectionError {
    /// 后端存储错误（SQLite / 序列化等）。
    #[error("lease projection backend error: {0}")]
    Backend(String),
    /// 投影不可用（Actor 关闭 / mutex poisoned）。
    #[error("lease projection unavailable (closed/poisoned)")]
    Unavailable,
}

/// 可选的 lease 持久化投影（对象安全，ADR-016/033）。
///
/// 投影在**单个事务**内保存 lease 快照 + 追加事件（append-only event log），
/// 用于崩溃 / 重启后的恢复重放与审计。投影只携带无 secret 的定位 / 归属 / 期限信息。
///
/// 与旧契约的关键差异（P18-4 主审修复）：
/// - 方法返回 `Result`：真实后端（如 `DatabaseActor`）会在 `.await` 上挂起并可能失败，
///   错误必须可传播，不再被「同步单次 poll」契约吞掉；
/// - [`crate::credential::LeaseGuard`] 的 `Drop` 路径不再假定投影可同步完成：若首次 poll 返回
///   `Pending`，会把释放 future 交给 detached task 继续驱动到完成，避免永久额度泄漏。
///
/// - `apply`：事务化 upsert 快照 + 追加事件（终态快照可从活跃集移除，事件永久保留）；
/// - `settle`：强制把某 lease 移出活跃集合（GC / 显式结算，事件保留）；
/// - `load_outstanding`：读取所有非终态快照，供启动恢复重建池。
#[async_trait]
pub trait LeaseProjection: Send + Sync {
    /// 事务化持久化：写入 / 更新快照并追加事件。终态（`Reclaimed`）快照可由实现移除。
    async fn apply(
        &self,
        snapshot: &LeaseRecord,
        events: &[LeaseEvent],
    ) -> Result<(), LeaseProjectionError>;
    /// 把 lease 移出活跃集合（结算 / GC）；事件日志保留。
    async fn settle(&self, lease_id: &LeaseId) -> Result<(), LeaseProjectionError>;
    /// 读取所有非终态 lease 快照（恢复扫描用）。
    async fn load_outstanding(&self) -> Result<Vec<LeaseRecord>, LeaseProjectionError>;
}

/// 空投影（默认）：不做任何持久化，纯内存池使用。
#[derive(Clone, Copy, Debug, Default)]
pub struct NullLeaseProjection;

#[async_trait]
impl LeaseProjection for NullLeaseProjection {
    async fn apply(&self, _: &LeaseRecord, _: &[LeaseEvent]) -> Result<(), LeaseProjectionError> {
        Ok(())
    }
    async fn settle(&self, _: &LeaseId) -> Result<(), LeaseProjectionError> {
        Ok(())
    }
    async fn load_outstanding(&self) -> Result<Vec<LeaseRecord>, LeaseProjectionError> {
        Ok(Vec::new())
    }
}

/// 进程内投影（测试 / 组合层开发用）：单把 `Mutex` 同时保护非终态快照表与
/// append-only 事件日志，使 `apply` 天然事务化（单次加锁）。
pub struct InMemoryLeaseProjection {
    inner: std::sync::Mutex<InMemoryProjectionInner>,
}

#[derive(Default)]
struct InMemoryProjectionInner {
    snapshots: HashMap<LeaseId, LeaseRecord>,
    events: Vec<LeaseEvent>,
}

impl Default for InMemoryLeaseProjection {
    fn default() -> Self {
        Self::new()
    }
}

impl InMemoryLeaseProjection {
    /// 创建空投影。
    pub fn new() -> Self {
        Self {
            inner: std::sync::Mutex::new(InMemoryProjectionInner::default()),
        }
    }

    /// 当前持有的非终态快照数。
    pub fn len(&self) -> usize {
        self.inner
            .lock()
            .expect("InMemoryLeaseProjection mutex poisoned")
            .snapshots
            .len()
    }

    /// 是否为空。
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 已追加的事件总数（append-only 日志长度，断言「事件不再被丢弃」用）。
    pub fn event_count(&self) -> usize {
        self.inner
            .lock()
            .expect("InMemoryLeaseProjection mutex poisoned")
            .events
            .len()
    }

    /// 读取已追加的全部事件（append-only 顺序，断言事件序列 canonical 用）。
    pub fn events(&self) -> Vec<LeaseEvent> {
        self.inner
            .lock()
            .expect("InMemoryLeaseProjection mutex poisoned")
            .events
            .clone()
    }
}

#[async_trait]
impl LeaseProjection for InMemoryLeaseProjection {
    async fn apply(
        &self,
        snapshot: &LeaseRecord,
        events: &[LeaseEvent],
    ) -> Result<(), LeaseProjectionError> {
        let mut inner = self
            .inner
            .lock()
            .expect("InMemoryLeaseProjection mutex poisoned");
        if snapshot.state.is_terminal() {
            inner.snapshots.remove(&snapshot.lease_id);
        } else {
            inner
                .snapshots
                .insert(snapshot.lease_id.clone(), snapshot.clone());
        }
        inner.events.extend_from_slice(events);
        Ok(())
    }

    async fn settle(&self, lease_id: &LeaseId) -> Result<(), LeaseProjectionError> {
        self.inner
            .lock()
            .expect("InMemoryLeaseProjection mutex poisoned")
            .snapshots
            .remove(lease_id);
        Ok(())
    }

    async fn load_outstanding(&self) -> Result<Vec<LeaseRecord>, LeaseProjectionError> {
        Ok(self
            .inner
            .lock()
            .expect("InMemoryLeaseProjection mutex poisoned")
            .snapshots
            .values()
            .cloned()
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn req(account: &str) -> AcquireRequest {
        AcquireRequest {
            tenant_id: TenantId::new("tenant-a"),
            principal_id: PrincipalId::new("principal-a"),
            session_id: SessionId::new("session-a"),
            agent_id: AgentId::new("agent-a"),
            provider_id: None,
            account_id: Some(AccountId::new(account)),
            trace_id: None,
        }
    }

    #[test]
    fn state_db_str_round_trips() {
        for state in [
            LeaseState::Requested,
            LeaseState::Acquired,
            LeaseState::Released,
            LeaseState::Expired,
            LeaseState::Reclaimed,
        ] {
            assert_eq!(LeaseState::from_db_str(state.as_db_str()), Some(state));
        }
        assert!(LeaseState::from_db_str("bogus").is_none());
    }

    #[test]
    fn open_yields_acquired_version_two_with_two_events() {
        let clock = FixedLeaseClock::new(Timestamp::from_unix_millis(1_000));
        let (record, requested, acquired) = LeaseRecord::open(
            &req("acct-1"),
            LeaseId::new("lease-1"),
            CredentialId::new("cred-1"),
            &clock,
            5_000,
        );
        assert_eq!(record.state, LeaseState::Acquired);
        assert_eq!(record.version, 2);
        assert_eq!(record.acquired_at, Timestamp::from_unix_millis(1_000));
        assert_eq!(record.expires_at, Timestamp::from_unix_millis(6_000));
        assert_eq!(record.credential_id, CredentialId::new("cred-1"));
        assert_eq!(requested.version(), 1);
        assert_eq!(acquired.version(), 2);
        assert!(matches!(acquired, LeaseEvent::Acquired { .. }));
        // 记录不含 secret 字段：序列化对象不含 forbidden key。
        let json = serde_json::to_value(&record).unwrap();
        for forbidden in ["secret", "token", "api_key", "password"] {
            assert!(!json.as_object().unwrap().contains_key(forbidden));
        }
    }

    #[test]
    fn canonical_path_requested_acquired_released_reclaimed() {
        let clock = FixedLeaseClock::new(Timestamp::from_unix_millis(100));
        let (mut record, _, _) = LeaseRecord::open(
            &req("acct-1"),
            LeaseId::new("lease-1"),
            CredentialId::new("c"),
            &clock,
            1_000,
        );
        assert_eq!(record.version, 2);

        let (released, ev) = record
            .release(LeaseOutcome::Completed, &clock)
            .expect("Acquired -> Released");
        assert_eq!(released.state, LeaseState::Released);
        assert_eq!(released.version, 3);
        assert_eq!(ev.version(), 3);
        assert_eq!(released.outcome, Some(LeaseOutcome::Completed));
        record = released;

        let (reclaimed, ev) = record.reclaim(&clock).expect("Released -> Reclaimed");
        assert_eq!(reclaimed.state, LeaseState::Reclaimed);
        assert_eq!(reclaimed.version, 4);
        assert_eq!(ev.version(), 4);
    }

    #[test]
    fn expired_path_acquired_expired_reclaimed() {
        let clock = FixedLeaseClock::new(Timestamp::from_unix_millis(100));
        let (record, _, _) = LeaseRecord::open(
            &req("acct-1"),
            LeaseId::new("lease-1"),
            CredentialId::new("c"),
            &clock,
            1_000,
        );
        let (expired, ev) = record.expire(&clock).expect("Acquired -> Expired");
        assert_eq!(expired.state, LeaseState::Expired);
        assert_eq!(expired.version, 3);
        assert!(matches!(ev, LeaseEvent::Expired { .. }));
        let (reclaimed, _) = expired.reclaim(&clock).expect("Expired -> Reclaimed");
        assert_eq!(reclaimed.state, LeaseState::Reclaimed);
        assert_eq!(reclaimed.version, 4);
    }

    #[test]
    fn invalid_transitions_are_rejected() {
        let clock = FixedLeaseClock::new(Timestamp::from_unix_millis(0));
        let (acquired, _, _) = LeaseRecord::open(
            &req("acct-1"),
            LeaseId::new("lease-1"),
            CredentialId::new("c"),
            &clock,
            1_000,
        );
        // Acquired 不能直接 reclaim。
        assert!(acquired.clone().reclaim(&clock).is_err());
        // Released 不能 expire / release。
        let (released, _) = acquired.release(LeaseOutcome::Completed, &clock).unwrap();
        assert!(released.clone().expire(&clock).is_err());
        assert!(released
            .clone()
            .release(LeaseOutcome::Failed, &clock)
            .is_err());
        // Reclaimed 终态拒绝一切。
        let (reclaimed, _) = released.reclaim(&clock).unwrap();
        assert!(reclaimed.clone().reclaim(&clock).is_err());
        assert!(reclaimed
            .clone()
            .release(LeaseOutcome::Failed, &clock)
            .is_err());
        assert!(reclaimed.clone().expire(&clock).is_err());
    }

    #[test]
    fn is_past_ttl_only_for_acquired_past_deadline() {
        let clock = FixedLeaseClock::new(Timestamp::from_unix_millis(1_000));
        let (record, _, _) = LeaseRecord::open(
            &req("acct-1"),
            LeaseId::new("lease-1"),
            CredentialId::new("c"),
            &clock,
            2_000,
        );
        // expiry 边界为闭区间：now >= expires 即视为过期（不再宽限到严格大于）。
        assert!(!record.is_past_ttl(Timestamp::from_unix_millis(2_999)));
        assert!(record.is_past_ttl(Timestamp::from_unix_millis(3_000)));
        // 释放后即使过了 deadline 也不再视为 past-ttl（slot 已归还）。
        let (released, _) = record.release(LeaseOutcome::Completed, &clock).unwrap();
        assert!(!released.is_past_ttl(Timestamp::from_unix_millis(99_999)));
    }

    #[test]
    fn in_memory_projection_upsert_settle_load() {
        let projection = InMemoryLeaseProjection::new();
        let clock = FixedLeaseClock::new(Timestamp::from_unix_millis(0));
        let (record, _, _) = LeaseRecord::open(
            &req("acct-1"),
            LeaseId::new("lease-1"),
            CredentialId::new("c"),
            &clock,
            1_000,
        );
        block_on_sync(async {
            projection.apply(&record, &[]).await.unwrap();
            assert_eq!(projection.len(), 1);
            let outstanding = projection.load_outstanding().await.unwrap();
            assert_eq!(outstanding.len(), 1);
            assert_eq!(outstanding[0].lease_id, record.lease_id);
            assert_eq!(projection.event_count(), 0);
            projection.settle(&record.lease_id).await.unwrap();
            assert_eq!(projection.len(), 0);
        });
    }

    /// 在纯单测里提供一个最小 block_on，避免依赖 tokio runtime。
    /// 投影方法体无真实挂起点（同步契约），因此单次 poll 即完成。
    fn block_on_sync<F: std::future::Future>(future: F) -> F::Output {
        // 投影 async 方法体无真实挂起点（同步契约）；用一个 current-thread runtime
        // 驱动它，避免在非 #[tokio::test] 的 proptest 中手写 poll 循环。
        tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("build current-thread runtime for projection test")
            .block_on(future)
    }

    #[derive(Clone, Copy, Debug)]
    enum Cmd {
        Release,
        Expire,
        Reclaim,
    }

    proptest! {
        /// 随机命令序列：每条合法命令使 version 恰好 +1，且状态机永不离开 canonical 路径。
        #[test]
        fn lifecycle_invariants(
            ops in proptest::collection::vec(
                (0u8..3).prop_map(|i| match i {
                    0 => Cmd::Release,
                    1 => Cmd::Expire,
                    _ => Cmd::Reclaim,
                }),
                0..16,
            )
        ) {
            let clock = FixedLeaseClock::new(Timestamp::from_unix_millis(0));
            let (mut record, _, _) = LeaseRecord::open(
                &req("acct-1"),
                LeaseId::new("lease-1"),
                CredentialId::new("c"),
                &clock,
                1_000,
            );
            let mut expected_version = 2u64;
            for op in ops {
                let before = record.clone();
                let before_version = before.version;
                let res = match op {
                    Cmd::Release => before.release(LeaseOutcome::Completed, &clock).map(|(r, _)| r),
                    Cmd::Expire => before.expire(&clock).map(|(r, _)| r),
                    Cmd::Reclaim => before.reclaim(&clock).map(|(r, _)| r),
                };
                match res {
                    Ok(next) => {
                        expected_version = expected_version
                            .checked_add(1)
                            .expect("version 不溢出");
                        prop_assert_eq!(next.version, expected_version);
                        record = next;
                    }
                    Err(_) => {
                        // 非法转换：version 与 state 保持不变（用原记录继续）。
                        prop_assert_eq!(before_version, expected_version);
                    }
                }
                // 终态后任何转换必须失败（保持 version 不变）。
                if record.state.is_terminal() {
                    prop_assert!(record.clone().reclaim(&clock).is_err());
                }
            }
        }
    }
}
