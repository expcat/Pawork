//! P18-7 Session Affinity / Binding：versioned、evented 的粘性状态机（ADR-033）。
//!
//! 目的：健康 session 在请求间稳定复用 account/model，而在 cooldown、禁用、
//! capability / policy 变化或 TTL 到期后安全 rebind，且不跨 tenant 复用粘性。
//!
//! ```text
//! Unbound → Bound → Rebinding → Bound
//!                   └──────────────→ Released
//! ```
//!
//! - [`SessionBinding`] 是唯一的 canonical 绑定实体：每次状态转换 `revision`
//!   自增 1，并产生一条 [`BindingEvent`]；`ownership_epoch` 在每次 rebind
//!   成功后自增，与 `revision` 一起构成 CAS 守卫（乐观并发 + 所有权隔离）。
//! - **不复制状态机**：lease 生命周期由 `lease` 模块（Route → Lease）负责；
//!   绑定只引用 `lease_id`，不持有 lease 状态，不接触任何 Secret
//!   （`credential_id` 是 opaque 定位符）。
//! - 健康判定同样不在此处复制：复用前宿主经 `HealthView` 检查绑定目标是否
//!   健康，本模块只裁决 fingerprint / TTL（`resolve`）。
//! - 本模块是纯领域逻辑（无 I/O；投影 trait 外无 await）；[`BindingProjection`]
//!   是可选的对象安全持久化 sink（内存实现 + 组合层 SQLite 适配，见
//!   `session-store::binding` 的扁平行仓库）。
//!
//! 依赖方向：`provider-control → agent-domain`（仅引用 opaque ID）。

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

use agent_domain::{
    AccountId, AgentId, CredentialId, ModelId, PrincipalId, ProviderId, SessionId, TenantId,
};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    AcquireRequest, CredentialLease, CredentialPool, LeaseId, LeaseOutcome, LeaseState, PoolError,
    ReleaseReceipt, CONTROL_PLANE_SCHEMA_VERSION,
};

/// Binding 实体 schema 版本（与 `session-store` 的 `session_bindings` 迁移对齐）。
///
/// 所有 binding 行与 canonical event 携带该版本字段，支持版本化迁移与重放。
pub const BINDING_SCHEMA_VERSION: u32 = CONTROL_PLANE_SCHEMA_VERSION;

/// Binding 生命周期状态：canonical 单向状态机。
///
/// 转换路径固定为 `Unbound → Bound → Rebinding → Bound`（rebind 成功后回到
/// Bound），或 `Bound | Rebinding → Released`（终态），由 [`SessionBinding`]
/// 的转换方法强制；非法转换返回 [`BindingTransitionError`]。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BindingState {
    /// 未绑定：持久化投影中的「不存在」即 Unbound（行不落盘）。
    #[default]
    Unbound,
    /// 已绑定：持有 route target + fingerprint + lease 引用，可稳定复用。
    Bound,
    /// 重绑定中：CAS 已推进 revision，正在获取新 lease（单飞）。
    Rebinding,
    /// 已释放：终态，审计闭环完成，行可由 GC 回收（事件日志保留）。
    Released,
}

impl BindingState {
    /// 冻结的持久化字符串（与 `session-store` `session_bindings.state` 列对齐）。
    pub fn as_db_str(self) -> &'static str {
        match self {
            Self::Unbound => "unbound",
            Self::Bound => "bound",
            Self::Rebinding => "rebinding",
            Self::Released => "released",
        }
    }

    /// 由持久化字符串反解；未知值返回 `None`。
    pub fn from_db_str(value: &str) -> Option<Self> {
        match value {
            "unbound" => Some(Self::Unbound),
            "bound" => Some(Self::Bound),
            "rebinding" => Some(Self::Rebinding),
            "released" => Some(Self::Released),
            _ => None,
        }
    }

    /// 是否为终态（不可再转换）。
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Released)
    }
}

impl fmt::Display for BindingState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_db_str())
    }
}

/// Binding 唯一键：`(tenant, session, agent)`。
///
/// tenant 参与键 = 粘性绝不跨租户复用（Tenant A 永不复用 Tenant B 的 binding）。
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct BindingKey {
    pub tenant_id: TenantId,
    pub session_id: SessionId,
    pub agent_id: AgentId,
}

impl BindingKey {
    /// 由 `(tenant, session, agent)` 构造。
    pub fn new(tenant_id: TenantId, session_id: SessionId, agent_id: AgentId) -> Self {
        Self {
            tenant_id,
            session_id,
            agent_id,
        }
    }
}

impl fmt::Display for BindingKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{}/{}/{}",
            self.tenant_id, self.session_id, self.agent_id
        )
    }
}

/// 绑定目标（Route 选出的 winner，与 [`crate::routing::RouteCandidate`] 一一对应）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BindingTarget {
    pub provider_id: ProviderId,
    pub model_id: ModelId,
    pub account_id: AccountId,
    pub credential_id: CredentialId,
}

/// 亲和指纹：capability 需求 + 路由 / 租户策略配置的稳定哈希。
///
/// 任一变化都会使旧 binding 失效（[`SessionBinding::resolve`] 返回 `Rebind`），
/// 保证能力 / 策略热切换后不再复用过期粘性。哈希由宿主从 canonical 输入计算
/// （见 [`fingerprint_hash`]、[`capability_fingerprint`]、[`policy_fingerprint`]）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AffinityFingerprint {
    pub capability_hash: u64,
    pub policy_hash: u64,
}

/// FNV-1a 64：稳定、无依赖的确定性哈希（指纹用途，非加密、非防碰撞）。
///
/// 输入必须来自 canonical 序列化（如 serde 的确定性输出），否则哈希不稳定。
pub fn fingerprint_hash(bytes: &[u8]) -> u64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash = OFFSET;
    for &byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(PRIME);
    }
    hash
}

/// capability 需求 → 稳定指纹（P18-6 canonical [`crate::routing::Capability`]
/// 集合的确定性哈希；`BTreeSet` 的 serde 输出按 `Ord` 排序）。
#[cfg(feature = "account-control-v1")]
pub fn capability_fingerprint(
    required: &std::collections::BTreeSet<crate::routing::Capability>,
) -> u64 {
    fingerprint_hash(
        &serde_json::to_vec(required).expect("capability set serialization is infallible"),
    )
}

/// 路由策略配置 → 稳定指纹（serde 字段序固定，同配置必得同一字节序列）。
#[cfg(feature = "account-control-v1")]
pub fn policy_fingerprint(policy: &crate::routing::RoutingPolicy) -> u64 {
    fingerprint_hash(
        &serde_json::to_vec(policy).expect("routing policy serialization is infallible"),
    )
}

/// rebind 触发原因（进入 [`BindingEvent::RebindingStarted`] 的审计字段）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RebindReason {
    /// capability hash 变化（required capabilities / budget 语义变化）。
    CapabilityChanged,
    /// policy hash 变化（RoutingPolicy / 租户策略版本变化）。
    PolicyChanged,
    /// 绑定 TTL 到期。
    TtlExpired,
    /// 绑定引用的 lease 已非 `Acquired`（已释放 / 已过期 / 回收），不得复用。
    LeaseLost,
    /// 绑定处于 Rebinding（另一 rebind 单飞中，须重读后重试）。
    InFlight,
    /// 绑定已 Released（须重新 bind，而非 rebind）。
    Released,
}

impl RebindReason {
    /// 冻结的持久化字符串（与 serde snake_case 一致）。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::CapabilityChanged => "capability_changed",
            Self::PolicyChanged => "policy_changed",
            Self::TtlExpired => "ttl_expired",
            Self::LeaseLost => "lease_lost",
            Self::InFlight => "in_flight",
            Self::Released => "released",
        }
    }
}

/// 亲和裁决：复用当前 binding，还是需要 rebind。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AffinityDecision {
    /// 指纹一致、未过期且处于 Bound：确定性复用（粘性命中）。
    Reuse,
    /// 需要 rebind（先 CAS 进入 Rebinding，再 Route → Lease 获取新 lease）。
    Rebind(RebindReason),
}

/// 版本化、evented 的 canonical session binding（**不含任何 secret**）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionBinding {
    /// 实体 schema 版本。
    pub schema_version: u32,
    /// 当前生命周期状态。
    pub state: BindingState,
    /// 乐观并发版本号：初次 `Bound` 为 1，此后每次转换 +1；CAS 守卫之一。
    pub revision: u64,
    /// 所有权 epoch：每次 rebind 成功后 +1；CAS 守卫之一
    /// （旧 owner 无法 release / 续绑他人已 rebind 的 binding）。
    pub ownership_epoch: u64,
    /// 所属租户（键组成部分，粘性不得跨租户复用）。
    pub tenant_id: TenantId,
    /// 所属会话（键组成部分）。
    pub session_id: SessionId,
    /// 所属 Agent（键组成部分）。
    pub agent_id: AgentId,
    /// 绑定目标：Provider。
    pub provider_id: ProviderId,
    /// 绑定目标：模型。
    pub model_id: ModelId,
    /// 绑定目标：账号。
    pub account_id: AccountId,
    /// 绑定目标：凭据（opaque 定位符，caller 经 lease 解析短生命周期 secret）。
    pub credential_id: CredentialId,
    /// 绑定时的 capability 需求哈希。
    pub capability_hash: u64,
    /// 绑定时的 policy 配置哈希。
    pub policy_hash: u64,
    /// 复用 Route → Lease：只引用 lease_id，不复制 lease 状态机。
    pub lease_id: LeaseId,
    /// 绑定时刻（Unix 毫秒）。
    pub bound_at_ms: u64,
    /// 绑定 TTL（毫秒）。
    pub ttl_ms: u64,
    /// 过期时刻（`bound_at_ms + ttl_ms`）；到期后须 rebind。
    pub expires_at_ms: u64,
}

impl SessionBinding {
    /// 物化初始绑定：`Unbound → Bound`（revision 1），返回记录与一条 `Bound` 事件。
    ///
    /// 选路由 / 准入（Route → Lease）由调用方在持锁临界区内完成；本方法只构造
    /// canonical 记录与事件。`initial_epoch` 由宿主提供单调值（如 lease.version
    /// 或本地 epoch 源）。
    pub fn bind(
        key: BindingKey,
        target: BindingTarget,
        fingerprint: AffinityFingerprint,
        lease_id: LeaseId,
        initial_epoch: u64,
        now_ms: u64,
        ttl_ms: u64,
    ) -> (Self, BindingEvent) {
        let expires_at_ms = now_ms.saturating_add(ttl_ms);
        let record = Self {
            schema_version: BINDING_SCHEMA_VERSION,
            state: BindingState::Bound,
            revision: 1,
            ownership_epoch: initial_epoch,
            tenant_id: key.tenant_id,
            session_id: key.session_id,
            agent_id: key.agent_id,
            provider_id: target.provider_id,
            model_id: target.model_id,
            account_id: target.account_id,
            credential_id: target.credential_id,
            capability_hash: fingerprint.capability_hash,
            policy_hash: fingerprint.policy_hash,
            lease_id,
            bound_at_ms: now_ms,
            ttl_ms,
            expires_at_ms,
        };
        let event = record.bound_event();
        (record, event)
    }

    /// 绑定键 `(tenant, session, agent)`。
    pub fn key(&self) -> BindingKey {
        BindingKey {
            tenant_id: self.tenant_id.clone(),
            session_id: self.session_id.clone(),
            agent_id: self.agent_id.clone(),
        }
    }

    /// 当前指纹。
    pub fn fingerprint(&self) -> AffinityFingerprint {
        AffinityFingerprint {
            capability_hash: self.capability_hash,
            policy_hash: self.policy_hash,
        }
    }

    /// 当前绑定目标。
    pub fn target(&self) -> BindingTarget {
        BindingTarget {
            provider_id: self.provider_id.clone(),
            model_id: self.model_id.clone(),
            account_id: self.account_id.clone(),
            credential_id: self.credential_id.clone(),
        }
    }

    /// 是否已过 TTL（`Released` 视为不可用，恒 `true`）。
    pub fn is_expired(&self, now_ms: u64) -> bool {
        self.state.is_terminal() || now_ms >= self.expires_at_ms
    }

    /// 健康（宿主判定）且能力未变时的稳定命中：指纹一致、未过期、处于 Bound。
    pub fn is_current(&self, fingerprint: &AffinityFingerprint, now_ms: u64) -> bool {
        self.state == BindingState::Bound
            && self.fingerprint() == *fingerprint
            && !self.is_expired(now_ms)
    }

    /// 亲和裁决（P18-7 细分步骤 2：稳定命中）。
    ///
    /// 健康 / 并发准入不在本方法判定（Route → Lease 负责）；`Rebinding` 单飞中
    /// 的并发请求必须重读当前快照后重试，不得自行第二次 CAS。
    pub fn resolve(&self, fingerprint: &AffinityFingerprint, now_ms: u64) -> AffinityDecision {
        match self.state {
            BindingState::Bound => {
                if self.capability_hash != fingerprint.capability_hash {
                    AffinityDecision::Rebind(RebindReason::CapabilityChanged)
                } else if self.policy_hash != fingerprint.policy_hash {
                    AffinityDecision::Rebind(RebindReason::PolicyChanged)
                } else if self.is_expired(now_ms) {
                    AffinityDecision::Rebind(RebindReason::TtlExpired)
                } else {
                    AffinityDecision::Reuse
                }
            }
            BindingState::Rebinding => AffinityDecision::Rebind(RebindReason::InFlight),
            BindingState::Released | BindingState::Unbound => {
                AffinityDecision::Rebind(RebindReason::Released)
            }
        }
    }

    /// CAS 守卫校验：`(revision, ownership_epoch)` 必须与调用方读到的快照完全一致。
    fn check_guards(
        &self,
        expected_revision: u64,
        expected_epoch: u64,
    ) -> Result<(), BindingTransitionError> {
        if self.revision != expected_revision || self.ownership_epoch != expected_epoch {
            return Err(BindingTransitionError::GuardMismatch {
                key: self.key(),
                expected_revision,
                expected_epoch,
                actual_revision: self.revision,
                actual_epoch: self.ownership_epoch,
            });
        }
        Ok(())
    }

    fn bound_event(&self) -> BindingEvent {
        BindingEvent::Bound {
            version: self.revision,
            ownership_epoch: self.ownership_epoch,
            tenant_id: self.tenant_id.clone(),
            session_id: self.session_id.clone(),
            agent_id: self.agent_id.clone(),
            provider_id: self.provider_id.clone(),
            model_id: self.model_id.clone(),
            account_id: self.account_id.clone(),
            credential_id: self.credential_id.clone(),
            capability_hash: self.capability_hash,
            policy_hash: self.policy_hash,
            lease_id: self.lease_id.clone(),
            bound_at_ms: self.bound_at_ms,
            ttl_ms: self.ttl_ms,
            expires_at_ms: self.expires_at_ms,
        }
    }

    /// `Bound → Rebinding`：CAS 先推进 revision，再获取新 lease（安全单次 rebind
    /// 的第一步，细分步骤 3）。
    ///
    /// 守卫不匹配（他人已转换 / 已 rebind）或状态非 `Bound` 时报错，调用方不得
    /// 继续获取 lease；持久化 CAS（[`BindingProjection::compare_and_apply`]）
    /// 保证同一时刻只有一个调用方进入 Rebinding。
    pub fn begin_rebind(
        self,
        expected_revision: u64,
        expected_epoch: u64,
        reason: RebindReason,
        now_ms: u64,
    ) -> Result<(Self, BindingEvent), BindingTransitionError> {
        self.check_guards(expected_revision, expected_epoch)?;
        if self.state != BindingState::Bound {
            return Err(BindingTransitionError::InvalidRebind {
                key: self.key(),
                state: self.state,
            });
        }
        let version = self.revision.saturating_add(1);
        let event = BindingEvent::RebindingStarted {
            version,
            tenant_id: self.tenant_id.clone(),
            session_id: self.session_id.clone(),
            agent_id: self.agent_id.clone(),
            reason,
            at_ms: now_ms,
        };
        let mut record = self;
        record.state = BindingState::Rebinding;
        record.revision = version;
        Ok((record, event))
    }

    /// `Rebinding → Bound`：新 lease 已获取，提交新目标并推进 ownership epoch
    /// （安全单次 rebind 的第二步，细分步骤 3）。
    ///
    /// 目标 / 指纹 / lease 全部原子替换；`revision` 与 `ownership_epoch` 各 +1，
    /// 旧 owner 持有的旧 `(revision, epoch)` 此后全部失效。
    pub fn commit_rebind(
        self,
        expected_revision: u64,
        expected_epoch: u64,
        target: BindingTarget,
        fingerprint: AffinityFingerprint,
        lease_id: LeaseId,
        now_ms: u64,
        ttl_ms: u64,
    ) -> Result<(Self, BindingEvent), BindingTransitionError> {
        self.check_guards(expected_revision, expected_epoch)?;
        if self.state != BindingState::Rebinding {
            return Err(BindingTransitionError::InvalidCommit {
                key: self.key(),
                state: self.state,
            });
        }
        let version = self.revision.saturating_add(1);
        let epoch = self.ownership_epoch.saturating_add(1);
        let mut record = self;
        record.state = BindingState::Bound;
        record.revision = version;
        record.ownership_epoch = epoch;
        record.provider_id = target.provider_id;
        record.model_id = target.model_id;
        record.account_id = target.account_id;
        record.credential_id = target.credential_id;
        record.capability_hash = fingerprint.capability_hash;
        record.policy_hash = fingerprint.policy_hash;
        record.lease_id = lease_id;
        record.bound_at_ms = now_ms;
        record.ttl_ms = ttl_ms;
        record.expires_at_ms = now_ms.saturating_add(ttl_ms);
        let event = record.bound_event();
        Ok((record, event))
    }

    /// `Bound → Bound` 同目标活 lease 续绑：原子更新指纹 / TTL，保留当前 lease。
    ///
    /// account cap=1 时同账号的 TTL / 指纹变化走本路径：不 release、不重新
    /// acquire，旧 lease 仍是该 binding 的活 lease，天然避免「旧 lease 占位
    /// 自锁」。`revision` 与 `ownership_epoch` 各 +1，CAS 守卫使续绑同样单飞。
    pub fn renew_binding(
        self,
        expected_revision: u64,
        expected_epoch: u64,
        fingerprint: AffinityFingerprint,
        now_ms: u64,
        ttl_ms: u64,
    ) -> Result<(Self, BindingEvent), BindingTransitionError> {
        self.check_guards(expected_revision, expected_epoch)?;
        if self.state != BindingState::Bound {
            return Err(BindingTransitionError::InvalidRebind {
                key: self.key(),
                state: self.state,
            });
        }
        let version = self.revision.saturating_add(1);
        let epoch = self.ownership_epoch.saturating_add(1);
        let mut record = self;
        record.revision = version;
        record.ownership_epoch = epoch;
        record.capability_hash = fingerprint.capability_hash;
        record.policy_hash = fingerprint.policy_hash;
        record.bound_at_ms = now_ms;
        record.ttl_ms = ttl_ms;
        record.expires_at_ms = now_ms.saturating_add(ttl_ms);
        let event = record.bound_event();
        Ok((record, event))
    }

    /// `Released → Bound`：release 后的再次 bind 直接延续 generation。
    ///
    /// `revision` 与 `ownership_epoch` 各 +1（绝不重置 v1 / 复用旧 epoch），
    /// 事件日志与重放严格连续；CAS 守卫保证并发重绑恰好一个胜者。
    pub fn rebind_after_release(
        self,
        expected_revision: u64,
        expected_epoch: u64,
        target: BindingTarget,
        fingerprint: AffinityFingerprint,
        lease_id: LeaseId,
        now_ms: u64,
        ttl_ms: u64,
    ) -> Result<(Self, BindingEvent), BindingTransitionError> {
        self.check_guards(expected_revision, expected_epoch)?;
        if self.state != BindingState::Released {
            return Err(BindingTransitionError::InvalidRebind {
                key: self.key(),
                state: self.state,
            });
        }
        let version = self.revision.saturating_add(1);
        let epoch = self.ownership_epoch.saturating_add(1);
        let mut record = self;
        record.state = BindingState::Bound;
        record.revision = version;
        record.ownership_epoch = epoch;
        record.provider_id = target.provider_id;
        record.model_id = target.model_id;
        record.account_id = target.account_id;
        record.credential_id = target.credential_id;
        record.capability_hash = fingerprint.capability_hash;
        record.policy_hash = fingerprint.policy_hash;
        record.lease_id = lease_id;
        record.bound_at_ms = now_ms;
        record.ttl_ms = ttl_ms;
        record.expires_at_ms = now_ms.saturating_add(ttl_ms);
        let event = record.bound_event();
        Ok((record, event))
    }

    /// `Rebinding → Bound`（取消 / 新 lease 获取失败 / crash 恢复），内容不变。
    ///
    /// 崩溃重放后停留在 `Rebinding` 的记录可经本方法收敛回 `Bound`，再正常
    /// rebind，避免孤儿 Rebinding 永久阻塞后续请求（细分步骤 4：可恢复）。
    pub fn abort_rebind(
        self,
        expected_revision: u64,
        expected_epoch: u64,
        now_ms: u64,
    ) -> Result<(Self, BindingEvent), BindingTransitionError> {
        self.check_guards(expected_revision, expected_epoch)?;
        if self.state != BindingState::Rebinding {
            return Err(BindingTransitionError::InvalidAbort {
                key: self.key(),
                state: self.state,
            });
        }
        let version = self.revision.saturating_add(1);
        let event = BindingEvent::RebindingAborted {
            version,
            tenant_id: self.tenant_id.clone(),
            session_id: self.session_id.clone(),
            agent_id: self.agent_id.clone(),
            at_ms: now_ms,
        };
        let mut record = self;
        record.state = BindingState::Bound;
        record.revision = version;
        Ok((record, event))
    }

    /// `Bound | Rebinding → Released`：终态释放（session 结束 / 显式解绑）。
    ///
    /// 同样受 CAS 守卫保护：只有持有当前 `(revision, epoch)` 的 owner 能释放。
    pub fn release(
        self,
        expected_revision: u64,
        expected_epoch: u64,
        now_ms: u64,
    ) -> Result<(Self, BindingEvent), BindingTransitionError> {
        self.check_guards(expected_revision, expected_epoch)?;
        if !matches!(self.state, BindingState::Bound | BindingState::Rebinding) {
            return Err(BindingTransitionError::InvalidRelease {
                key: self.key(),
                state: self.state,
            });
        }
        let version = self.revision.saturating_add(1);
        let event = BindingEvent::Released {
            version,
            tenant_id: self.tenant_id.clone(),
            session_id: self.session_id.clone(),
            agent_id: self.agent_id.clone(),
            at_ms: now_ms,
        };
        let mut record = self;
        record.state = BindingState::Released;
        record.revision = version;
        Ok((record, event))
    }
}

/// 从 append-only 事件日志重放 binding 状态（crash / 重启恢复，细分步骤 4）。
///
/// - `Bound` 事件重建完整 Bound 记录（初次 bind 与 rebind commit 共用）；
/// - 后续事件必须属于同一键且 `version` 严格 +1，否则 fail-closed
///   （丢失事件 / 损坏日志不得静默产生不一致状态）。
pub fn apply_event(
    current: Option<SessionBinding>,
    event: &BindingEvent,
) -> Result<Option<SessionBinding>, BindingTransitionError> {
    match event {
        BindingEvent::Bound {
            version,
            ownership_epoch,
            tenant_id,
            session_id,
            agent_id,
            provider_id,
            model_id,
            account_id,
            credential_id,
            capability_hash,
            policy_hash,
            lease_id,
            bound_at_ms,
            ttl_ms,
            expires_at_ms,
        } => {
            let record = SessionBinding {
                schema_version: BINDING_SCHEMA_VERSION,
                state: BindingState::Bound,
                revision: *version,
                ownership_epoch: *ownership_epoch,
                tenant_id: tenant_id.clone(),
                session_id: session_id.clone(),
                agent_id: agent_id.clone(),
                provider_id: provider_id.clone(),
                model_id: model_id.clone(),
                account_id: account_id.clone(),
                credential_id: credential_id.clone(),
                capability_hash: *capability_hash,
                policy_hash: *policy_hash,
                lease_id: lease_id.clone(),
                bound_at_ms: *bound_at_ms,
                ttl_ms: *ttl_ms,
                expires_at_ms: *expires_at_ms,
            };
            if let Some(previous) = &current {
                if previous.key() != record.key() {
                    return Err(BindingTransitionError::Replay(format!(
                        "Bound event key {} does not match replay key {}",
                        record.key(),
                        previous.key()
                    )));
                }
                if record.revision != previous.revision.saturating_add(1) {
                    return Err(BindingTransitionError::Replay(format!(
                        "Bound event revision {} is not contiguous after {}",
                        record.revision, previous.revision
                    )));
                }
            }
            Ok(Some(record))
        }
        BindingEvent::RebindingStarted {
            version,
            tenant_id,
            session_id,
            agent_id,
            ..
        } => replay_transition(
            current,
            &BindingKey::new(tenant_id.clone(), session_id.clone(), agent_id.clone()),
            *version,
            BindingState::Rebinding,
            "RebindingStarted",
        ),
        BindingEvent::RebindingAborted {
            version,
            tenant_id,
            session_id,
            agent_id,
            ..
        } => replay_transition(
            current,
            &BindingKey::new(tenant_id.clone(), session_id.clone(), agent_id.clone()),
            *version,
            BindingState::Bound,
            "RebindingAborted",
        ),
        BindingEvent::Released {
            version,
            tenant_id,
            session_id,
            agent_id,
            ..
        } => replay_transition(
            current,
            &BindingKey::new(tenant_id.clone(), session_id.clone(), agent_id.clone()),
            *version,
            BindingState::Released,
            "Released",
        ),
    }
}

/// 非 `Bound` 事件的重放：键必须一致、version 必须严格 +1。
fn replay_transition(
    current: Option<SessionBinding>,
    key: &BindingKey,
    version: u64,
    state: BindingState,
    label: &str,
) -> Result<Option<SessionBinding>, BindingTransitionError> {
    let Some(mut record) = current else {
        return Err(BindingTransitionError::Replay(format!(
            "{label}: no current binding for key {key}"
        )));
    };
    if record.key() != *key {
        return Err(BindingTransitionError::Replay(format!(
            "{label}: event key {key} does not match replay key {}",
            record.key()
        )));
    }
    if version != record.revision.saturating_add(1) {
        return Err(BindingTransitionError::Replay(format!(
            "{label}: event revision {version} is not contiguous after {}",
            record.revision
        )));
    }
    record.revision = version;
    record.state = state;
    Ok(Some(record))
}

/// 状态转换产生的 canonical 事件（versioned，支持重放与审计，无 Secret）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum BindingEvent {
    /// 进入 `Bound`（初次 bind 或 rebind commit）。`version` = 转换后 revision。
    Bound {
        version: u64,
        ownership_epoch: u64,
        tenant_id: TenantId,
        session_id: SessionId,
        agent_id: AgentId,
        provider_id: ProviderId,
        model_id: ModelId,
        account_id: AccountId,
        credential_id: CredentialId,
        capability_hash: u64,
        policy_hash: u64,
        lease_id: LeaseId,
        bound_at_ms: u64,
        ttl_ms: u64,
        expires_at_ms: u64,
    },
    /// `Bound → Rebinding`：CAS 已推进 revision，单飞获取新 lease。
    RebindingStarted {
        version: u64,
        tenant_id: TenantId,
        session_id: SessionId,
        agent_id: AgentId,
        reason: RebindReason,
        at_ms: u64,
    },
    /// `Rebinding → Bound`（取消 / lease 获取失败 / 恢复），内容不变。
    RebindingAborted {
        version: u64,
        tenant_id: TenantId,
        session_id: SessionId,
        agent_id: AgentId,
        at_ms: u64,
    },
    /// `Bound | Rebinding → Released`：终态。
    Released {
        version: u64,
        tenant_id: TenantId,
        session_id: SessionId,
        agent_id: AgentId,
        at_ms: u64,
    },
}

impl BindingEvent {
    /// 事件所属的绑定键。
    pub fn key(&self) -> BindingKey {
        match self {
            Self::Bound {
                tenant_id,
                session_id,
                agent_id,
                ..
            }
            | Self::RebindingStarted {
                tenant_id,
                session_id,
                agent_id,
                ..
            }
            | Self::RebindingAborted {
                tenant_id,
                session_id,
                agent_id,
                ..
            }
            | Self::Released {
                tenant_id,
                session_id,
                agent_id,
                ..
            } => BindingKey::new(tenant_id.clone(), session_id.clone(), agent_id.clone()),
        }
    }

    /// 事件携带的 version（转换后的新 revision）。
    pub fn version(&self) -> u64 {
        match self {
            Self::Bound { version, .. }
            | Self::RebindingStarted { version, .. }
            | Self::RebindingAborted { version, .. }
            | Self::Released { version, .. } => *version,
        }
    }
}

/// 状态机转换错误。`Display` 不含 secret（binding 记录本身无 secret）。
#[derive(Debug, Error)]
pub enum BindingTransitionError {
    /// CAS 守卫不匹配：调用方基于过期快照，须重读后重试。
    #[error(
        "binding {key}: guard mismatch (expected revision {expected_revision} / \
         epoch {expected_epoch}, actual revision {actual_revision} / epoch {actual_epoch})"
    )]
    GuardMismatch {
        key: BindingKey,
        expected_revision: u64,
        expected_epoch: u64,
        actual_revision: u64,
        actual_epoch: u64,
    },
    /// `begin_rebind` 只能从 `Bound` 触发。
    #[error("binding {key}: invalid rebind from state {state:?}")]
    InvalidRebind {
        key: BindingKey,
        state: BindingState,
    },
    /// `commit_rebind` 只能从 `Rebinding` 触发。
    #[error("binding {key}: invalid commit from state {state:?}")]
    InvalidCommit {
        key: BindingKey,
        state: BindingState,
    },
    /// `abort_rebind` 只能从 `Rebinding` 触发。
    #[error("binding {key}: invalid abort from state {state:?}")]
    InvalidAbort {
        key: BindingKey,
        state: BindingState,
    },
    /// `release` 只能从 `Bound` / `Rebinding` 触发。
    #[error("binding {key}: invalid release from state {state:?}")]
    InvalidRelease {
        key: BindingKey,
        state: BindingState,
    },
    /// 事件重放失败（键不一致 / version 非连续 / 缺失前序事件）。
    #[error("binding event replay failed: {0}")]
    Replay(String),
}

/// Binding 投影错误（真实后端可在 `.await` 挂起并失败；错误必须可传播）。
#[derive(Debug, Error)]
pub enum BindingProjectionError {
    /// 后端存储错误（SQLite / 序列化等）。
    #[error("binding projection backend error: {0}")]
    Backend(String),
    /// 投影不可用（Actor 关闭 / mutex poisoned）。
    #[error("binding projection unavailable (closed/poisoned)")]
    Unavailable,
    /// 初始绑定冲突：键已存在（并发 double-bind 防护）。
    #[error("binding {key} already exists (insert conflict)")]
    AlreadyExists { key: BindingKey },
    /// 键不存在。
    #[error("binding {key} not found")]
    NotFound { key: BindingKey },
    /// 键已 Released（终态，须重新 bind）。
    #[error("binding {key} is already released")]
    AlreadyReleased { key: BindingKey },
    /// CAS 冲突：存储中的 `(revision, ownership_epoch)` 与期望不一致。
    #[error(
        "binding {key}: CAS conflict (expected revision {expected_revision} / \
         epoch {expected_epoch}, actual revision {actual_revision} / epoch {actual_epoch})"
    )]
    Conflict {
        key: BindingKey,
        expected_revision: u64,
        expected_epoch: u64,
        actual_revision: u64,
        actual_epoch: u64,
    },
    /// settle（GC）只能作用于 `Released` 行：非 Released 行拒绝 GC，fail-closed
    /// 防止把仍在使用的 Bound / Rebinding 行移出投影。
    #[error("binding {key} is not released (state {state:?}); settle denied")]
    NotReleased {
        key: BindingKey,
        state: BindingState,
    },
}

/// 可选的 binding 持久化投影（对象安全，ADR-016/033）。
///
/// 投影在**单个事务**内保存 binding 快照 + 追加事件（append-only event log），
/// 用于崩溃 / 重启后的恢复重放与审计；`compare_and_apply` 在存储层原子执行
/// `(revision, ownership_epoch)` CAS，是「安全单次 rebind」的持久化基础。
#[async_trait]
pub trait BindingProjection: Send + Sync {
    /// 初始绑定：键不存在才插入（否则 [`BindingProjectionError::AlreadyExists`]）；
    /// 事务化 snapshot + 事件。
    async fn insert(
        &self,
        snapshot: &SessionBinding,
        events: &[BindingEvent],
    ) -> Result<(), BindingProjectionError>;

    /// 乐观并发 CAS：当前行的 `(revision, ownership_epoch)` 完全匹配才覆盖，
    /// 并在同一事务追加事件。Released 行同样可被守卫匹配的 CAS 覆盖——
    /// `Released → Bound` 重绑经此延续 revision / epoch，事件重放保持连续。
    async fn compare_and_apply(
        &self,
        key: &BindingKey,
        expected_revision: u64,
        expected_epoch: u64,
        snapshot: &SessionBinding,
        events: &[BindingEvent],
    ) -> Result<(), BindingProjectionError>;

    /// 重放 / 对账修复：快照可覆盖，但事件按 `(binding key, version)` 幂等追加。
    /// 同版本同内容视为已应用；同版本不同内容或版本跳跃一律 fail-closed，避免
    /// 修复式重放制造重复 revision、破坏严格连续的 append-only 日志。
    async fn apply(
        &self,
        snapshot: &SessionBinding,
        events: &[BindingEvent],
    ) -> Result<(), BindingProjectionError>;

    /// 读取当前快照（含 Released；不存在返回 `None`）。
    async fn load(
        &self,
        key: &BindingKey,
    ) -> Result<Option<SessionBinding>, BindingProjectionError>;

    /// 所有非 Released 快照（恢复扫描 / 孤儿 Rebinding 收敛用）。
    async fn load_outstanding(&self) -> Result<Vec<SessionBinding>, BindingProjectionError>;

    /// Released 行移出活跃集合（GC）；事件日志保留。
    ///
    /// **只允许 `Released`**：行存在但非 Released 返回
    /// [`BindingProjectionError::NotReleased`]；行不存在视为已 GC（幂等 `Ok`）。
    async fn settle(&self, key: &BindingKey) -> Result<(), BindingProjectionError>;

    /// 该键的世代高水位 `(revision, ownership_epoch)`：从**保留的事件日志**读出
    /// 最近一次转换后的 version 与最近一条 `Bound` 事件的 epoch。行被 settle
    /// （GC）后仍可用它延续 generation——GC 后再 bind 必须从 `(revision+1,
    /// epoch+1)` 继续，绝不重置 v1 / 复用旧 epoch。无任何历史返回 `None`。
    async fn continuation(
        &self,
        key: &BindingKey,
    ) -> Result<Option<(u64, u64)>, BindingProjectionError>;
}

/// 空投影（默认）：不做任何持久化；`load` 恒为 `None`，CAS 恒 `NotFound`。
#[derive(Clone, Copy, Debug, Default)]
pub struct NullBindingProjection;

#[async_trait]
impl BindingProjection for NullBindingProjection {
    async fn insert(
        &self,
        _snapshot: &SessionBinding,
        _events: &[BindingEvent],
    ) -> Result<(), BindingProjectionError> {
        Ok(())
    }

    async fn compare_and_apply(
        &self,
        key: &BindingKey,
        _expected_revision: u64,
        _expected_epoch: u64,
        _snapshot: &SessionBinding,
        _events: &[BindingEvent],
    ) -> Result<(), BindingProjectionError> {
        Err(BindingProjectionError::NotFound { key: key.clone() })
    }

    async fn apply(
        &self,
        _snapshot: &SessionBinding,
        _events: &[BindingEvent],
    ) -> Result<(), BindingProjectionError> {
        Ok(())
    }

    async fn load(
        &self,
        _key: &BindingKey,
    ) -> Result<Option<SessionBinding>, BindingProjectionError> {
        Ok(None)
    }

    async fn load_outstanding(&self) -> Result<Vec<SessionBinding>, BindingProjectionError> {
        Ok(Vec::new())
    }

    async fn settle(&self, _key: &BindingKey) -> Result<(), BindingProjectionError> {
        Ok(())
    }

    async fn continuation(
        &self,
        _key: &BindingKey,
    ) -> Result<Option<(u64, u64)>, BindingProjectionError> {
        Ok(None)
    }
}

/// 进程内投影（测试 / 组合层开发用）：单把 `Mutex` 同时保护快照表与
/// append-only 事件日志，`insert` / `compare_and_apply` 天然事务化（单次加锁）。
pub struct InMemoryBindingProjection {
    inner: std::sync::Mutex<InMemoryBindingInner>,
}

#[derive(Default)]
struct InMemoryBindingInner {
    snapshots: HashMap<BindingKey, SessionBinding>,
    events: Vec<BindingEvent>,
}

impl Default for InMemoryBindingProjection {
    fn default() -> Self {
        Self::new()
    }
}

impl InMemoryBindingProjection {
    /// 创建空投影。
    pub fn new() -> Self {
        Self {
            inner: std::sync::Mutex::new(InMemoryBindingInner::default()),
        }
    }

    /// 当前持有的快照数（含 Released）。
    pub fn len(&self) -> usize {
        self.inner
            .lock()
            .expect("InMemoryBindingProjection mutex poisoned")
            .snapshots
            .len()
    }

    /// 是否为空。
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 已追加的事件总数（append-only 日志长度，断言「事件不被丢弃」用）。
    pub fn event_count(&self) -> usize {
        self.inner
            .lock()
            .expect("InMemoryBindingProjection mutex poisoned")
            .events
            .len()
    }

    /// 读取已追加的全部事件（append-only 顺序，断言事件序列 canonical 用）。
    pub fn events(&self) -> Vec<BindingEvent> {
        self.inner
            .lock()
            .expect("InMemoryBindingProjection mutex poisoned")
            .events
            .clone()
    }

    /// 直接读取某键快照（与 `BindingProjection::load` 同语义）。
    pub fn snapshot(&self, key: &BindingKey) -> Option<SessionBinding> {
        self.inner
            .lock()
            .expect("InMemoryBindingProjection mutex poisoned")
            .snapshots
            .get(key)
            .cloned()
    }
}

#[async_trait]
impl BindingProjection for InMemoryBindingProjection {
    async fn insert(
        &self,
        snapshot: &SessionBinding,
        events: &[BindingEvent],
    ) -> Result<(), BindingProjectionError> {
        let mut inner = self
            .inner
            .lock()
            .expect("InMemoryBindingProjection mutex poisoned");
        if inner.snapshots.contains_key(&snapshot.key()) {
            return Err(BindingProjectionError::AlreadyExists {
                key: snapshot.key(),
            });
        }
        inner.snapshots.insert(snapshot.key(), snapshot.clone());
        inner.events.extend_from_slice(events);
        Ok(())
    }

    async fn compare_and_apply(
        &self,
        key: &BindingKey,
        expected_revision: u64,
        expected_epoch: u64,
        snapshot: &SessionBinding,
        events: &[BindingEvent],
    ) -> Result<(), BindingProjectionError> {
        let mut inner = self
            .inner
            .lock()
            .expect("InMemoryBindingProjection mutex poisoned");
        let Some(current) = inner.snapshots.get(key) else {
            return Err(BindingProjectionError::NotFound { key: key.clone() });
        };
        if current.revision != expected_revision || current.ownership_epoch != expected_epoch {
            return Err(BindingProjectionError::Conflict {
                key: key.clone(),
                expected_revision,
                expected_epoch,
                actual_revision: current.revision,
                actual_epoch: current.ownership_epoch,
            });
        }
        inner.snapshots.insert(key.clone(), snapshot.clone());
        inner.events.extend_from_slice(events);
        Ok(())
    }

    async fn apply(
        &self,
        snapshot: &SessionBinding,
        events: &[BindingEvent],
    ) -> Result<(), BindingProjectionError> {
        let mut inner = self
            .inner
            .lock()
            .expect("InMemoryBindingProjection mutex poisoned");
        let key = snapshot.key();
        let mut existing_by_version = inner
            .events
            .iter()
            .filter(|event| event.key() == key)
            .map(|event| (event.version(), event.clone()))
            .collect::<HashMap<_, _>>();
        let mut high_watermark = existing_by_version.keys().copied().max().unwrap_or(0);
        let mut pending = Vec::new();
        for event in events {
            if event.key() != key {
                return Err(BindingProjectionError::Backend(format!(
                    "binding replay event key {} does not match snapshot key {key}",
                    event.key()
                )));
            }
            let version = event.version();
            if let Some(existing) = existing_by_version.get(&version) {
                if existing != event {
                    return Err(BindingProjectionError::Backend(format!(
                        "binding {key}: conflicting replay event at revision {version}"
                    )));
                }
                continue;
            }
            let expected = high_watermark.saturating_add(1);
            if version != expected {
                return Err(BindingProjectionError::Backend(format!(
                    "binding {key}: replay revision {version} is not contiguous after {high_watermark}"
                )));
            }
            high_watermark = version;
            existing_by_version.insert(version, event.clone());
            pending.push(event.clone());
        }
        inner.snapshots.insert(key, snapshot.clone());
        inner.events.extend(pending);
        Ok(())
    }

    async fn load(
        &self,
        key: &BindingKey,
    ) -> Result<Option<SessionBinding>, BindingProjectionError> {
        Ok(self
            .inner
            .lock()
            .expect("InMemoryBindingProjection mutex poisoned")
            .snapshots
            .get(key)
            .cloned())
    }

    async fn load_outstanding(&self) -> Result<Vec<SessionBinding>, BindingProjectionError> {
        Ok(self
            .inner
            .lock()
            .expect("InMemoryBindingProjection mutex poisoned")
            .snapshots
            .values()
            .filter(|snapshot| !snapshot.state.is_terminal())
            .cloned()
            .collect())
    }

    async fn settle(&self, key: &BindingKey) -> Result<(), BindingProjectionError> {
        let mut inner = self
            .inner
            .lock()
            .expect("InMemoryBindingProjection mutex poisoned");
        // 只允许 Released 行 GC：Bound / Rebinding 行仍在使用，拒绝删除。
        let state = match inner.snapshots.get(key) {
            None => return Ok(()),
            Some(snapshot) => snapshot.state,
        };
        if state != BindingState::Released {
            return Err(BindingProjectionError::NotReleased {
                key: key.clone(),
                state,
            });
        }
        inner.snapshots.remove(key);
        Ok(())
    }

    async fn continuation(
        &self,
        key: &BindingKey,
    ) -> Result<Option<(u64, u64)>, BindingProjectionError> {
        let inner = self
            .inner
            .lock()
            .expect("InMemoryBindingProjection mutex poisoned");
        let mut revision = None;
        let mut epoch = None;
        for event in &inner.events {
            if event.key() != *key {
                continue;
            }
            revision = Some(event.version());
            if let BindingEvent::Bound {
                ownership_epoch, ..
            } = event
            {
                epoch = Some(*ownership_epoch);
            }
        }
        Ok(revision.zip(epoch))
    }
}

/// 单次协调结果：提交后的 canonical binding + 旧 lease 释放证据。
#[derive(Debug)]
pub struct BindingAcquisition {
    /// 提交后的 canonical binding（`Bound`）。
    pub binding: SessionBinding,
    /// 成功 rebind 后旧 lease 的释放结果；初次 bind / 稳定复用为 `None`。
    pub old_lease_release: Option<Result<ReleaseReceipt, PoolError>>,
}

/// 一次 acquire / rebind 协调的输入（键 + 目标 + 指纹 + 主体 + 时钟 / TTL）。
#[derive(Clone, Debug)]
pub struct RebindRequest {
    /// 绑定键 `(tenant, session, agent)`。
    pub key: BindingKey,
    /// Route 选出的目标（provider / model / account / credential）。
    pub target: BindingTarget,
    /// capability / policy 亲和指纹。
    pub fingerprint: AffinityFingerprint,
    /// 发起主体（lease 所有权）。
    pub principal_id: PrincipalId,
    /// 裁决时刻（Unix 毫秒）。
    pub now_ms: u64,
    /// 绑定 TTL（毫秒）。
    pub ttl_ms: u64,
}

/// 协调器错误（fail-closed；`Display` 不含任何 secret）。
#[derive(Debug, Error)]
pub enum BindingServiceError {
    /// 投影读写失败。
    #[error(transparent)]
    Projection(#[from] BindingProjectionError),
    /// 池 acquire / release 失败（已按流程回滚，或无需回滚）。
    #[error(transparent)]
    Pool(#[from] PoolError),
    /// 状态机转换失败（如单飞中被第二次 rebind）。
    #[error(transparent)]
    Transition(#[from] BindingTransitionError),
    /// 初始 bind 冲突：本次 acquire 的 lease 已被释放（幂等双绑防护）。
    #[error(
        "binding {key}: initial bind conflict ({conflict}); new lease release: {new_lease_release:?}"
    )]
    BindConflict {
        key: BindingKey,
        conflict: Box<BindingProjectionError>,
        new_lease_release: Result<ReleaseReceipt, Box<PoolError>>,
    },
    /// commit CAS 冲突：本次 acquire 的 lease 已被释放，绝不泄漏额度。
    #[error(
        "binding {key}: commit CAS conflict ({conflict}); new lease release: {new_lease_release:?}"
    )]
    CommitConflict {
        key: BindingKey,
        conflict: Box<BindingProjectionError>,
        new_lease_release: Result<ReleaseReceipt, Box<PoolError>>,
    },
    /// acquire 失败且 abort 回滚也失败：残留 Rebinding 孤儿，须 [`SessionBindingService::recover_outstanding`]。
    #[error("binding {key}: acquire failed ({pool}) and abort rollback failed: {abort}")]
    AcquireFailedAbortFailed {
        key: BindingKey,
        pool: Box<PoolError>,
        abort: Box<BindingProjectionError>,
    },
    /// pre-release（释放自持旧 lease）后重试 acquire 仍失败（或旧 lease 释放
    /// 本身失败）：binding 已 fail-closed 转 `Released`，**绝不回到 Bound 引用
    /// 已释放 lease**；`released` 为该 Released CAS 的结果（失败时残留
    /// Rebinding 孤儿，须 [`SessionBindingService::recover_outstanding`]）。
    #[error("binding {key}: pre-release retry failed ({pool}); released transition: {released:?}")]
    PreReleaseRetryFailed {
        key: BindingKey,
        pool: Box<PoolError>,
        released: Result<(), Box<BindingProjectionError>>,
    },
    /// 投影返回的 snapshot 键与请求键不一致（防御性 fail-closed）。
    #[error("binding projection returned snapshot for {actual} while {requested} was requested")]
    KeyMismatch {
        requested: Box<BindingKey>,
        actual: Box<BindingKey>,
    },
    /// 事件的绑定键与 CAS 键不一致（防御性 fail-closed）。
    #[error("binding {key}: event key {event} does not match CAS key")]
    EventKeyMismatch {
        key: Box<BindingKey>,
        event: Box<BindingKey>,
    },
    /// Reuse 校验：池无法观测 lease 状态（`lease_state` 返回 `None`），fail-closed。
    #[error("binding {key}: pool cannot observe state of lease {lease}")]
    LeaseStateUnobservable { key: BindingKey, lease: LeaseId },
}

/// P18-7 单飞协调器：真实执行「Binding CAS → CredentialPool acquire → commit / abort」。
///
/// 不复制 lease 状态机：acquire / release 全部委托给 [`CredentialPool`]
/// （Route → Lease），binding 只引用 opaque `lease_id`，不接触任何 Secret。
pub struct SessionBindingService<P, B> {
    pool: Arc<P>,
    projection: Arc<B>,
}

impl<P, B> Clone for SessionBindingService<P, B> {
    fn clone(&self) -> Self {
        Self {
            pool: self.pool.clone(),
            projection: self.projection.clone(),
        }
    }
}

impl<P, B> SessionBindingService<P, B>
where
    P: CredentialPool,
    B: BindingProjection,
{
    /// 以真实池与投影构造。
    pub fn new(pool: Arc<P>, projection: Arc<B>) -> Self {
        Self { pool, projection }
    }

    /// 全流程单飞：load → 键校验 → 亲和裁决 → 稳定复用 / 初始 bind / 安全 rebind。
    ///
    /// - `Reuse`：先校验 lease 仍 `Acquired` 才返回当前 binding；lease 已死则
    ///   转 `LeaseLost` rebind，池无法观测 lease 状态时 fail-closed。
    /// - 初始 bind（无行 / `Released`）：先 acquire，再 `insert`；`AlreadyExists`
    ///   冲突时释放本次 lease 后上抛。
    /// - 安全 rebind：先 CAS 进入 `Rebinding`（冲突不触碰 pool），再 acquire；
    ///   同目标活 lease 走续绑（不触碰 pool）；同账号旧 lease 占位（cap 满）先
    ///   释放旧 lease 再重试一次 acquire；commit CAS 冲突释放新 lease；commit
    ///   成功后释放旧 lease（新 binding 已可见，安全顺序）。
    /// - `Released`：不 settle / 不重置，直接 CAS 延续 revision/epoch 重绑。
    /// - 无行但存在保留事件历史（Released 行已 settle / GC）：从事件日志高水位
    ///   延续 revision/epoch，绝不重置 v1 / 复用旧 epoch。
    pub async fn acquire_binding(
        &self,
        request: RebindRequest,
    ) -> Result<BindingAcquisition, BindingServiceError> {
        let snapshot = self.projection.load(&request.key).await?;
        if let Some(seen) = &snapshot {
            Self::check_snapshot_key(&request.key, seen)?;
        }
        match snapshot {
            None => {
                let continuation = self.projection.continuation(&request.key).await?;
                self.initial_bind(&request, continuation).await
            }
            Some(current) => match current.resolve(&request.fingerprint, request.now_ms) {
                AffinityDecision::Reuse => match self.pool.lease_state(&current.lease_id) {
                    Some(LeaseState::Acquired) => Ok(BindingAcquisition {
                        binding: current,
                        old_lease_release: None,
                    }),
                    None => Err(BindingServiceError::LeaseStateUnobservable {
                        key: request.key.clone(),
                        lease: current.lease_id.clone(),
                    }),
                    Some(_) => {
                        self.safe_rebind(&request, current, RebindReason::LeaseLost)
                            .await
                    }
                },
                AffinityDecision::Rebind(RebindReason::Released) => {
                    self.rebind_after_release(&request, current).await
                }
                AffinityDecision::Rebind(reason) => {
                    self.safe_rebind(&request, current, reason).await
                }
            },
        }
    }

    /// 崩溃恢复：把残留的 `Rebinding` 快照收敛（孤儿收敛，只观测 pool、不产生
    /// acquire / release 流量）。
    ///
    /// pre-release 后崩溃的孤儿（旧 lease 已释放 / 不可观测）**绝不 abort 回
    /// Bound 引用已释放 lease**：只有池仍观测到 `Acquired` 才 abort 回 Bound，
    /// 否则 fail-closed 转 `Released`。
    ///
    /// 返回成功收敛的数量；单个键的 CAS 冲突（他人已完成转换）跳过。
    pub async fn recover_outstanding(&self, now_ms: u64) -> Result<usize, BindingServiceError> {
        let outstanding = self.projection.load_outstanding().await?;
        let mut recovered = 0usize;
        for snapshot in outstanding {
            if snapshot.state != BindingState::Rebinding {
                continue;
            }
            let key = snapshot.key();
            let guards = (snapshot.revision, snapshot.ownership_epoch);
            let (transitioned, event) = match self.pool.lease_state(&snapshot.lease_id) {
                Some(LeaseState::Acquired) => snapshot.abort_rebind(guards.0, guards.1, now_ms)?,
                _ => snapshot.release(guards.0, guards.1, now_ms)?,
            };
            Self::check_event_keys(&key, std::slice::from_ref(&event))?;
            match self
                .projection
                .compare_and_apply(&key, guards.0, guards.1, &transitioned, &[event])
                .await
            {
                Ok(()) => recovered += 1,
                Err(
                    BindingProjectionError::Conflict { .. }
                    | BindingProjectionError::NotFound { .. }
                    | BindingProjectionError::AlreadyReleased { .. },
                ) => {}
                Err(other) => return Err(BindingServiceError::Projection(other)),
            }
        }
        Ok(recovered)
    }

    /// `None` 后的初始 bind：acquire → `insert`，冲突释放本次 lease。
    ///
    /// `continuation` 为 settle（GC）后从保留事件日志读出的世代高水位：存在时
    /// 以 Released 墓碑延续 `revision / epoch`（各 +1），事件严格连续。
    async fn initial_bind(
        &self,
        request: &RebindRequest,
        continuation: Option<(u64, u64)>,
    ) -> Result<BindingAcquisition, BindingServiceError> {
        let lease = self
            .pool
            .acquire(Self::acquire_request(
                &request.key,
                &request.target,
                request.principal_id.clone(),
            ))
            .await?;
        let effective = effective_target(&request.target, &lease);
        let (snapshot, event) = match continuation {
            None => SessionBinding::bind(
                request.key.clone(),
                effective,
                request.fingerprint,
                lease.lease_id.clone(),
                // ownership_epoch 从 0 起步并在每次 rebind / release 重绑时 +1：
                // 不借用 lease.version（按账号循环，跨代可重复），保证代际严格单调。
                0,
                request.now_ms,
                request.ttl_ms,
            ),
            Some((revision, epoch)) => {
                // GC 后再 bind：重建 Released 墓碑并延续 generation。
                let tombstone = SessionBinding {
                    schema_version: BINDING_SCHEMA_VERSION,
                    state: BindingState::Released,
                    revision,
                    ownership_epoch: epoch,
                    tenant_id: request.key.tenant_id.clone(),
                    session_id: request.key.session_id.clone(),
                    agent_id: request.key.agent_id.clone(),
                    provider_id: effective.provider_id.clone(),
                    model_id: effective.model_id.clone(),
                    account_id: effective.account_id.clone(),
                    credential_id: effective.credential_id.clone(),
                    capability_hash: request.fingerprint.capability_hash,
                    policy_hash: request.fingerprint.policy_hash,
                    lease_id: lease.lease_id.clone(),
                    bound_at_ms: request.now_ms,
                    ttl_ms: request.ttl_ms,
                    expires_at_ms: request.now_ms.saturating_add(request.ttl_ms),
                };
                tombstone.rebind_after_release(
                    revision,
                    epoch,
                    effective,
                    request.fingerprint,
                    lease.lease_id.clone(),
                    request.now_ms,
                    request.ttl_ms,
                )?
            }
        };
        Self::check_event_keys(&request.key, std::slice::from_ref(&event))?;
        match self.projection.insert(&snapshot, &[event]).await {
            Ok(()) => Ok(BindingAcquisition {
                binding: snapshot,
                old_lease_release: None,
            }),
            Err(error) => {
                let new_lease_release = self
                    .pool
                    .release(lease.lease_id.clone(), LeaseOutcome::Cancelled)
                    .await;
                match error {
                    BindingProjectionError::AlreadyExists { .. } => {
                        Err(BindingServiceError::BindConflict {
                            key: request.key.clone(),
                            conflict: Box::new(error),
                            new_lease_release: new_lease_release.map_err(Box::new),
                        })
                    }
                    other => {
                        let _ = new_lease_release;
                        Err(BindingServiceError::Projection(other))
                    }
                }
            }
        }
    }

    /// 安全单次 rebind（细分步骤 3）：CAS → acquire → commit / abort。
    ///
    /// 同目标且旧 lease 仍 `Acquired` 时走续绑（`renew_binding`），不触碰 pool；
    /// 否则走 replacement：acquire 被自持旧 lease 占位（同账号 / 同租户 cap
    /// 满）时先释放旧 lease 再重试恰好一次；pre-release 后重试仍失败（或释放
    /// 本身失败）时 **fail-closed 转 `Released`，绝不 abort 回 Bound 引用已
    /// 释放 lease**。
    async fn safe_rebind(
        &self,
        request: &RebindRequest,
        current: SessionBinding,
        reason: RebindReason,
    ) -> Result<BindingAcquisition, BindingServiceError> {
        let key = &request.key;
        if same_target(&current, &request.target)
            && self.pool.lease_state(&current.lease_id) == Some(LeaseState::Acquired)
        {
            return self.renew_binding(request, current).await;
        }
        let old_lease_id = current.lease_id.clone();
        let old_account_id = current.account_id.clone();
        let begin_guards = (current.revision, current.ownership_epoch);
        let (rebinding, started) =
            current.begin_rebind(begin_guards.0, begin_guards.1, reason, request.now_ms)?;
        Self::check_event_keys(key, std::slice::from_ref(&started))?;
        // 第一步 CAS：同一时刻只有一个调用方进入 Rebinding；冲突不触碰 pool。
        self.projection
            .compare_and_apply(key, begin_guards.0, begin_guards.1, &rebinding, &[started])
            .await?;

        let mut pre_release: Option<Result<ReleaseReceipt, PoolError>> = None;
        let lease = match self
            .pool
            .acquire(Self::acquire_request(
                key,
                &request.target,
                request.principal_id.clone(),
            ))
            .await
        {
            Ok(lease) => lease,
            Err(error)
                if is_self_cap_exhaustion(&error, &old_account_id, &request.key.tenant_id) =>
            {
                // 自持旧 lease 占位（同账号 / 同租户 cap 满）：先安全释放旧 lease，
                // 再重试恰好一次；释放失败同样 fail-closed 转 Released。
                match self
                    .pool
                    .release(old_lease_id.clone(), LeaseOutcome::Completed)
                    .await
                {
                    Ok(receipt) => pre_release = Some(Ok(receipt)),
                    Err(pool_error) => {
                        return Err(self
                            .release_and_fail(key, rebinding, pool_error, request.now_ms)
                            .await);
                    }
                }
                match self
                    .pool
                    .acquire(Self::acquire_request(
                        key,
                        &request.target,
                        request.principal_id.clone(),
                    ))
                    .await
                {
                    Ok(lease) => lease,
                    Err(pool_error) => {
                        return Err(self
                            .release_and_fail(key, rebinding, pool_error, request.now_ms)
                            .await);
                    }
                }
            }
            Err(pool_error) => {
                return Err(self
                    .abort_and_fail(key, rebinding, pool_error, request.now_ms)
                    .await);
            }
        };
        let effective = effective_target(&request.target, &lease);
        let commit_guards = (rebinding.revision, rebinding.ownership_epoch);
        let (committed, rebound) = rebinding.commit_rebind(
            commit_guards.0,
            commit_guards.1,
            effective,
            request.fingerprint,
            lease.lease_id.clone(),
            request.now_ms,
            request.ttl_ms,
        )?;
        Self::check_event_keys(key, std::slice::from_ref(&rebound))?;
        match self
            .projection
            .compare_and_apply(
                key,
                commit_guards.0,
                commit_guards.1,
                &committed,
                &[rebound],
            )
            .await
        {
            Ok(()) => {
                // 新 binding 已可见后再释放旧 lease：即便释放失败也不回滚已提交的 rebind。
                let old_lease_release = match pre_release {
                    Some(receipt) => Some(receipt),
                    None => Some(
                        self.pool
                            .release(old_lease_id, LeaseOutcome::Completed)
                            .await,
                    ),
                };
                Ok(BindingAcquisition {
                    binding: committed,
                    old_lease_release,
                })
            }
            Err(conflict) => {
                // commit 冲突：本次 acquire 的 lease 必须释放，绝不泄漏额度。
                let new_lease_release = self
                    .pool
                    .release(lease.lease_id.clone(), LeaseOutcome::Cancelled)
                    .await;
                Err(BindingServiceError::CommitConflict {
                    key: key.clone(),
                    conflict: Box::new(conflict),
                    new_lease_release: new_lease_release.map_err(Box::new),
                })
            }
        }
    }

    /// 同目标活 lease 续绑：单一 CAS（Bound → Bound，revision / epoch +1），
    /// 不 release、不 acquire，cap=1 下同账号 TTL / 指纹变化不会自锁。
    async fn renew_binding(
        &self,
        request: &RebindRequest,
        current: SessionBinding,
    ) -> Result<BindingAcquisition, BindingServiceError> {
        let key = &request.key;
        let guards = (current.revision, current.ownership_epoch);
        let (renewed, event) = current.renew_binding(
            guards.0,
            guards.1,
            request.fingerprint,
            request.now_ms,
            request.ttl_ms,
        )?;
        Self::check_event_keys(key, std::slice::from_ref(&event))?;
        self.projection
            .compare_and_apply(key, guards.0, guards.1, &renewed, &[event])
            .await?;
        Ok(BindingAcquisition {
            binding: renewed,
            old_lease_release: None,
        })
    }

    /// `Released → Bound`：不 settle、不重置 v1，CAS 延续 revision / epoch
    /// （各 +1），事件日志与重放严格连续；成功后再幂等释放 Released 代遗留 lease。
    ///
    /// `release_binding` 池释放失败会留下「行已 Released、lease 仍占位」的残留：
    /// acquire 被残留 lease 顶满（同账号 / 同租户 cap）时先幂等释放残留 lease
    /// 再重试恰好一次；重试仍失败时行保持 Released（一致且 fail-closed），
    /// 绝不产生 Bound 引用已释放 lease。
    async fn rebind_after_release(
        &self,
        request: &RebindRequest,
        released: SessionBinding,
    ) -> Result<BindingAcquisition, BindingServiceError> {
        let key = &request.key;
        let guards = (released.revision, released.ownership_epoch);
        let old_lease_id = released.lease_id.clone();
        let old_account_id = released.account_id.clone();
        let mut pre_release: Option<Result<ReleaseReceipt, PoolError>> = None;
        let lease = match self
            .pool
            .acquire(Self::acquire_request(
                key,
                &request.target,
                request.principal_id.clone(),
            ))
            .await
        {
            Ok(lease) => lease,
            Err(error)
                if is_self_cap_exhaustion(&error, &old_account_id, &request.key.tenant_id) =>
            {
                // 池中残留旧 lease 占位：幂等释放一次，再重试恰好一次。
                let receipt = self
                    .pool
                    .release(old_lease_id.clone(), LeaseOutcome::Released)
                    .await;
                match receipt {
                    Ok(ok) => pre_release = Some(Ok(ok)),
                    Err(pool_error) => return Err(BindingServiceError::Pool(pool_error)),
                }
                match self
                    .pool
                    .acquire(Self::acquire_request(
                        key,
                        &request.target,
                        request.principal_id.clone(),
                    ))
                    .await
                {
                    Ok(lease) => lease,
                    // 重试失败：行保持 Released（含残留 lease），一致且 fail-closed。
                    Err(pool_error) => return Err(BindingServiceError::Pool(pool_error)),
                }
            }
            Err(pool_error) => return Err(BindingServiceError::Pool(pool_error)),
        };
        let effective = effective_target(&request.target, &lease);
        let (rebound, event) = released.rebind_after_release(
            guards.0,
            guards.1,
            effective,
            request.fingerprint,
            lease.lease_id.clone(),
            request.now_ms,
            request.ttl_ms,
        )?;
        Self::check_event_keys(key, std::slice::from_ref(&event))?;
        match self
            .projection
            .compare_and_apply(key, guards.0, guards.1, &rebound, &[event])
            .await
        {
            Ok(()) => {
                let old_lease_release = match pre_release {
                    Some(receipt) => Some(receipt),
                    None => Some(
                        self.pool
                            .release(old_lease_id, LeaseOutcome::Released)
                            .await,
                    ),
                };
                Ok(BindingAcquisition {
                    binding: rebound,
                    old_lease_release,
                })
            }
            Err(conflict) => {
                let new_lease_release = self
                    .pool
                    .release(lease.lease_id.clone(), LeaseOutcome::Cancelled)
                    .await;
                Err(BindingServiceError::CommitConflict {
                    key: key.clone(),
                    conflict: Box::new(conflict),
                    new_lease_release: new_lease_release.map_err(Box::new),
                })
            }
        }
    }

    /// canonical release（细分步骤 4）：CAS 事件化转 `Released`，再幂等释放
    /// lease。pool 释放失败时行已 Released，重试本方法即可收敛（释放幂等）。
    pub async fn release_binding(
        &self,
        key: &BindingKey,
        now_ms: u64,
    ) -> Result<ReleaseReceipt, BindingServiceError> {
        let snapshot = self.projection.load(key).await?;
        let Some(current) = snapshot else {
            return Err(BindingServiceError::Projection(
                BindingProjectionError::NotFound { key: key.clone() },
            ));
        };
        Self::check_snapshot_key(key, &current)?;
        if current.state != BindingState::Released {
            let guards = (current.revision, current.ownership_epoch);
            let (released, event) = current.clone().release(guards.0, guards.1, now_ms)?;
            Self::check_event_keys(key, std::slice::from_ref(&event))?;
            self.projection
                .compare_and_apply(key, guards.0, guards.1, &released, &[event])
                .await?;
        }
        self.pool
            .release(current.lease_id.clone(), LeaseOutcome::Released)
            .await
            .map_err(BindingServiceError::Pool)
    }

    /// acquire 失败后的回滚：CAS 把 `Rebinding` abort 回 `Bound`。
    async fn abort_and_fail(
        &self,
        key: &BindingKey,
        rebinding: SessionBinding,
        pool_error: PoolError,
        now_ms: u64,
    ) -> BindingServiceError {
        let guards = (rebinding.revision, rebinding.ownership_epoch);
        let (aborted, event) = match rebinding.abort_rebind(guards.0, guards.1, now_ms) {
            Ok(pair) => pair,
            Err(transition) => return BindingServiceError::Transition(transition),
        };
        if let Err(key_error) = Self::check_event_keys(key, std::slice::from_ref(&event)) {
            return key_error;
        }
        match self
            .projection
            .compare_and_apply(key, guards.0, guards.1, &aborted, &[event])
            .await
        {
            Ok(()) => BindingServiceError::Pool(pool_error),
            Err(abort) => BindingServiceError::AcquireFailedAbortFailed {
                key: key.clone(),
                pool: Box::new(pool_error),
                abort: Box::new(abort),
            },
        }
    }

    /// pre-release 后的 fail-closed 收敛：CAS 把 `Rebinding` 转 `Released`
    /// （canonical release，事件化），**绝不 abort 回 Bound**——此时旧 lease
    /// 已释放（或释放结果不可判定），abort 会产生「Bound 引用已释放 lease」。
    /// Released CAS 失败时残留 Rebinding 孤儿，由 `recover_outstanding` 按池
    /// lease 状态继续收敛（同样不会回到 Bound）。
    async fn release_and_fail(
        &self,
        key: &BindingKey,
        rebinding: SessionBinding,
        pool_error: PoolError,
        now_ms: u64,
    ) -> BindingServiceError {
        let guards = (rebinding.revision, rebinding.ownership_epoch);
        let (released, event) = match rebinding.release(guards.0, guards.1, now_ms) {
            Ok(pair) => pair,
            Err(transition) => return BindingServiceError::Transition(transition),
        };
        if let Err(key_error) = Self::check_event_keys(key, std::slice::from_ref(&event)) {
            return key_error;
        }
        let released_cas = self
            .projection
            .compare_and_apply(key, guards.0, guards.1, &released, &[event])
            .await;
        BindingServiceError::PreReleaseRetryFailed {
            key: key.clone(),
            pool: Box::new(pool_error),
            released: released_cas.map_err(Box::new),
        }
    }

    fn acquire_request(
        key: &BindingKey,
        target: &BindingTarget,
        principal_id: PrincipalId,
    ) -> AcquireRequest {
        AcquireRequest {
            tenant_id: key.tenant_id.clone(),
            principal_id,
            session_id: key.session_id.clone(),
            agent_id: key.agent_id.clone(),
            provider_id: Some(target.provider_id.clone()),
            account_id: Some(target.account_id.clone()),
            trace_id: None,
        }
    }

    /// 防御：投影返回的 snapshot 必须与请求键一致。
    fn check_snapshot_key(
        requested: &BindingKey,
        snapshot: &SessionBinding,
    ) -> Result<(), BindingServiceError> {
        let actual = snapshot.key();
        if &actual == requested {
            Ok(())
        } else {
            Err(BindingServiceError::KeyMismatch {
                requested: Box::new(requested.clone()),
                actual: Box::new(actual),
            })
        }
    }

    /// 防御：CAS 附带的事件必须全部属于该键。
    fn check_event_keys(
        key: &BindingKey,
        events: &[BindingEvent],
    ) -> Result<(), BindingServiceError> {
        for event in events {
            let event_key = event.key();
            if &event_key != key {
                return Err(BindingServiceError::EventKeyMismatch {
                    key: Box::new(key.clone()),
                    event: Box::new(event_key),
                });
            }
        }
        Ok(())
    }
}

/// 以 lease 事实构造绑定目标：provider / account 以真实 lease 为准（池可能对
/// 缺省请求做默认化），credential 以 Route 请求为准——`AcquireRequest` 不携带
/// credential，池 picker 只按 `(tenant, account, provider)` 选择，lease 上的
/// credential 可能是合成回退（如 legacy `default`），不得覆盖路由裁决。
fn effective_target(requested: &BindingTarget, lease: &CredentialLease) -> BindingTarget {
    BindingTarget {
        provider_id: lease.provider_id.clone(),
        model_id: requested.model_id.clone(),
        account_id: lease.account_id.clone(),
        credential_id: requested.credential_id.clone(),
    }
}

/// 目标一致判定（provider / model / account / credential 全部相同才续绑）。
fn same_target(current: &SessionBinding, requested: &BindingTarget) -> bool {
    current.provider_id == requested.provider_id
        && current.model_id == requested.model_id
        && current.account_id == requested.account_id
        && current.credential_id == requested.credential_id
}

/// cap 耗尽是否由本 binding 自持的旧 lease 占位造成：同账号 cap 满（
/// `ConcurrencyExhausted.account == 旧账号`）或同租户 cap 满
/// （`TenantConcurrencyExhausted.tenant == 本键租户`）。只有这两种情况释放
/// 自持旧 lease 才能腾出额度，才允许 pre-release + 恰好一次重试。
fn is_self_cap_exhaustion(error: &PoolError, account: &AccountId, tenant: &TenantId) -> bool {
    match error {
        PoolError::ConcurrencyExhausted {
            account: exhausted, ..
        } => exhausted == account,
        PoolError::TenantConcurrencyExhausted {
            tenant: exhausted, ..
        } => exhausted == tenant,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use agent_domain::{AccountId, CredentialId, ModelId, ProviderId};

    use crate::{AccountHealth, InMemoryCredentialPool, LeaseGuard};

    use super::*;

    fn key(tenant: &str) -> BindingKey {
        BindingKey::new(
            TenantId::new(tenant),
            SessionId::new("session-1"),
            AgentId::new("agent-1"),
        )
    }

    fn target(account: &str, credential: &str) -> BindingTarget {
        BindingTarget {
            provider_id: ProviderId::new("prov-1"),
            model_id: ModelId::new("model-1"),
            account_id: AccountId::new(account),
            credential_id: CredentialId::new(credential),
        }
    }

    fn fingerprint(capability: u64, policy: u64) -> AffinityFingerprint {
        AffinityFingerprint {
            capability_hash: capability,
            policy_hash: policy,
        }
    }

    #[test]
    fn state_db_str_round_trips() {
        for state in [
            BindingState::Unbound,
            BindingState::Bound,
            BindingState::Rebinding,
            BindingState::Released,
        ] {
            assert_eq!(BindingState::from_db_str(state.as_db_str()), Some(state));
        }
        assert!(BindingState::from_db_str("bogus").is_none());
        assert!(BindingState::Released.is_terminal());
        assert!(!BindingState::Bound.is_terminal());
    }

    #[test]
    fn bind_creates_bound_revision_one_with_bound_event() {
        let (binding, event) = SessionBinding::bind(
            key("tenant-a"),
            target("acct-1", "cred-1"),
            fingerprint(7, 11),
            LeaseId::new("lease-1"),
            0,
            1_000,
            60_000,
        );
        assert_eq!(binding.state, BindingState::Bound);
        assert_eq!(binding.revision, 1);
        assert_eq!(binding.ownership_epoch, 0);
        assert_eq!(binding.key(), key("tenant-a"));
        assert_eq!(binding.target(), target("acct-1", "cred-1"));
        assert_eq!(binding.fingerprint(), fingerprint(7, 11));
        assert_eq!(binding.lease_id, LeaseId::new("lease-1"));
        assert_eq!(binding.bound_at_ms, 1_000);
        assert_eq!(binding.expires_at_ms, 61_000);
        assert_eq!(event.version(), 1);
        assert_eq!(event.key(), key("tenant-a"));
        assert!(matches!(event, BindingEvent::Bound { .. }));
    }

    #[test]
    fn resolve_reuses_healthy_unchanged_binding() {
        let (binding, _) = SessionBinding::bind(
            key("tenant-a"),
            target("acct-1", "cred-1"),
            fingerprint(7, 11),
            LeaseId::new("lease-1"),
            0,
            1_000,
            60_000,
        );
        // 指纹一致且未到期：确定性复用（P18-7 验收：healthy affinity 稳定）。
        assert_eq!(
            binding.resolve(&fingerprint(7, 11), 30_000),
            AffinityDecision::Reuse
        );
        assert!(binding.is_current(&fingerprint(7, 11), 30_000));
    }

    #[test]
    fn resolve_rebinds_on_capability_policy_and_ttl_change() {
        let (binding, _) = SessionBinding::bind(
            key("tenant-a"),
            target("acct-1", "cred-1"),
            fingerprint(7, 11),
            LeaseId::new("lease-1"),
            0,
            1_000,
            60_000,
        );
        // capability / policy hash 改变使旧 binding 失效（P18-7 验收）。
        assert_eq!(
            binding.resolve(&fingerprint(8, 11), 2_000),
            AffinityDecision::Rebind(RebindReason::CapabilityChanged)
        );
        assert_eq!(
            binding.resolve(&fingerprint(7, 12), 2_000),
            AffinityDecision::Rebind(RebindReason::PolicyChanged)
        );
        // TTL 到期（expires_at = 61_000）。
        assert_eq!(
            binding.resolve(&fingerprint(7, 11), 61_000),
            AffinityDecision::Rebind(RebindReason::TtlExpired)
        );
        assert!(!binding.is_current(&fingerprint(7, 11), 61_000));
    }

    #[test]
    fn rebind_flow_bumps_revision_and_epoch_and_swaps_lease() {
        let (binding, _) = SessionBinding::bind(
            key("tenant-a"),
            target("acct-1", "cred-1"),
            fingerprint(7, 11),
            LeaseId::new("lease-1"),
            0,
            1_000,
            60_000,
        );
        // 第一步：CAS 推进 revision（rev 1 → 2，epoch 不变）。
        let (rebinding, started) = binding
            .begin_rebind(1, 0, RebindReason::TtlExpired, 62_000)
            .expect("Bound -> Rebinding");
        assert_eq!(rebinding.state, BindingState::Rebinding);
        assert_eq!(rebinding.revision, 2);
        assert_eq!(rebinding.ownership_epoch, 0);
        assert!(matches!(
            started,
            BindingEvent::RebindingStarted {
                version: 2,
                reason: RebindReason::TtlExpired,
                ..
            }
        ));
        // 第二步：提交新目标（rev 2 → 3，epoch 0 → 1），新 lease 只出现一次。
        let (committed, rebound) = rebinding
            .commit_rebind(
                2,
                0,
                target("acct-2", "cred-2"),
                fingerprint(7, 11),
                LeaseId::new("lease-2"),
                63_000,
                60_000,
            )
            .expect("Rebinding -> Bound");
        assert_eq!(committed.state, BindingState::Bound);
        assert_eq!(committed.revision, 3);
        assert_eq!(committed.ownership_epoch, 1);
        assert_eq!(committed.target(), target("acct-2", "cred-2"));
        assert_eq!(committed.lease_id, LeaseId::new("lease-2"));
        assert_eq!(committed.bound_at_ms, 63_000);
        assert_eq!(committed.expires_at_ms, 123_000);
        assert_eq!(rebound.version(), 3);
        assert!(matches!(rebound, BindingEvent::Bound { version: 3, .. }));
        // 旧 owner 的守卫全部失效。
        assert!(matches!(
            committed.release(1, 0, 64_000),
            Err(BindingTransitionError::GuardMismatch { .. })
        ));
    }

    #[test]
    fn renew_binding_preserves_lease_and_bumps_revision_epoch_with_new_ttl() {
        let (binding, _) = SessionBinding::bind(
            key("tenant-a"),
            target("acct-1", "cred-1"),
            fingerprint(7, 11),
            LeaseId::new("lease-1"),
            0,
            1_000,
            60_000,
        );
        let (renewed, event) = binding
            .clone()
            .renew_binding(1, 0, fingerprint(9, 11), 40_000, 90_000)
            .expect("renew");
        assert_eq!(renewed.state, BindingState::Bound);
        assert_eq!(renewed.revision, 2);
        assert_eq!(renewed.ownership_epoch, 1);
        assert_eq!(renewed.lease_id, LeaseId::new("lease-1"));
        assert_eq!(renewed.fingerprint(), fingerprint(9, 11));
        assert_eq!(renewed.bound_at_ms, 40_000);
        assert_eq!(renewed.ttl_ms, 90_000);
        assert_eq!(renewed.expires_at_ms, 130_000);
        assert!(matches!(
            event,
            BindingEvent::Bound {
                version: 2,
                ownership_epoch: 1,
                lease_id: _,
                ttl_ms: 90_000,
                ..
            }
        ));
        // 续绑只允许从 Bound 出发。
        let (released, _) = renewed.release(2, 1, 41_000).expect("release");
        assert!(matches!(
            released.renew_binding(3, 1, fingerprint(9, 11), 42_000, 90_000),
            Err(BindingTransitionError::InvalidRebind { .. })
        ));
    }

    #[test]
    fn abort_rebind_restores_bound_with_original_content() {
        let (binding, _) = SessionBinding::bind(
            key("tenant-a"),
            target("acct-1", "cred-1"),
            fingerprint(7, 11),
            LeaseId::new("lease-1"),
            0,
            1_000,
            60_000,
        );
        let (rebinding, _) = binding
            .begin_rebind(1, 0, RebindReason::TtlExpired, 62_000)
            .expect("begin");
        let (restored, aborted) = rebinding
            .abort_rebind(2, 0, 63_000)
            .expect("Rebinding -> Bound");
        assert_eq!(restored.state, BindingState::Bound);
        assert_eq!(restored.revision, 3);
        assert_eq!(restored.ownership_epoch, 0);
        assert_eq!(restored.target(), target("acct-1", "cred-1"));
        assert_eq!(restored.lease_id, LeaseId::new("lease-1"));
        assert!(matches!(
            aborted,
            BindingEvent::RebindingAborted { version: 3, .. }
        ));
    }

    #[test]
    fn invalid_transitions_and_guard_mismatch_fail_closed() {
        let (binding, _) = SessionBinding::bind(
            key("tenant-a"),
            target("acct-1", "cred-1"),
            fingerprint(7, 11),
            LeaseId::new("lease-1"),
            0,
            1_000,
            60_000,
        );
        // 状态不符：Bound 上不能 commit / abort。
        assert!(matches!(
            binding.clone().commit_rebind(
                1,
                0,
                target("acct-2", "cred-2"),
                fingerprint(7, 11),
                LeaseId::new("lease-2"),
                2_000,
                60_000,
            ),
            Err(BindingTransitionError::InvalidCommit { .. })
        ));
        assert!(matches!(
            binding.clone().abort_rebind(1, 0, 2_000),
            Err(BindingTransitionError::InvalidAbort { .. })
        ));
        // 守卫不符：过期 revision / epoch 都不放行。
        assert!(matches!(
            binding
                .clone()
                .begin_rebind(9, 0, RebindReason::TtlExpired, 2_000),
            Err(BindingTransitionError::GuardMismatch {
                expected_revision: 9,
                ..
            })
        ));
        assert!(matches!(
            binding
                .clone()
                .begin_rebind(1, 5, RebindReason::TtlExpired, 2_000),
            Err(BindingTransitionError::GuardMismatch {
                expected_epoch: 5,
                ..
            })
        ));
        // Released 上不可再 release / rebind。
        let (released, _) = binding.release(1, 0, 2_000).expect("Bound -> Released");
        assert!(released.state.is_terminal());
        assert!(matches!(
            released.clone().release(2, 0, 3_000),
            Err(BindingTransitionError::InvalidRelease { .. })
        ));
        assert!(matches!(
            released.resolve(&fingerprint(7, 11), 3_000),
            AffinityDecision::Rebind(RebindReason::Released)
        ));
        // Released → Bound 延续 generation：revision / epoch 各 +1，事件版本连续。
        let (rebound, rebound_event) = released
            .clone()
            .rebind_after_release(
                2,
                0,
                target("acct-2", "cred-2"),
                fingerprint(8, 12),
                LeaseId::new("lease-2"),
                4_000,
                90_000,
            )
            .expect("Released -> Bound");
        assert_eq!(rebound.state, BindingState::Bound);
        assert_eq!(rebound.revision, 3);
        assert_eq!(rebound.ownership_epoch, 1);
        assert_eq!(rebound.lease_id, LeaseId::new("lease-2"));
        assert_eq!(rebound.ttl_ms, 90_000);
        assert_eq!(rebound.expires_at_ms, 94_000);
        assert!(matches!(
            rebound_event,
            BindingEvent::Bound {
                version: 3,
                ownership_epoch: 1,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn concurrent_rebind_acquires_exactly_one_lease() {
        let projection = Arc::new(InMemoryBindingProjection::new());
        let (binding, bound_event) = SessionBinding::bind(
            key("tenant-a"),
            target("acct-1", "cred-1"),
            fingerprint(7, 11),
            LeaseId::new("lease-1"),
            0,
            1_000,
            60_000,
        );
        projection
            .insert(&binding, std::slice::from_ref(&bound_event))
            .await
            .expect("initial bind");

        // 两个并发 rebind 都基于同一快照（rev=1, epoch=0）：恰好一个 CAS 成功，
        // 另一个得到 Conflict 且不获取 lease（P18-7 验收：只发生一次安全 rebind）。
        let read = projection
            .load(&key("tenant-a"))
            .await
            .expect("load")
            .expect("bound");
        assert_eq!((read.revision, read.ownership_epoch), (1, 0));
        let mut tasks = Vec::new();
        for _ in 0..2 {
            let projection = projection.clone();
            let read = read.clone();
            tasks.push(tokio::spawn(async move {
                let revision = read.revision;
                let epoch = read.ownership_epoch;
                let key = read.key();
                let (rebinding, event) = read
                    .begin_rebind(revision, epoch, RebindReason::TtlExpired, 62_000)
                    .expect("domain transition");
                projection
                    .compare_and_apply(
                        &key,
                        revision,
                        epoch,
                        &rebinding,
                        std::slice::from_ref(&event),
                    )
                    .await
            }));
        }
        let mut results = Vec::new();
        for task in tasks {
            results.push(task.await.expect("task"));
        }
        assert_eq!(
            results.iter().filter(|result| result.is_ok()).count(),
            1,
            "恰好一个 CAS 成功（单次安全 rebind）"
        );
        assert_eq!(
            results
                .iter()
                .filter(|result| matches!(result, Err(BindingProjectionError::Conflict { .. })))
                .count(),
            1,
            "另一个并发 rebind 必须得到 CAS Conflict"
        );

        // 胜者 commit：只有一次新的 lease 引用。
        let rebinding = projection
            .load(&key("tenant-a"))
            .await
            .expect("load")
            .expect("bound");
        assert_eq!(rebinding.state, BindingState::Rebinding);
        let rebinding_revision = rebinding.revision;
        let rebinding_epoch = rebinding.ownership_epoch;
        let (committed, rebound_event) = rebinding
            .commit_rebind(
                rebinding_revision,
                rebinding_epoch,
                target("acct-2", "cred-2"),
                fingerprint(7, 11),
                LeaseId::new("lease-2"),
                63_000,
                60_000,
            )
            .expect("commit");
        projection
            .compare_and_apply(
                &committed.key(),
                rebinding_revision,
                rebinding_epoch,
                &committed,
                std::slice::from_ref(&rebound_event),
            )
            .await
            .expect("commit CAS");

        let final_binding = projection
            .load(&key("tenant-a"))
            .await
            .expect("load")
            .expect("bound");
        assert_eq!(final_binding.lease_id, LeaseId::new("lease-2"));
        assert_eq!(final_binding.revision, 3);
        assert_eq!(final_binding.ownership_epoch, 1);
        let events = projection.events();
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, BindingEvent::RebindingStarted { .. }))
                .count(),
            1
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, BindingEvent::Bound { .. }))
                .count(),
            2,
            "初始 bind + 一次 rebind commit"
        );
        // 败者用旧守卫再次 CAS 依然失败：不重复占用 lease。
        let stale = binding; // 初始快照（rev=1, epoch=0）。
        let conflict = projection
            .compare_and_apply(
                &stale.key(),
                1,
                0,
                &stale,
                std::slice::from_ref(&bound_event),
            )
            .await;
        assert!(matches!(
            conflict,
            Err(BindingProjectionError::Conflict { .. })
        ));
    }

    #[test]
    fn replay_rebuilds_final_state_from_event_log() {
        let (binding, bound) = SessionBinding::bind(
            key("tenant-a"),
            target("acct-1", "cred-1"),
            fingerprint(7, 11),
            LeaseId::new("lease-1"),
            0,
            1_000,
            60_000,
        );
        let (rebinding, started) = binding
            .begin_rebind(1, 0, RebindReason::PolicyChanged, 2_000)
            .expect("begin");
        let (committed, rebound) = rebinding
            .commit_rebind(
                2,
                0,
                target("acct-2", "cred-2"),
                fingerprint(7, 12),
                LeaseId::new("lease-2"),
                3_000,
                60_000,
            )
            .expect("commit");

        let mut state = None;
        for event in [&bound, &started, &rebound] {
            state = apply_event(state, event).expect("replay");
        }
        assert_eq!(state, Some(committed), "事件重放必须重建最终快照");

        // 事件缺失（跳过 RebindingStarted）fail-closed。
        let result = apply_event(None, &bound).and_then(|s| apply_event(s, &rebound));
        assert!(matches!(result, Err(BindingTransitionError::Replay(_))));
    }

    #[tokio::test]
    async fn cross_tenant_keys_never_share_affinity() {
        // Tenant A 与 Tenant B 同名 session/agent 是不同键（P18-7 验收）。
        let key_a = key("tenant-a");
        let key_b = key("tenant-b");
        assert_ne!(key_a, key_b);

        let projection = InMemoryBindingProjection::new();
        let (binding, event) = SessionBinding::bind(
            key_a.clone(),
            target("acct-1", "cred-1"),
            fingerprint(7, 11),
            LeaseId::new("lease-1"),
            0,
            1_000,
            60_000,
        );
        projection
            .insert(&binding, std::slice::from_ref(&event))
            .await
            .expect("insert");
        assert_eq!(
            projection.load(&key_a).await.expect("load"),
            Some(binding.clone())
        );
        assert_eq!(
            projection.load(&key_b).await.expect("load"),
            None,
            "Tenant B 不得命中 Tenant A 的 binding"
        );
    }

    #[test]
    fn serialization_contains_no_secret_fields() {
        let (binding, _) = SessionBinding::bind(
            key("tenant-a"),
            target("acct-1", "cred-1"),
            fingerprint(7, 11),
            LeaseId::new("lease-1"),
            0,
            1_000,
            60_000,
        );
        let value = serde_json::to_value(&binding).expect("serialize binding");
        let mut stack = vec![value];
        while let Some(node) = stack.pop() {
            match node {
                serde_json::Value::Object(map) => {
                    for (key, child) in map {
                        assert!(
                            !matches!(key.as_str(), "secret" | "token" | "api_key" | "password"),
                            "binding must not carry secret fields, found `{key}`"
                        );
                        stack.push(child);
                    }
                }
                serde_json::Value::Array(items) => stack.extend(items),
                _ => {}
            }
        }
        // 事件 serde 往返稳定（tagged）。
        let wire = serde_json::to_string(&binding.bound_event()).expect("serialize event");
        let decoded: BindingEvent = serde_json::from_str(&wire).expect("decode event");
        assert_eq!(decoded, binding.bound_event());
    }

    #[test]
    fn fingerprint_hash_is_deterministic() {
        assert_eq!(fingerprint_hash(b"abc"), fingerprint_hash(b"abc"));
        assert_ne!(fingerprint_hash(b"abc"), fingerprint_hash(b"abd"));
    }

    #[test]
    #[cfg(feature = "account-control-v1")]
    fn capability_and_policy_fingerprints_are_deterministic_and_sensitive() {
        use std::collections::BTreeSet;
        let base: BTreeSet<crate::routing::Capability> =
            [crate::routing::Capability::Text].into_iter().collect();
        let extended: BTreeSet<crate::routing::Capability> = [
            crate::routing::Capability::Text,
            crate::routing::Capability::ToolCalls,
        ]
        .into_iter()
        .collect();
        assert_eq!(capability_fingerprint(&base), capability_fingerprint(&base));
        assert_ne!(
            capability_fingerprint(&base),
            capability_fingerprint(&extended)
        );
        let policy = crate::routing::RoutingPolicy::default();
        assert_eq!(policy_fingerprint(&policy), policy_fingerprint(&policy));
    }

    // ── P18-7 主审补充：真实 pool 的单飞协调器验收 ─────────────────────────

    /// 计数包装：把真实 [`InMemoryCredentialPool`] 的 acquire / release 调用与
    /// 真实 lease id 记录下来，证明「只发生一次 acquire / release」。
    #[derive(Clone)]
    struct CountingPool {
        inner: Arc<InMemoryCredentialPool>,
        acquires: Arc<AtomicUsize>,
        acquired: Arc<Mutex<Vec<LeaseId>>>,
        releases: Arc<Mutex<Vec<(LeaseId, LeaseOutcome)>>>,
        fail_acquire: Arc<AtomicBool>,
        /// 第 N 次 acquire（从 1 计数）注入失败；0 = 不注入。
        fail_on_acquire_call: Arc<AtomicUsize>,
        /// 下一次 release 注入失败（不触碰内层池，模拟「行已 Released、
        /// lease 仍占位」的残留）。
        fail_release: Arc<AtomicBool>,
    }

    impl CountingPool {
        fn new(inner: InMemoryCredentialPool) -> Self {
            Self {
                inner: Arc::new(inner),
                acquires: Arc::new(AtomicUsize::new(0)),
                acquired: Arc::new(Mutex::new(Vec::new())),
                releases: Arc::new(Mutex::new(Vec::new())),
                fail_acquire: Arc::new(AtomicBool::new(false)),
                fail_on_acquire_call: Arc::new(AtomicUsize::new(0)),
                fail_release: Arc::new(AtomicBool::new(false)),
            }
        }

        fn set_fail_acquire(&self, fail: bool) {
            self.fail_acquire.store(fail, Ordering::SeqCst);
        }

        fn set_fail_on_acquire_call(&self, call: usize) {
            self.fail_on_acquire_call.store(call, Ordering::SeqCst);
        }

        fn set_fail_release(&self, fail: bool) {
            self.fail_release.store(fail, Ordering::SeqCst);
        }

        fn acquire_count(&self) -> usize {
            self.acquires.load(Ordering::SeqCst)
        }

        fn acquired_ids(&self) -> Vec<LeaseId> {
            self.acquired
                .lock()
                .expect("counting pool poisoned")
                .clone()
        }

        fn released(&self) -> Vec<(LeaseId, LeaseOutcome)> {
            self.releases
                .lock()
                .expect("counting pool poisoned")
                .clone()
        }
    }

    #[async_trait]
    impl CredentialPool for CountingPool {
        async fn acquire(&self, req: AcquireRequest) -> Result<CredentialLease, PoolError> {
            let call = self.acquires.fetch_add(1, Ordering::SeqCst) + 1;
            if self.fail_acquire.load(Ordering::SeqCst) {
                return Err(PoolError::NoCandidate);
            }
            let fail_on_call = self.fail_on_acquire_call.load(Ordering::SeqCst);
            if fail_on_call != 0 && call == fail_on_call {
                return Err(PoolError::NoCandidate);
            }
            let lease = self.inner.acquire(req).await?;
            self.acquired
                .lock()
                .expect("counting pool poisoned")
                .push(lease.lease_id.clone());
            Ok(lease)
        }

        async fn acquire_guard(&self, req: AcquireRequest) -> Result<LeaseGuard, PoolError> {
            self.inner.acquire_guard(req).await
        }

        async fn release(
            &self,
            lease_id: LeaseId,
            outcome: LeaseOutcome,
        ) -> Result<ReleaseReceipt, PoolError> {
            self.releases
                .lock()
                .expect("counting pool poisoned")
                .push((lease_id.clone(), outcome));
            if self.fail_release.load(Ordering::SeqCst) {
                return Err(PoolError::NoCandidate);
            }
            self.inner.release(lease_id, outcome).await
        }

        fn active_count(&self, account: &AccountId) -> u64 {
            self.inner.active_count(account)
        }

        fn account_health(&self, account: &AccountId) -> AccountHealth {
            self.inner.account_health(account)
        }

        fn lease_state(&self, lease_id: &LeaseId) -> Option<LeaseState> {
            self.inner.lease_state(lease_id)
        }
    }

    /// 在第 N 次 `compare_and_apply` 注入 `Conflict`（N=0 不注入）。
    #[derive(Clone)]
    struct FaultProjection {
        inner: Arc<InMemoryBindingProjection>,
        compare_and_apply_calls: Arc<AtomicUsize>,
        fail_on_call: Arc<AtomicUsize>,
        fail_load_outstanding: Arc<AtomicBool>,
    }

    impl FaultProjection {
        fn new(inner: Arc<InMemoryBindingProjection>) -> Self {
            Self {
                inner,
                compare_and_apply_calls: Arc::new(AtomicUsize::new(0)),
                fail_on_call: Arc::new(AtomicUsize::new(0)),
                fail_load_outstanding: Arc::new(AtomicBool::new(false)),
            }
        }

        fn set_fail_on_call(&self, call: usize) {
            self.fail_on_call.store(call, Ordering::SeqCst);
        }

        fn set_fail_load_outstanding(&self, fail: bool) {
            self.fail_load_outstanding.store(fail, Ordering::SeqCst);
        }
    }

    #[async_trait]
    impl BindingProjection for FaultProjection {
        async fn insert(
            &self,
            snapshot: &SessionBinding,
            events: &[BindingEvent],
        ) -> Result<(), BindingProjectionError> {
            self.inner.insert(snapshot, events).await
        }

        async fn compare_and_apply(
            &self,
            key: &BindingKey,
            expected_revision: u64,
            expected_epoch: u64,
            snapshot: &SessionBinding,
            events: &[BindingEvent],
        ) -> Result<(), BindingProjectionError> {
            let call = self.compare_and_apply_calls.fetch_add(1, Ordering::SeqCst) + 1;
            if call == self.fail_on_call.load(Ordering::SeqCst) {
                let actual = self.inner.load(key).await?;
                return match actual {
                    Some(current) => Err(BindingProjectionError::Conflict {
                        key: key.clone(),
                        expected_revision,
                        expected_epoch,
                        actual_revision: current.revision,
                        actual_epoch: current.ownership_epoch,
                    }),
                    None => Err(BindingProjectionError::NotFound { key: key.clone() }),
                };
            }
            self.inner
                .compare_and_apply(key, expected_revision, expected_epoch, snapshot, events)
                .await
        }

        async fn apply(
            &self,
            snapshot: &SessionBinding,
            events: &[BindingEvent],
        ) -> Result<(), BindingProjectionError> {
            self.inner.apply(snapshot, events).await
        }

        async fn load(
            &self,
            key: &BindingKey,
        ) -> Result<Option<SessionBinding>, BindingProjectionError> {
            self.inner.load(key).await
        }

        async fn load_outstanding(&self) -> Result<Vec<SessionBinding>, BindingProjectionError> {
            if self.fail_load_outstanding.load(Ordering::SeqCst) {
                return Err(BindingProjectionError::Backend(
                    "injected load_outstanding failure".to_string(),
                ));
            }
            self.inner.load_outstanding().await
        }

        async fn settle(&self, key: &BindingKey) -> Result<(), BindingProjectionError> {
            self.inner.settle(key).await
        }

        async fn continuation(
            &self,
            key: &BindingKey,
        ) -> Result<Option<(u64, u64)>, BindingProjectionError> {
            self.inner.continuation(key).await
        }
    }

    /// 无论请求哪个键，`load` 恒返回另一键的快照（键不一致的故障后端）。
    #[derive(Clone)]
    struct WrongKeyProjection {
        inner: Arc<InMemoryBindingProjection>,
        wrong: Arc<SessionBinding>,
    }

    #[async_trait]
    impl BindingProjection for WrongKeyProjection {
        async fn insert(
            &self,
            snapshot: &SessionBinding,
            events: &[BindingEvent],
        ) -> Result<(), BindingProjectionError> {
            self.inner.insert(snapshot, events).await
        }

        async fn compare_and_apply(
            &self,
            key: &BindingKey,
            expected_revision: u64,
            expected_epoch: u64,
            snapshot: &SessionBinding,
            events: &[BindingEvent],
        ) -> Result<(), BindingProjectionError> {
            self.inner
                .compare_and_apply(key, expected_revision, expected_epoch, snapshot, events)
                .await
        }

        async fn apply(
            &self,
            snapshot: &SessionBinding,
            events: &[BindingEvent],
        ) -> Result<(), BindingProjectionError> {
            self.inner.apply(snapshot, events).await
        }

        async fn load(
            &self,
            _key: &BindingKey,
        ) -> Result<Option<SessionBinding>, BindingProjectionError> {
            Ok(Some((*self.wrong).clone()))
        }

        async fn load_outstanding(&self) -> Result<Vec<SessionBinding>, BindingProjectionError> {
            self.inner.load_outstanding().await
        }

        async fn settle(&self, key: &BindingKey) -> Result<(), BindingProjectionError> {
            self.inner.settle(key).await
        }

        async fn continuation(
            &self,
            key: &BindingKey,
        ) -> Result<Option<(u64, u64)>, BindingProjectionError> {
            self.inner.continuation(key).await
        }
    }

    /// 并发双绑测试用门控投影：两个并发 `load` 都完成（各自读到 `None`）后
    /// 屏障才放行，保证双方都带着 `None` 快照竞争 `insert`（确定性复现双绑
    /// 竞态；其余方法原样委托）。
    struct DoubleBindBarrierProjection {
        inner: Arc<InMemoryBindingProjection>,
        gate: Arc<tokio::sync::Barrier>,
    }

    impl DoubleBindBarrierProjection {
        fn new(inner: Arc<InMemoryBindingProjection>, participants: usize) -> Self {
            Self {
                inner,
                gate: Arc::new(tokio::sync::Barrier::new(participants)),
            }
        }
    }

    #[async_trait]
    impl BindingProjection for DoubleBindBarrierProjection {
        async fn insert(
            &self,
            snapshot: &SessionBinding,
            events: &[BindingEvent],
        ) -> Result<(), BindingProjectionError> {
            self.inner.insert(snapshot, events).await
        }

        async fn compare_and_apply(
            &self,
            key: &BindingKey,
            expected_revision: u64,
            expected_epoch: u64,
            snapshot: &SessionBinding,
            events: &[BindingEvent],
        ) -> Result<(), BindingProjectionError> {
            self.inner
                .compare_and_apply(key, expected_revision, expected_epoch, snapshot, events)
                .await
        }

        async fn apply(
            &self,
            snapshot: &SessionBinding,
            events: &[BindingEvent],
        ) -> Result<(), BindingProjectionError> {
            self.inner.apply(snapshot, events).await
        }

        async fn load(
            &self,
            key: &BindingKey,
        ) -> Result<Option<SessionBinding>, BindingProjectionError> {
            let seen = self.inner.load(key).await;
            self.gate.wait().await;
            seen
        }

        async fn load_outstanding(&self) -> Result<Vec<SessionBinding>, BindingProjectionError> {
            self.inner.load_outstanding().await
        }

        async fn settle(&self, key: &BindingKey) -> Result<(), BindingProjectionError> {
            self.inner.settle(key).await
        }

        async fn continuation(
            &self,
            key: &BindingKey,
        ) -> Result<Option<(u64, u64)>, BindingProjectionError> {
            self.inner.continuation(key).await
        }
    }

    fn acquire_req(key: &BindingKey, target: &BindingTarget) -> AcquireRequest {
        AcquireRequest {
            tenant_id: key.tenant_id.clone(),
            principal_id: PrincipalId::new("principal-a"),
            session_id: key.session_id.clone(),
            agent_id: key.agent_id.clone(),
            provider_id: Some(target.provider_id.clone()),
            account_id: Some(target.account_id.clone()),
            trace_id: None,
        }
    }

    /// 构造服务协调请求：tenant-a / 指定目标 / 给定指纹、时钟与 TTL。
    fn rebind_request_full(
        account: &str,
        credential: &str,
        fp: AffinityFingerprint,
        now_ms: u64,
        ttl_ms: u64,
    ) -> RebindRequest {
        RebindRequest {
            key: key("tenant-a"),
            target: target(account, credential),
            fingerprint: fp,
            principal_id: PrincipalId::new("principal-a"),
            now_ms,
            ttl_ms,
        }
    }

    /// 构造服务协调请求：tenant-a / acct-2（换账号 replacement 场景）。
    fn rebind_request(fp: AffinityFingerprint, now_ms: u64) -> RebindRequest {
        rebind_request_full("acct-2", "cred-2", fp, now_ms, 60_000)
    }

    /// 同目标（acct-1 / cred-1）续绑场景请求。
    fn rebind_request_same_target(
        fp: AffinityFingerprint,
        now_ms: u64,
        ttl_ms: u64,
    ) -> RebindRequest {
        rebind_request_full("acct-1", "cred-1", fp, now_ms, ttl_ms)
    }

    /// 用真实 pool acquire 一个 lease 并落盘 Bound 快照（作为 rebind 的起点）。
    async fn seed_bound(
        pool: &CountingPool,
        projection: &InMemoryBindingProjection,
    ) -> (SessionBinding, CredentialLease) {
        let key = key("tenant-a");
        let target = target("acct-1", "cred-1");
        let lease = pool
            .acquire(acquire_req(&key, &target))
            .await
            .expect("seed acquire");
        let (binding, event) = SessionBinding::bind(
            key,
            target,
            fingerprint(7, 11),
            lease.lease_id.clone(),
            0,
            1_000,
            60_000,
        );
        projection
            .insert(&binding, std::slice::from_ref(&event))
            .await
            .expect("seed insert");
        (binding, lease)
    }

    #[tokio::test]
    async fn service_rebind_runs_real_cas_acquire_commit_and_releases_old_lease_once() {
        let pool = CountingPool::new(InMemoryCredentialPool::new(4));
        let projection = Arc::new(InMemoryBindingProjection::new());
        let (old_binding, old_lease) = seed_bound(&pool, &projection).await;
        let service = SessionBindingService::new(Arc::new(pool.clone()), projection.clone());

        let outcome = service
            .acquire_binding(rebind_request(fingerprint(8, 11), 30_000))
            .await
            .expect("rebind");

        // 真实 pool：1 次 seed + 恰好 1 次服务 acquire；旧 lease 恰好在 commit 成功后释放 1 次。
        assert_eq!(pool.acquire_count(), 2);
        assert_eq!(
            pool.released(),
            vec![(old_lease.lease_id.clone(), LeaseOutcome::Completed)]
        );
        assert_eq!(pool.active_count(&AccountId::new("acct-1")), 0);
        assert_eq!(pool.active_count(&AccountId::new("acct-2")), 1);
        assert!(matches!(&outcome.old_lease_release, Some(Ok(_))));

        // 提交后的 canonical 快照：rev 3 / epoch +1 / 新 lease，与投影完全一致。
        let binding = &outcome.binding;
        assert_eq!(binding.state, BindingState::Bound);
        assert_eq!(binding.revision, 3);
        assert_eq!(binding.ownership_epoch, old_binding.ownership_epoch + 1);
        assert_ne!(binding.lease_id, old_binding.lease_id);
        let new_lease = pool.acquired_ids().pop().expect("new lease id");
        assert_eq!(binding.lease_id, new_lease);
        assert_eq!(projection.snapshot(&key("tenant-a")), Some(binding.clone()));

        // 事件日志键 / 版本与 CAS 键一致：Bound(1) → RebindingStarted(2) → Bound(3)。
        let events = projection.events();
        assert_eq!(events.len(), 3);
        assert!(matches!(
            events[1],
            BindingEvent::RebindingStarted {
                version: 2,
                reason: RebindReason::CapabilityChanged,
                ..
            }
        ));
        assert_eq!(events[2].version(), 3);
        for event in &events {
            assert_eq!(event.key(), key("tenant-a"));
        }
    }

    #[tokio::test]
    async fn service_acquire_failure_aborts_back_to_bound_without_lease_traffic() {
        let pool = CountingPool::new(InMemoryCredentialPool::new(4));
        let projection = Arc::new(InMemoryBindingProjection::new());
        let (old_binding, _) = seed_bound(&pool, &projection).await;
        pool.set_fail_acquire(true);
        let service = SessionBindingService::new(Arc::new(pool.clone()), projection.clone());

        let error = service
            .acquire_binding(rebind_request(fingerprint(8, 11), 30_000))
            .await
            .expect_err("acquire failure");
        assert!(matches!(
            error,
            BindingServiceError::Pool(PoolError::NoCandidate)
        ));

        // CAS 已回滚：Bound，rev 3（Started + Aborted），epoch 不变，旧 lease 保留。
        let restored = projection.snapshot(&key("tenant-a")).expect("restored");
        assert_eq!(restored.state, BindingState::Bound);
        assert_eq!(restored.revision, 3);
        assert_eq!(restored.ownership_epoch, old_binding.ownership_epoch);
        assert_eq!(restored.lease_id, old_binding.lease_id);
        assert_eq!(pool.acquire_count(), 2, "1 seed + 1 failed attempt");
        assert!(pool.released().is_empty(), "未获取新 lease，也不动旧 lease");

        let events = projection.events();
        assert_eq!(events.len(), 3);
        assert!(matches!(
            events[2],
            BindingEvent::RebindingAborted { version: 3, .. }
        ));
        for event in &events {
            assert_eq!(event.key(), key("tenant-a"));
        }
    }

    #[tokio::test]
    async fn service_commit_conflict_releases_new_lease_and_keeps_single_flight() {
        let pool = CountingPool::new(InMemoryCredentialPool::new(4));
        let projection = Arc::new(InMemoryBindingProjection::new());
        seed_bound(&pool, &projection).await;
        // 第 2 次 compare_and_apply（commit CAS）注入 Conflict；第 1 次（begin CAS）放行。
        let fault = FaultProjection::new(projection.clone());
        fault.set_fail_on_call(2);
        let service = SessionBindingService::new(Arc::new(pool.clone()), Arc::new(fault));

        let error = service
            .acquire_binding(rebind_request(fingerprint(8, 11), 30_000))
            .await
            .expect_err("commit conflict");
        let conflict = match error {
            BindingServiceError::CommitConflict {
                conflict,
                new_lease_release,
                ..
            } => {
                assert!(matches!(*conflict, BindingProjectionError::Conflict { .. }));
                assert!(new_lease_release.is_ok());
                conflict
            }
            other => panic!("unexpected error: {other:?}"),
        };
        let _ = conflict;

        // 新 lease 恰好释放 1 次（Cancelled）；旧 seed lease 未动。
        assert_eq!(pool.acquire_count(), 2);
        let releases = pool.released();
        assert_eq!(releases.len(), 1);
        let new_lease_id = pool.acquired_ids().pop().expect("new lease id");
        assert_eq!(releases[0].0, new_lease_id);
        assert_eq!(releases[0].1, LeaseOutcome::Cancelled);
        assert_eq!(pool.active_count(&AccountId::new("acct-1")), 1);
        assert_eq!(pool.active_count(&AccountId::new("acct-2")), 0);

        // 投影停留在 Rebinding（冲突未覆盖）；再次调用被单飞拒绝，不产生额外 acquire。
        let snap = projection.snapshot(&key("tenant-a")).expect("row");
        assert_eq!(snap.state, BindingState::Rebinding);
        assert_eq!(snap.revision, 2);
        let again = service
            .acquire_binding(rebind_request(fingerprint(8, 11), 31_000))
            .await;
        assert!(matches!(
            again,
            Err(BindingServiceError::Transition(
                BindingTransitionError::InvalidRebind { .. }
            ))
        ));
        assert_eq!(pool.acquire_count(), 2, "单飞拒绝不得再 acquire");
    }

    #[tokio::test]
    async fn service_recover_aborts_orphan_rebinding_without_lease_traffic() {
        let pool = CountingPool::new(InMemoryCredentialPool::new(4));
        let projection = Arc::new(InMemoryBindingProjection::new());
        let (binding, _) = seed_bound(&pool, &projection).await;
        // 模拟「begin CAS 后 crash」：手动留下 Rebinding 孤儿。
        let epoch = binding.ownership_epoch;
        let (rebinding, started) = binding
            .begin_rebind(1, epoch, RebindReason::TtlExpired, 62_000)
            .expect("begin");
        projection
            .compare_and_apply(
                &key("tenant-a"),
                1,
                epoch,
                &rebinding,
                std::slice::from_ref(&started),
            )
            .await
            .expect("orphan CAS");
        let service = SessionBindingService::new(Arc::new(pool.clone()), projection.clone());

        let recovered = service.recover_outstanding(63_000).await.expect("recover");
        assert_eq!(recovered, 1);
        let restored = projection.snapshot(&key("tenant-a")).expect("restored");
        assert_eq!(restored.state, BindingState::Bound);
        assert_eq!(restored.revision, 3);
        assert_eq!(restored.ownership_epoch, epoch);
        assert_eq!(pool.acquire_count(), 1, "恢复不得 acquire");
        assert!(pool.released().is_empty(), "恢复不得 release");
    }

    #[tokio::test]
    async fn service_fails_closed_on_snapshot_key_mismatch() {
        let pool = CountingPool::new(InMemoryCredentialPool::new(4));
        let projection = Arc::new(InMemoryBindingProjection::new());
        let (wrong, _) = SessionBinding::bind(
            key("tenant-b"),
            target("acct-1", "cred-1"),
            fingerprint(7, 11),
            LeaseId::new("lease-b"),
            0,
            1_000,
            60_000,
        );
        let wrong_projection = WrongKeyProjection {
            inner: projection.clone(),
            wrong: Arc::new(wrong),
        };
        let service =
            SessionBindingService::new(Arc::new(pool.clone()), Arc::new(wrong_projection));

        let error = service
            .acquire_binding(RebindRequest {
                key: key("tenant-a"),
                target: target("acct-1", "cred-1"),
                fingerprint: fingerprint(7, 11),
                principal_id: PrincipalId::new("principal-a"),
                now_ms: 30_000,
                ttl_ms: 60_000,
            })
            .await
            .expect_err("key mismatch");
        assert!(matches!(error, BindingServiceError::KeyMismatch { .. }));
        assert_eq!(pool.acquire_count(), 0, "键校验失败不得触碰 pool");
    }

    #[test]
    fn service_key_and_event_validators_fail_closed() {
        let requested = key("tenant-a");
        let (snapshot, event) = SessionBinding::bind(
            key("tenant-b"),
            target("acct-1", "cred-1"),
            fingerprint(7, 11),
            LeaseId::new("lease-b"),
            0,
            1_000,
            60_000,
        );
        assert!(matches!(
            SessionBindingService::<InMemoryCredentialPool, InMemoryBindingProjection>::check_snapshot_key(
                &requested,
                &snapshot
            ),
            Err(BindingServiceError::KeyMismatch { .. })
        ));
        assert!(matches!(
            SessionBindingService::<InMemoryCredentialPool, InMemoryBindingProjection>::check_event_keys(
                &requested,
                &[event]
            ),
            Err(BindingServiceError::EventKeyMismatch { .. })
        ));
        assert!(
            SessionBindingService::<InMemoryCredentialPool, InMemoryBindingProjection>::check_snapshot_key(
                &snapshot.key(),
                &snapshot
            )
            .is_ok()
        );
    }

    // ── P18-7 复审：cap=1 续绑 / replacement、release 重绑连续、Reuse lease 校验 ──

    #[tokio::test]
    async fn service_same_target_rebind_renews_live_lease_under_account_cap_one() {
        let pool = CountingPool::new(InMemoryCredentialPool::new(1));
        let projection = Arc::new(InMemoryBindingProjection::new());
        let (old_binding, old_lease) = seed_bound(&pool, &projection).await;
        let service = SessionBindingService::new(Arc::new(pool.clone()), projection.clone());

        // 同账号 cap=1：同目标指纹变化必须走续绑，不得因旧 lease 占位自锁。
        let outcome = service
            .acquire_binding(rebind_request_same_target(
                fingerprint(9, 11),
                40_000,
                90_000,
            ))
            .await
            .expect("renew");
        assert_eq!(pool.acquire_count(), 1, "续绑不得重新 acquire");
        assert!(pool.released().is_empty(), "续绑不得 release 活 lease");
        assert_eq!(pool.active_count(&AccountId::new("acct-1")), 1);
        let binding = &outcome.binding;
        assert_eq!(binding.revision, 2);
        assert_eq!(binding.ownership_epoch, old_binding.ownership_epoch + 1);
        assert_eq!(binding.lease_id, old_lease.lease_id);
        assert_eq!(binding.capability_hash, 9);
        assert_eq!(binding.ttl_ms, 90_000, "续绑应用新 TTL");
        assert_eq!(binding.bound_at_ms, 40_000);
        assert_eq!(binding.expires_at_ms, 130_000);
        assert_eq!(projection.snapshot(&key("tenant-a")), Some(binding.clone()));
        let events = projection.events();
        assert_eq!(events.len(), 2);
        assert_eq!(events[1].version(), 2);
        for event in &events {
            assert_eq!(event.key(), key("tenant-a"));
        }
        // 续绑后活 lease 稳定复用，且不再触碰 pool。
        let reuse = service
            .acquire_binding(rebind_request_same_target(
                fingerprint(9, 11),
                45_000,
                90_000,
            ))
            .await
            .expect("reuse");
        assert_eq!(reuse.binding, binding.clone());
        assert_eq!(pool.acquire_count(), 1, "复用不得 acquire");
    }

    #[tokio::test]
    async fn service_same_account_replacement_releases_old_lease_once_under_cap_one() {
        let pool = CountingPool::new(InMemoryCredentialPool::new(1));
        let projection = Arc::new(InMemoryBindingProjection::new());
        let (_, old_lease) = seed_bound(&pool, &projection).await;
        let service = SessionBindingService::new(Arc::new(pool.clone()), projection.clone());

        // 同账号 acct-1、换凭据 cred-2：先释放旧 lease，再恰好 acquire 一次。
        let outcome = service
            .acquire_binding(rebind_request_full(
                "acct-1",
                "cred-2",
                fingerprint(8, 11),
                30_000,
                60_000,
            ))
            .await
            .expect("replacement");
        assert_eq!(pool.acquire_count(), 3, "seed + cap 拒绝 + 恰好一次重试");
        assert_eq!(
            pool.released(),
            vec![(old_lease.lease_id.clone(), LeaseOutcome::Completed)]
        );
        assert_eq!(pool.active_count(&AccountId::new("acct-1")), 1);
        assert_eq!(outcome.binding.credential_id, CredentialId::new("cred-2"));
        assert_eq!(
            outcome.binding.lease_id,
            *pool.acquired_ids().last().expect("new lease")
        );
        assert_eq!(
            projection.snapshot(&key("tenant-a")),
            Some(outcome.binding.clone())
        );
    }

    #[tokio::test]
    async fn service_tenant_cap_rebind_prereleases_old_lease_and_retries_once() {
        let pool = CountingPool::new(InMemoryCredentialPool::with_config(
            crate::PoolConfig::new(4).with_tenant_cap(1),
        ));
        let projection = Arc::new(InMemoryBindingProjection::new());
        let (old_binding, old_lease) = seed_bound(&pool, &projection).await;
        let service = SessionBindingService::new(Arc::new(pool.clone()), projection.clone());

        // 租户 cap=1：换账号 rebind 被自持旧 lease 占满租户额度，必须先释放
        // 旧 lease 再重试恰好一次（与账号 cap 同一 pre-release 路径）。
        let outcome = service
            .acquire_binding(rebind_request(fingerprint(8, 11), 30_000))
            .await
            .expect("tenant cap rebind");
        assert_eq!(pool.acquire_count(), 3, "seed + cap 拒绝 + 恰好一次重试");
        assert_eq!(
            pool.released(),
            vec![(old_lease.lease_id.clone(), LeaseOutcome::Completed)]
        );
        assert_eq!(pool.active_count(&AccountId::new("acct-1")), 0);
        assert_eq!(pool.active_count(&AccountId::new("acct-2")), 1);
        assert_eq!(outcome.binding.state, BindingState::Bound);
        assert_eq!(outcome.binding.revision, old_binding.revision + 2);
        assert_eq!(
            outcome.binding.ownership_epoch,
            old_binding.ownership_epoch + 1
        );
        assert_eq!(
            projection.snapshot(&key("tenant-a")),
            Some(outcome.binding.clone())
        );
    }

    #[tokio::test]
    async fn service_rebind_aborts_when_cap_occupied_by_other_lease() {
        let pool = CountingPool::new(InMemoryCredentialPool::new(1));
        let projection = Arc::new(InMemoryBindingProjection::new());
        let (old_binding, old_lease) = seed_bound(&pool, &projection).await;
        // 他人（非自持旧 lease）占满目标账号 acct-2：不是自锁，绝不得
        // pre-release 自己的活 lease。
        let other = pool
            .acquire(acquire_req(&key("tenant-a"), &target("acct-2", "cred-2")))
            .await
            .expect("other lease");
        let service = SessionBindingService::new(Arc::new(pool.clone()), projection.clone());

        let error = service
            .acquire_binding(rebind_request(fingerprint(8, 11), 30_000))
            .await
            .expect_err("other-held cap");
        assert!(matches!(
            error,
            BindingServiceError::Pool(PoolError::ConcurrencyExhausted { .. })
        ));
        // abort 回 Bound：旧 lease 未动、无新 lease、他人 lease 存活。
        assert_eq!(pool.acquire_count(), 3, "seed + other + 恰好一次失败");
        assert!(pool.released().is_empty());
        let restored = projection.snapshot(&key("tenant-a")).expect("restored");
        assert_eq!(restored.state, BindingState::Bound);
        assert_eq!(restored.lease_id, old_lease.lease_id);
        assert_eq!(restored.revision, old_binding.revision + 2);
        assert_eq!(restored.ownership_epoch, old_binding.ownership_epoch);
        assert_eq!(
            pool.lease_state(&other.lease_id),
            Some(LeaseState::Acquired)
        );
        let events = projection.events();
        assert!(matches!(
            events[2],
            BindingEvent::RebindingAborted { version: 3, .. }
        ));
    }

    #[tokio::test]
    async fn service_pre_release_retry_failure_fails_closed_to_released() {
        let pool = CountingPool::new(InMemoryCredentialPool::new(1));
        let projection = Arc::new(InMemoryBindingProjection::new());
        let (old_binding, old_lease) = seed_bound(&pool, &projection).await;
        // 同账号换凭据：首次 acquire 被自锁 cap 拒绝（call 2），pre-release 后
        // 重试（call 3）仍失败。
        pool.set_fail_on_acquire_call(3);
        let service = SessionBindingService::new(Arc::new(pool.clone()), projection.clone());

        let error = service
            .acquire_binding(rebind_request_full(
                "acct-1",
                "cred-2",
                fingerprint(8, 11),
                30_000,
                60_000,
            ))
            .await
            .expect_err("retry failure");
        assert!(matches!(
            error,
            BindingServiceError::PreReleaseRetryFailed {
                released: Ok(()),
                ..
            }
        ));

        // fail-closed：绝不回到 Bound 引用已释放 lease；行转 Released，投影 /
        // 事件一致（释放恰好一次、新 lease 不泄漏）。
        assert_eq!(pool.acquire_count(), 3);
        assert_eq!(
            pool.released(),
            vec![(old_lease.lease_id.clone(), LeaseOutcome::Completed)]
        );
        assert_eq!(pool.active_count(&AccountId::new("acct-1")), 0);
        let released = projection.snapshot(&key("tenant-a")).expect("row");
        assert_eq!(released.state, BindingState::Released);
        assert_eq!(released.revision, old_binding.revision + 2);
        assert_eq!(released.ownership_epoch, old_binding.ownership_epoch);
        let events = projection.events();
        let versions: Vec<u64> = events.iter().map(|event| event.version()).collect();
        assert_eq!(versions, vec![1, 2, 3]);
        assert!(matches!(
            events[2],
            BindingEvent::Released { version: 3, .. }
        ));
        // 事件日志重放重建 Released 快照（投影与事件一致）。
        let mut replay = None;
        for event in &events {
            replay = apply_event(replay, event).expect("replay");
        }
        assert_eq!(replay, Some(released));
    }

    #[tokio::test]
    async fn service_rebind_after_release_clears_pool_leftover_and_retries_once() {
        let pool = CountingPool::new(InMemoryCredentialPool::new(1));
        let projection = Arc::new(InMemoryBindingProjection::new());
        let (bound, old_lease) = seed_bound(&pool, &projection).await;
        let service = SessionBindingService::new(Arc::new(pool.clone()), projection.clone());

        // 制造「行已 Released、lease 仍占位」的残留：canonical release 的池
        // 释放失败。
        pool.set_fail_release(true);
        let release_error = service
            .release_binding(&key("tenant-a"), 70_000)
            .await
            .expect_err("pool release failure");
        assert!(matches!(release_error, BindingServiceError::Pool(_)));
        assert_eq!(
            pool.active_count(&AccountId::new("acct-1")),
            1,
            "lease 残留占位"
        );
        assert_eq!(
            projection.snapshot(&key("tenant-a")).expect("row").state,
            BindingState::Released
        );
        pool.set_fail_release(false);

        // 同账号重绑：先幂等释放残留 lease，再恰好一次重试成功。
        let outcome = service
            .acquire_binding(rebind_request_full(
                "acct-1",
                "cred-2",
                fingerprint(8, 11),
                72_000,
                60_000,
            ))
            .await
            .expect("rebind after release");
        assert_eq!(pool.acquire_count(), 3, "seed + cap 拒绝 + 恰好一次重试");
        assert_eq!(pool.active_count(&AccountId::new("acct-1")), 1);
        assert_eq!(outcome.binding.state, BindingState::Bound);
        assert_eq!(outcome.binding.revision, bound.revision + 2);
        assert_eq!(outcome.binding.ownership_epoch, bound.ownership_epoch + 1);
        assert_eq!(outcome.binding.credential_id, CredentialId::new("cred-2"));
        // 残留 lease 恰好针对旧 lease 释放（失败尝试 + pre-release 成功各一次），
        // 成功后不再重复释放。
        let releases = pool.released();
        assert_eq!(releases.len(), 2);
        assert!(releases.iter().all(|(id, _)| *id == old_lease.lease_id));
        assert!(matches!(outcome.old_lease_release, Some(Ok(_))));
        let events = projection.events();
        let versions: Vec<u64> = events.iter().map(|event| event.version()).collect();
        assert_eq!(versions, vec![1, 2, 3], "Released → Bound 事件严格连续");
        assert!(matches!(
            events[1],
            BindingEvent::Released { version: 2, .. }
        ));
        assert!(matches!(
            events[2],
            BindingEvent::Bound {
                version: 3,
                ownership_epoch: 1,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn service_rebind_after_release_retry_failure_keeps_released_row() {
        let pool = CountingPool::new(InMemoryCredentialPool::new(1));
        let projection = Arc::new(InMemoryBindingProjection::new());
        let (_, old_lease) = seed_bound(&pool, &projection).await;
        let service = SessionBindingService::new(Arc::new(pool.clone()), projection.clone());

        pool.set_fail_release(true);
        service
            .release_binding(&key("tenant-a"), 70_000)
            .await
            .expect_err("leave leftover");
        pool.set_fail_release(false);
        // 残留 lease 幂等释放后，重试 acquire（call 3）失败。
        pool.set_fail_on_acquire_call(3);

        let error = service
            .acquire_binding(rebind_request_full(
                "acct-1",
                "cred-2",
                fingerprint(8, 11),
                72_000,
                60_000,
            ))
            .await
            .expect_err("retry failure");
        assert!(matches!(
            error,
            BindingServiceError::Pool(PoolError::NoCandidate)
        ));

        // 重试失败：行保持 Released（含残留 lease），绝不产生 Bound + 已释放
        // lease，也不追加任何新事件。
        let row = projection.snapshot(&key("tenant-a")).expect("row");
        assert_eq!(row.state, BindingState::Released);
        assert_eq!(row.revision, 2);
        assert_eq!(
            pool.active_count(&AccountId::new("acct-1")),
            0,
            "残留 lease 已幂等释放"
        );
        assert_eq!(pool.acquire_count(), 3);
        assert_eq!(projection.events().len(), 2, "不得追加新事件");
        assert!(pool
            .released()
            .iter()
            .any(|(id, _)| *id == old_lease.lease_id));
    }

    #[tokio::test]
    async fn service_settle_only_released_and_gc_then_bind_continues_generation() {
        let pool = CountingPool::new(InMemoryCredentialPool::new(4));
        let projection = Arc::new(InMemoryBindingProjection::new());
        let (bound, _) = seed_bound(&pool, &projection).await;
        let service = SessionBindingService::new(Arc::new(pool.clone()), projection.clone());

        // Bound 行拒绝 settle（fail-closed），行保持原样。
        let denied = projection.settle(&key("tenant-a")).await;
        assert!(matches!(
            denied,
            Err(BindingProjectionError::NotReleased {
                state: BindingState::Bound,
                ..
            })
        ));
        assert_eq!(projection.snapshot(&key("tenant-a")), Some(bound.clone()));

        // canonical release → settle（GC）后行删除、事件日志保留。
        service
            .release_binding(&key("tenant-a"), 70_000)
            .await
            .expect("release");
        projection.settle(&key("tenant-a")).await.expect("settle");
        assert_eq!(projection.snapshot(&key("tenant-a")), None);
        assert_eq!(projection.events().len(), 2);

        // GC 后再 bind：从事件日志高水位延续 revision / epoch，事件严格连续，
        // 绝不重置 v1 / 复用旧 epoch。
        let outcome = service
            .acquire_binding(rebind_request(fingerprint(9, 13), 72_000))
            .await
            .expect("bind after gc");
        assert_eq!(outcome.binding.state, BindingState::Bound);
        assert_eq!(outcome.binding.revision, 3);
        assert_eq!(outcome.binding.ownership_epoch, 1);
        let events = projection.events();
        let versions: Vec<u64> = events.iter().map(|event| event.version()).collect();
        assert_eq!(versions, vec![1, 2, 3], "GC 后事件版本严格连续");
        assert!(matches!(
            events[2],
            BindingEvent::Bound {
                version: 3,
                ownership_epoch: 1,
                ..
            }
        ));
        // 事件日志重放重建最终 Bound 快照。
        let mut replay = None;
        for event in &events {
            replay = apply_event(replay, event).expect("replay");
        }
        assert_eq!(replay, Some(outcome.binding.clone()));
    }

    #[tokio::test]
    async fn service_recover_orphan_with_dead_lease_goes_released_not_bound() {
        let pool = CountingPool::new(InMemoryCredentialPool::new(4));
        let projection = Arc::new(InMemoryBindingProjection::new());
        let (binding, old_lease) = seed_bound(&pool, &projection).await;
        let epoch = binding.ownership_epoch;
        let (rebinding, started) = binding
            .begin_rebind(1, epoch, RebindReason::TtlExpired, 62_000)
            .expect("begin");
        projection
            .compare_and_apply(
                &key("tenant-a"),
                1,
                epoch,
                &rebinding,
                std::slice::from_ref(&started),
            )
            .await
            .expect("orphan CAS");
        // pre-release 后崩溃：旧 lease 已释放，池不再观测为 Acquired。
        pool.inner
            .release(old_lease.lease_id.clone(), LeaseOutcome::Completed)
            .await
            .expect("pre-release");
        let service = SessionBindingService::new(Arc::new(pool.clone()), projection.clone());

        let recovered = service.recover_outstanding(63_000).await.expect("recover");
        assert_eq!(recovered, 1);
        // 绝不 abort 回 Bound 引用已释放 lease：fail-closed 转 Released。
        let row = projection.snapshot(&key("tenant-a")).expect("row");
        assert_eq!(row.state, BindingState::Released);
        assert_eq!(row.revision, 3);
        assert_eq!(row.ownership_epoch, epoch);
        let events = projection.events();
        assert!(matches!(
            events[2],
            BindingEvent::Released { version: 3, .. }
        ));
    }

    #[tokio::test]
    async fn service_canonical_release_is_evented_idempotent_and_rebind_stays_continuous() {
        let pool = CountingPool::new(InMemoryCredentialPool::new(4));
        let projection = Arc::new(InMemoryBindingProjection::new());
        let (bound, old_lease) = seed_bound(&pool, &projection).await;
        let service = SessionBindingService::new(Arc::new(pool.clone()), projection.clone());

        let receipt = service
            .release_binding(&key("tenant-a"), 70_000)
            .await
            .expect("canonical release");
        assert_eq!(receipt.lease_id, old_lease.lease_id);
        assert!(!receipt.already_released);
        assert_eq!(receipt.outcome, LeaseOutcome::Released);
        let released = projection.snapshot(&key("tenant-a")).expect("row");
        assert_eq!(released.state, BindingState::Released);
        assert_eq!(released.revision, 2);
        assert_eq!(pool.active_count(&AccountId::new("acct-1")), 0);

        // 幂等：行已 Released，重试只做池释放（release 幂等）。
        let receipt2 = service
            .release_binding(&key("tenant-a"), 71_000)
            .await
            .expect("idempotent release");
        assert!(receipt2.already_released);

        // Released → Bound 延续 generation：绝不 settle 后重置 v1 / 复用 epoch。
        let outcome = service
            .acquire_binding(rebind_request(fingerprint(9, 13), 72_000))
            .await
            .expect("rebind after release");
        assert_eq!(outcome.binding.revision, 3);
        assert_eq!(outcome.binding.ownership_epoch, bound.ownership_epoch + 1);
        assert_eq!(outcome.binding.state, BindingState::Bound);
        let events = projection.events();
        let versions: Vec<u64> = events.iter().map(|event| event.version()).collect();
        assert_eq!(versions, vec![1, 2, 3], "事件版本严格连续");
        assert!(matches!(
            events[1],
            BindingEvent::Released { version: 2, .. }
        ));
        assert!(matches!(
            events[2],
            BindingEvent::Bound {
                version: 3,
                ownership_epoch: 1,
                ..
            }
        ));
        // 事件日志重放重建最终快照。
        let mut replay = None;
        for event in &events {
            replay = apply_event(replay, event).expect("replay");
        }
        assert_eq!(replay, Some(outcome.binding.clone()));
    }

    #[tokio::test]
    async fn service_reuse_checks_lease_state_and_fails_closed_when_unobservable() {
        let pool = CountingPool::new(InMemoryCredentialPool::new(4));
        let projection = Arc::new(InMemoryBindingProjection::new());
        let (_, old_lease) = seed_bound(&pool, &projection).await;
        let service = SessionBindingService::new(Arc::new(pool.clone()), projection.clone());

        // 活 lease：稳定复用。
        let reuse = service
            .acquire_binding(rebind_request_same_target(
                fingerprint(7, 11),
                30_000,
                60_000,
            ))
            .await
            .expect("reuse");
        assert_eq!(reuse.binding.lease_id, old_lease.lease_id);

        // lease 已死：Reuse 前校验拦截，转 LeaseLost rebind，绝不复用死 lease。
        pool.inner
            .release(old_lease.lease_id.clone(), LeaseOutcome::Completed)
            .await
            .expect("dead lease");
        let rebound = service
            .acquire_binding(rebind_request_same_target(
                fingerprint(7, 11),
                31_000,
                60_000,
            ))
            .await
            .expect("lease lost rebind");
        assert_ne!(rebound.binding.lease_id, old_lease.lease_id);
        assert_eq!(pool.acquire_count(), 2, "rebind 恰好 acquire 一次");

        // 池无法观测 lease 状态：fail-closed，不得静默复用。
        let opaque = NoLeaseStatePool(pool.clone());
        let opaque_service = SessionBindingService::new(Arc::new(opaque), projection.clone());
        let error = opaque_service
            .acquire_binding(rebind_request_same_target(
                fingerprint(7, 11),
                32_000,
                60_000,
            ))
            .await
            .expect_err("unobservable lease state");
        assert!(matches!(
            error,
            BindingServiceError::LeaseStateUnobservable { .. }
        ));
    }

    #[tokio::test]
    async fn service_load_outstanding_failure_fails_closed_without_pool_traffic() {
        let pool = CountingPool::new(InMemoryCredentialPool::new(4));
        let projection = Arc::new(InMemoryBindingProjection::new());
        seed_bound(&pool, &projection).await;
        let fault = FaultProjection::new(projection.clone());
        fault.set_fail_load_outstanding(true);
        let service = SessionBindingService::new(Arc::new(pool.clone()), Arc::new(fault));

        let error = service
            .recover_outstanding(70_000)
            .await
            .expect_err("outstanding failure");
        assert!(matches!(
            error,
            BindingServiceError::Projection(BindingProjectionError::Backend(_))
        ));
        assert_eq!(pool.acquire_count(), 1, "恢复扫描失败不得触碰 pool");
        assert!(pool.released().is_empty());
    }

    #[tokio::test]
    async fn service_concurrent_initial_bind_exactly_one_wins_and_loser_lease_released() {
        let pool = CountingPool::new(InMemoryCredentialPool::new(2));
        let projection = Arc::new(InMemoryBindingProjection::new());
        // 内存池 / 投影没有 yield 点：直接 `join!` 会串行化成「第二个请求复用
        // 第一个的结果」。用屏障投影强制两个请求都先读到 `None`，真实复现
        // 并发初始 bind 的 double-bind 竞态（insert 冲突决定唯一胜者）。
        let service = SessionBindingService::new(
            Arc::new(pool.clone()),
            Arc::new(DoubleBindBarrierProjection::new(projection.clone(), 2)),
        );

        let (a, b) = tokio::join!(
            service.acquire_binding(rebind_request(fingerprint(7, 11), 30_000)),
            service.acquire_binding(rebind_request(fingerprint(7, 11), 30_000)),
        );
        let winners = usize::from(a.is_ok()) + usize::from(b.is_ok());
        assert_eq!(winners, 1, "并发初始 bind 恰好一个成功");
        // 败者释放自己的 lease（Cancelled），不泄漏额度；快照键与请求键一致。
        assert_eq!(pool.acquire_count(), 2);
        let released = pool.released();
        assert_eq!(released.len(), 1);
        assert_eq!(released[0].1, LeaseOutcome::Cancelled);
        assert_eq!(pool.active_count(&AccountId::new("acct-2")), 1);
        let bound = projection.snapshot(&key("tenant-a")).expect("bound");
        assert_eq!(bound.key(), key("tenant-a"));
        assert_eq!(bound.revision, 1);
        assert_eq!(projection.event_count(), 1);
    }

    /// 包装：一切委托给内层，唯独 `lease_state` 恒 `None`（不可观测池）。
    #[derive(Clone)]
    struct NoLeaseStatePool(CountingPool);

    #[async_trait]
    impl CredentialPool for NoLeaseStatePool {
        async fn acquire(&self, req: AcquireRequest) -> Result<CredentialLease, PoolError> {
            self.0.acquire(req).await
        }

        async fn acquire_guard(&self, req: AcquireRequest) -> Result<LeaseGuard, PoolError> {
            self.0.acquire_guard(req).await
        }

        async fn release(
            &self,
            lease_id: LeaseId,
            outcome: LeaseOutcome,
        ) -> Result<ReleaseReceipt, PoolError> {
            self.0.release(lease_id, outcome).await
        }

        fn active_count(&self, account: &AccountId) -> u64 {
            self.0.active_count(account)
        }

        fn account_health(&self, account: &AccountId) -> AccountHealth {
            self.0.account_health(account)
        }
    }
}
