//! provider-control：Pawork 账号控制面（P18-4 最小契约，供 Phase 12 编排消费）。
//!
//! 本 crate 为 Agent 提供基于 Lease 的并发准入（CredentialPool）与幂等释放，
//! 作为账号侧的容量闸门，**不接触任何 API Key 或其它 Secret**：
//!
//! - [`CredentialPool`]：异步准入抽象，`acquire` / `acquire_guard` / `release`
//!   与账号健康查询；
//! - [`InMemoryCredentialPool`]：进程内默认实现，按账号限制并发（Lease 计数），
//!   释放幂等；取消（`Cancelled`）不计入连续失败，失败（`Failed`）才累加健康分；
//! - [`LeaseGuard`]：RAII 守卫，`Drop` 时以当前 outcome 自动释放持有的 lease，
//!   取消永远不会惩罚账号健康。
//!
//! 不依赖网络、数据库与 Secret 存储；测试仅使用 tokio 运行时。

use std::collections::HashMap;
use std::fmt;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use agent_domain::{AgentId, PrincipalId, ProviderId, SessionId, TenantId};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

// 账号与凭据标识已在 P18-1 上移至 `agent-domain`（opaque ID/value object 留在
// domain，见 plan step 3），此处仅 re-export 以保持 `provider_control::AccountId`
// 的既有导入路径不变（serde 形态与原本地定义一致，wire 兼容）。
pub use agent_domain::{AccountId, CredentialId};

// P18-4 lease 状态机的关键类型在 crate 根再导出，便于 `provider_control::LeaseState`
// 等既有导入习惯；完整集合见 [`lease`] 模块。
pub use lease::{
    FixedLeaseClock, InMemoryLeaseProjection, LeaseClock, LeaseEvent, LeaseProjection,
    LeaseProjectionError, LeaseRecord, LeaseState, LeaseTransitionError, NullLeaseProjection,
    ReclaimReport, SystemLeaseClock, LEASE_SCHEMA_VERSION,
};

// P18-7 session affinity / binding 状态机的关键类型在 crate 根再导出；完整集合
// 见 [`binding`] 模块。
pub use binding::{
    apply_event, fingerprint_hash, AffinityDecision, AffinityFingerprint, BindingAcquisition,
    BindingEvent, BindingKey, BindingProjection, BindingProjectionError, BindingServiceError,
    BindingState, BindingTarget, BindingTransitionError, InMemoryBindingProjection,
    NullBindingProjection, RebindReason, RebindRequest, SessionBinding, SessionBindingService,
    BINDING_SCHEMA_VERSION,
};
#[cfg(feature = "account-control-v1")]
pub use binding::{capability_fingerprint, policy_fingerprint};

/// 控制面 schema 版本（与 `core-api` / `app-database` 控制面迁移对齐，ADR-033）。
///
/// 所有新增持久化实体与 canonical event 必须携带该版本字段，支持版本化迁移与重放。
pub const CONTROL_PLANE_SCHEMA_VERSION: u32 = 2;

/// legacy 单凭据回退（始终可用，独立于 `account-control-v1` feature）。
pub mod legacy;

/// 版本化、evented 的 credential-lease 状态机（P18-4 canonical，始终可用）。
///
/// Canonical 生命周期 `Requested → Acquired → Released | Expired → Reclaimed`，
/// 含 [`lease::LeaseRecord`]（versioned，无 secret）、[`lease::LeaseEvent`]、
/// [`lease::LeaseProjection`]（对象安全持久化 sink）、[`lease::LeaseClock`] 与
/// 回收扫描类型。独立于 `account-control-v1`，可被关闭该 feature 的部署使用。
pub mod lease;

/// 版本化、evented 的 session affinity / binding 状态机（P18-7 canonical，始终可用）。
///
/// Canonical 生命周期 `Unbound → Bound → Rebinding → Bound`（或 `→ Released`），
/// 含 [`binding::SessionBinding`]（versioned，无 secret）、[`binding::BindingEvent`]、
/// [`binding::BindingProjection`]（对象安全持久化 sink，含 revision/ownership_epoch
/// CAS）与事件重放。独立于 `account-control-v1`；路由策略 `SessionAffinity` 的
/// 粘性状态由本模块持有，不在路由器内复制。
pub mod binding;

#[cfg(feature = "account-control-v1")]
pub mod account;
#[cfg(feature = "account-control-v1")]
pub mod classifier;
#[cfg(feature = "account-control-v1")]
pub mod credential;
#[cfg(feature = "account-control-v1")]
pub mod factory;
#[cfg(feature = "account-control-v1")]
pub mod health;
#[cfg(feature = "account-control-v1")]
pub mod reconciler;
#[cfg(feature = "account-control-v1")]
pub mod registry;
#[cfg(feature = "account-control-v1")]
pub mod repository;
#[cfg(feature = "account-control-v1")]
pub mod routing;
#[cfg(feature = "account-control-v1")]
pub use account::{
    AccountState, Clock, CredentialKind, CredentialMetadata, CredentialRecord, CredentialState,
    FixedClock, NotUsableReason, ProviderAccountRecord, RefreshState, SecretRef, SystemClock,
};
#[cfg(feature = "account-control-v1")]
pub use classifier::{
    ClassifierRegistry, ErrorClassifier, FailureClass, FailureClassification, FailureScope,
    HealthImpact, HttpErrorClassifier, ProviderClassifier, ProviderErrorKind, ProviderErrorSignal,
    Retryability,
};
#[cfg(feature = "account-control-v1")]
pub use credential::{
    BackendErrorCategory, CredentialResolver, InMemoryCredentialResolver, ResolveError,
};
#[cfg(feature = "account-control-v1")]
pub use factory::{
    ComposedProvider, FactoryError, ProtectorFactory, ProviderBuilder, ProviderDescriptor,
    ProviderFactory, SessionRunScope,
};
#[cfg(feature = "account-control-v1")]
pub use health::{
    BackoffPolicy, CircuitBreaker, CircuitConfig, CircuitState, CooldownKey, CooldownTracker,
    FailureContext, HealthProbe, HealthRecord, HealthRuntime, HealthState, ProbeBudget,
    ProbeFailure, ProbeKind, ProbeReport, ProbeRuntime, ProbeTargetKey,
};
#[cfg(feature = "account-control-v1")]
pub use reconciler::{
    AccountUsability, BindingDesiredView, DesiredBinding, PoolReconciler, ReconcileError,
    ReconcileReport,
};
#[cfg(feature = "account-control-v1")]
pub use registry::{
    ProviderRegistry, ProviderRegistrySnapshot, ProviderRegistryStage, RegistryError,
};
#[cfg(feature = "account-control-v1")]
pub use repository::{
    AccountBinding, CredentialSummary, CredentialTestStatus, InMemoryProviderAccountRepository,
    ProviderAccountRepository, ProviderAccountSummary, RepoError,
};
#[cfg(feature = "account-control-v1")]
pub use routing::{
    capabilities_of, AdmitAllHealth, CandidateRef, Capability, FallbackAction, FallbackKind,
    FallbackPlan, HealthVerdict, HealthView, LocalDefaultPolicy, PolicyDenial, RouteBudget,
    RouteCandidate, RouteContext, RouteDecision, RouteStep, RoutingError, RoutingPolicy,
    RoutingStrategy, SelectedRoute, SmoothWeightedPicker, TenantPolicy,
};

/// 类型安全的 lease 标识。
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct LeaseId(String);

impl LeaseId {
    /// 从任意可转换为 `String` 的值构造。
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// 返回内部字符串的借用视图。
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for LeaseId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// lease 释放时的结果分类。
///
/// 只有 `Failed` 会累加账号的连续失败计数；`Cancelled` 只累加取消计数，
/// 保证取消不惩罚账号健康。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LeaseOutcome {
    /// 任务正常完成。
    Completed,
    /// 任务被取消（不计入连续失败）。
    Cancelled,
    /// 任务失败（计入连续失败）。
    Failed,
    /// 显式释放，未完成也未失败。
    Released,
}

/// 申请 credential lease 的请求。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcquireRequest {
    /// 租户。
    pub tenant_id: TenantId,
    /// 发起主体。
    pub principal_id: PrincipalId,
    /// 会话。
    pub session_id: SessionId,
    /// 申请 lease 的 Agent。
    pub agent_id: AgentId,
    /// Provider；为 `None` 时由池使用默认值。
    pub provider_id: Option<ProviderId>,
    /// 账号；为 `None` 时由池使用默认值 `local/default`。
    pub account_id: Option<AccountId>,
    /// 可选的追踪标识，便于日志关联。
    pub trace_id: Option<String>,
}

/// 已授予的 credential lease。
///
/// 本结构**不包含任何 secret 字段**（无 API Key / Token），只携带定位信息。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CredentialLease {
    /// lease 唯一标识。
    pub lease_id: LeaseId,
    /// 实体 schema 版本（与 `app-database` `credential_leases` 迁移对齐）。
    pub schema_version: u32,
    /// 本次 lease 绑定的凭据（caller 可据此 resolve 短生命周期 secret；绝不携带明文）。
    pub credential_id: CredentialId,
    /// 被占用的账号。
    pub account_id: AccountId,
    /// 使用的 Provider。
    pub provider_id: ProviderId,
    /// 持有该 lease 的 Agent。
    pub agent_id: AgentId,
    /// 持有该 lease 的会话。
    pub session_id: SessionId,
    /// 发起主体（ownership：谁拥有本次 lease）。
    pub principal_id: PrincipalId,
    /// 所属租户。
    pub tenant_id: TenantId,
    /// 授予时刻（Unix 毫秒）。
    pub acquired_at_ms: u64,
    /// 过期时刻（`acquired_at_ms + ttl_ms`）；到期未释放由回收扫描归还额度。
    pub expires_at_ms: u64,
    /// 乐观并发版本号（canonical [`LeaseRecord`] 的 `version`，转换时自增）。
    pub version: u64,
}

/// 凭据池错误。
#[derive(Debug, thiserror::Error)]
pub enum PoolError {
    /// 没有可用的候选凭据。
    #[error("no credential candidate available")]
    NoCandidate,
    /// 账号并发额度已耗尽。
    #[error("concurrency exhausted for account {account}: active {active} of max {max}")]
    ConcurrencyExhausted {
        /// 被占满的账号。
        account: AccountId,
        /// 当前活跃 lease 数。
        active: u64,
        /// 账号并发上限。
        max: u64,
    },
    /// 租户并发额度已耗尽（per-tenant cap）。
    #[error("concurrency exhausted for tenant {tenant}: active {active} of max {max}")]
    TenantConcurrencyExhausted {
        /// 被占满的租户。
        tenant: TenantId,
        /// 当前租户活跃 lease 数。
        active: u64,
        /// 租户并发上限。
        max: u64,
    },
    /// 租户被拒绝。
    #[error("tenant denied: {reason}")]
    TenantDenied {
        /// 拒绝原因。
        reason: String,
    },
    /// 持久化投影失败（事务化保存 snapshot/events 失败）。
    #[error(transparent)]
    Projection(#[from] LeaseProjectionError),
}

/// 账号健康状态。
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct AccountHealth {
    /// 当前活跃 lease 数。
    pub active_leases: u64,
    /// 连续失败次数（仅 `Failed` 累加）。
    pub consecutive_failures: u64,
    /// 累计取消次数（`Cancelled`，不惩罚连续失败）。
    pub cancelled_count: u64,
}

/// 释放操作的回执。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReleaseReceipt {
    /// 被释放的 lease 标识。
    pub lease_id: LeaseId,
    /// 该 lease 是否已被释放过（幂等释放的第二次为 `true`）。
    pub already_released: bool,
    /// 本次释放携带的结果分类。
    pub outcome: LeaseOutcome,
}

/// 账号凭据池：基于 lease 的并发准入 + 幂等释放。
///
/// 实现约定：`release` 可在真实投影（如 `DatabaseActor`）上 `.await` 挂起；
/// [`LeaseGuard`] 的 `Drop` 先尝试同步完成释放，若 `Pending` 则把释放 future 交给
/// detached task 继续驱动到完成，确保释放最终发生、杜绝永久额度泄漏。
#[async_trait]
pub trait CredentialPool: Send + Sync {
    /// 申请一个 credential lease；超出账号并发上限时返回
    /// [`PoolError::ConcurrencyExhausted`]。
    async fn acquire(&self, req: AcquireRequest) -> Result<CredentialLease, PoolError>;

    /// 申请一个受 RAII 保护的 lease 守卫。
    ///
    /// 典型实现委托给 [`CredentialPool::acquire`]：先获取 lease，
    /// 再用 `Arc<dyn CredentialPool>`（由 `self` 克隆而来）构造 [`LeaseGuard`]。
    async fn acquire_guard(&self, req: AcquireRequest) -> Result<LeaseGuard, PoolError>;

    /// 释放 lease。幂等：未知或已释放的 lease 返回 `already_released = true`。
    async fn release(
        &self,
        lease_id: LeaseId,
        outcome: LeaseOutcome,
    ) -> Result<ReleaseReceipt, PoolError>;

    /// 账号当前活跃 lease 数；账号不存在时返回 0。
    fn active_count(&self, account: &AccountId) -> u64;

    /// 账号健康状态；账号不存在时返回 [`AccountHealth::default`]。
    fn account_health(&self, account: &AccountId) -> AccountHealth;

    /// 账号当前活跃 lease 数（**tenant-scoped canonical** 视图，P18-4 主审 #1）。
    ///
    /// 与 [`CredentialPool::active_count`] 的 legacy 聚合不同：本方法精确返回
    /// `(tenant, account)` 维度的活跃计数，跨租户同名账号互不影响。默认 0。
    fn active_count_for(&self, _tenant: &TenantId, _account: &AccountId) -> u64 {
        0
    }

    /// 账号健康状态（**tenant-scoped canonical** 视图）。默认 [`AccountHealth::default`]。
    fn account_health_for(&self, _tenant: &TenantId, _account: &AccountId) -> AccountHealth {
        AccountHealth::default()
    }

    /// 回收过期 / 已释放 lease 到终态 `Reclaimed`（幂等，无永久泄漏）。
    ///
    /// 扫描 `Acquired` 且已过 TTL 的 lease → `Expired`（归还并发额度），再把所有
    /// `Released`/`Expired` 回收到 `Reclaimed`，并事务化持久化每个 lease 的终态
    /// snapshot 与累计事件。异步方法（投影 `apply` 可在真实后端 await）；宿主在
    /// 启动 / 周期性心跳时调用。默认实现为空（无持久化的纯内存池可空跑）。
    async fn reclaim_expired(&self) -> Result<ReclaimReport, PoolError> {
        Ok(ReclaimReport::default())
    }

    /// 查询 lease 当前生命周期状态；未知 / 已 GC 返回 `None`。
    ///
    /// 默认实现返回 `None`（纯内存池可按需覆盖以暴露可观测性）。
    fn lease_state(&self, _lease_id: &LeaseId) -> Option<LeaseState> {
        None
    }

    /// 从持久化投影重建池状态（崩溃 / 重启恢复）。
    ///
    /// 读取投影中所有非终态 lease 快照，重建 active 计数并回收孤儿 lease。
    /// 默认实现为空（无持久化的池无需恢复）。实现约定：可在 `.await` 上挂起
    /// （不在 [`LeaseGuard`] 的 `Drop` 路径调用）。
    async fn restore(&self) -> Result<ReclaimReport, PoolError> {
        Ok(ReclaimReport::default())
    }
}

/// RAII 守卫：持有 `Option<CredentialLease>` 与 outcome，`Drop` 时把
/// 存储的 lease 以当前 outcome 释放。
///
/// - [`LeaseGuard::lease`]：借用查看持有的 lease；
/// - [`LeaseGuard::outcome_mut`]：在释放前改写结果分类（如标记取消/失败）；
/// - [`LeaseGuard::into_lease`]：取走 lease，`Drop` 不再产生任何释放副作用。
pub struct LeaseGuard {
    lease: Option<CredentialLease>,
    outcome: LeaseOutcome,
    pool: Arc<dyn CredentialPool>,
}

impl LeaseGuard {
    /// 借用查看持有的 lease（已被 `into_lease` 取走时为 `None`）。
    pub fn lease(&self) -> Option<&CredentialLease> {
        self.lease.as_ref()
    }

    /// 可变借用结果分类，供 `Drop` 释放时使用。
    pub fn outcome_mut(&mut self) -> &mut LeaseOutcome {
        &mut self.outcome
    }

    /// 取走持有的 lease 且不触发释放副作用；之后守卫的 `Drop` 为空操作。
    pub fn into_lease(mut self) -> Option<CredentialLease> {
        self.lease.take()
    }
}

impl Drop for LeaseGuard {
    fn drop(&mut self) {
        let Some(lease) = self.lease.take() else {
            return;
        };
        let outcome = self.outcome;
        let lease_id = lease.lease_id.clone();
        let pool = self.pool.clone();
        // release 的 async 体可能因真实投影（DatabaseActor）在 `.await` 挂起；release 现在
        // 返回 `Result`（durable-first：projection 失败时内存保持 Acquired、返回 Err）。
        // 先尝试同步完成（纯内存投影首次 poll 即 Ready）；若 Pending 则把 future 交给
        // detached task 继续驱动到完成。若 release 返回 Err（projection 持久化失败），
        // lease 在内存 / durable 均仍为 Acquired——不泄漏、不假成功，由 TTL / reclaim_expired
        // 可靠收敛。错误被显式日志（lease_id + error）以便诊断。
        let release_id = lease_id.clone();
        let mut future: Pin<
            Box<dyn std::future::Future<Output = Result<ReleaseReceipt, PoolError>> + Send>,
        > = Box::pin(async move { pool.release(release_id, outcome).await });
        match poll_pinned(future.as_mut()) {
            Some(Ok(receipt)) => {
                tracing::trace!(
                    lease_id = %receipt.lease_id,
                    already_released = receipt.already_released,
                    "lease released on guard drop (synchronous)"
                );
            }
            Some(Err(err)) => {
                tracing::error!(
                    lease_id = %lease_id,
                    error = %err,
                    "lease release failed on guard drop (projection); lease stays Acquired, \
                     will converge via TTL/reclaim_expired"
                );
            }
            None => {
                tracing::debug!(
                    lease_id = %lease_id,
                    "release pending on drop; spawning detached completion to avoid quota leak"
                );
                spawn_detached_release(future, lease_id);
            }
        }
    }
}

/// 用 noop waker 对 boxed future 单次 poll；立即完成返回输出，否则返回 `None`。
fn poll_pinned(
    future: Pin<&mut (dyn std::future::Future<Output = Result<ReleaseReceipt, PoolError>> + Send)>,
) -> Option<Result<ReleaseReceipt, PoolError>> {
    let waker = std::task::Waker::noop();
    let mut context = std::task::Context::from_waker(waker);
    match future.poll(&mut context) {
        std::task::Poll::Ready(receipt) => Some(receipt),
        std::task::Poll::Pending => None,
    }
}

/// 把未同步完成的释放 future 交给后台运行时继续驱动，保证释放最终完成。
fn spawn_detached_release(
    future: Pin<Box<dyn std::future::Future<Output = Result<ReleaseReceipt, PoolError>> + Send>>,
    lease_id: LeaseId,
) {
    // `lease_id` 会被 async 块 move 走，线程名与 spawn 失败日志用克隆。
    let lease_id_for_log = lease_id.clone();
    let detached = async move {
        match future.await {
            Ok(receipt) => tracing::info!(
                lease_id = %receipt.lease_id,
                already_released = receipt.already_released,
                "lease released via detached drop completion"
            ),
            Err(err) => tracing::error!(
                lease_id = %lease_id,
                error = %err,
                "detached lease release failed (projection); lease stays Acquired, \
                 will converge via TTL/reclaim_expired"
            ),
        }
    };
    match tokio::runtime::Handle::try_current() {
        Ok(handle) => {
            // 显式 detach：JoinHandle 不被 await，detached 任务在后台运行到完成。
            // `Handle::spawn` 不返回 Result（运行时关闭时任务被丢弃，错误在 await
            // JoinHandle 时才可见）；这里显式 drop handle 完成 detach。任务被丢弃时
            // lease 保持 Acquired，由 TTL / reclaim_expired 收敛（上方
            // "release pending on drop" 日志已记录该事件）。
            drop(handle.spawn(detached));
        }
        Err(_) => {
            let name = format!("pawork-lease-release-{}", lease_id_for_log);
            let _ = std::thread::Builder::new().name(name).spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("build detached lease-release runtime");
                runtime.block_on(detached);
            });
        }
    }
}

/// 默认 lease TTL（毫秒）：1 小时。`new(max)` 使用此值；到期由回收扫描归还额度。
pub const DEFAULT_LEASE_TTL_MS: u64 = 3_600_000;

/// Credential 选择器（对象安全）：为一次 acquire 决定绑定哪个 credential。
///
/// 池本身不接触凭据明文与 repository；宿主组合层注入基于账号仓库的真实选择器。
/// 默认 [`LegacyCredentialPicker`] 返回合成 `default` 凭据（legacy 单凭据回退），
/// 满足 P18-4 在未接入 repository 时的独立可用性。
pub trait CredentialPicker: Send + Sync {
    /// 按 `(tenant, account, provider)` 选出一个 credential_id（opaque，无明文）。
    fn pick(
        &self,
        tenant: &TenantId,
        account: &AccountId,
        provider: &ProviderId,
    ) -> Result<CredentialId, PoolError>;
}

/// 默认选择器：始终返回合成 `default` 凭据（legacy 单凭据回退）。
#[derive(Clone, Copy, Debug, Default)]
pub struct LegacyCredentialPicker;

impl CredentialPicker for LegacyCredentialPicker {
    fn pick(
        &self,
        _tenant: &TenantId,
        _account: &AccountId,
        _provider: &ProviderId,
    ) -> Result<CredentialId, PoolError> {
        Ok(CredentialId::new("default"))
    }
}

/// 池并发 / 期限配置。
///
/// 准入按 `(tenant, account)` 原子判定：先校验可选的 per-tenant 上限，再校验
/// per-account 上限（`per_account_overrides` 命中优先，否则 `max_concurrency_per_account`）。
#[derive(Clone, Debug)]
pub struct PoolConfig {
    /// 每账号并发上限（默认值；未在 `per_account_overrides` 命中的账号使用）。
    pub max_concurrency_per_account: u64,
    /// 可选的每租户并发上限（`None` 表示不限租户）。
    pub max_concurrency_per_tenant: Option<u64>,
    /// 默认 lease TTL（毫秒）。
    pub default_ttl_ms: u64,
    /// 按账号覆盖的并发上限（key = `(tenant, account)`）。
    pub per_account_overrides: HashMap<(TenantId, AccountId), u64>,
}

impl PoolConfig {
    /// 以每账号并发上限与默认 TTL 构造（不限租户）。
    pub fn new(max_concurrency_per_account: u64) -> Self {
        Self {
            max_concurrency_per_account,
            max_concurrency_per_tenant: None,
            default_ttl_ms: DEFAULT_LEASE_TTL_MS,
            per_account_overrides: HashMap::new(),
        }
    }

    /// 设置每租户并发上限（builder）。
    pub fn with_tenant_cap(mut self, max: u64) -> Self {
        self.max_concurrency_per_tenant = Some(max);
        self
    }

    /// 设置默认 lease TTL（builder）。
    pub fn with_ttl_ms(mut self, ttl_ms: u64) -> Self {
        self.default_ttl_ms = ttl_ms;
        self
    }

    /// 覆盖某 `(tenant, account)` 的并发上限（builder）。
    pub fn with_account_override(mut self, tenant: TenantId, account: AccountId, max: u64) -> Self {
        self.per_account_overrides.insert((tenant, account), max);
        self
    }

    /// 解析某账号的有效并发上限（override 优先，否则默认）。
    pub fn max_for(&self, tenant: &TenantId, account: &AccountId) -> u64 {
        self.per_account_overrides
            .get(&(tenant.clone(), account.clone()))
            .copied()
            .unwrap_or(self.max_concurrency_per_account)
    }
}

impl Default for PoolConfig {
    fn default() -> Self {
        Self::new(1)
    }
}

/// Durable lease 标识生成器：避免 `pid + counter` 在重启后碰撞（P18-4 主审 #6）。
///
/// `system()` 以「进程内全局启动序号 + 墙钟纳秒 + pid」组合成一次性前缀，再接单调
/// 计数器。同一进程内每次构造都拿到不同的启动序号 → 前缀必然不同；跨进程（重启）
/// 时 pid / 纳秒至少其一不同。因此重启后即便 pid 被操作系统复用、计数器归零，
/// 生成的 lease_id 也绝不与历史值碰撞。
pub struct LeaseIdGenerator {
    prefix: String,
    counter: AtomicU64,
}

static LEASE_ID_BOOT_SEQ: AtomicU64 = AtomicU64::new(0);

impl LeaseIdGenerator {
    /// 生产生成器：前缀来自启动序号 + 墙钟纳秒 + pid，重启安全。
    pub fn system() -> Self {
        let seq = LEASE_ID_BOOT_SEQ.fetch_add(1, Ordering::Relaxed);
        let boot_ns = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        Self {
            prefix: format!("{seq:x}-{boot_ns:x}-{}", std::process::id()),
            counter: AtomicU64::new(0),
        }
    }

    /// 注入固定前缀（确定性测试用）。
    pub fn with_prefix(prefix: impl Into<String>) -> Self {
        Self {
            prefix: prefix.into(),
            counter: AtomicU64::new(0),
        }
    }

    /// 生成下一个全局唯一 lease_id。
    pub fn next(&self) -> LeaseId {
        let n = self.counter.fetch_add(1, Ordering::Relaxed);
        LeaseId::new(format!("lease-{}-{n}", self.prefix))
    }
}

impl Default for LeaseIdGenerator {
    fn default() -> Self {
        Self::system()
    }
}

/// 进程内默认凭据池：canonical versioned/evented lease 状态机 + 原子并发准入。
///
/// - 单一 `std::sync::Mutex` 保护全部状态，临界区内**绝不** `.await`；
/// - 准入按 `(tenant, account)` 原子判定，永不超配；
/// - lease 以 [`LeaseRecord`]（versioned）保存，`release`/`reclaim` 幂等；
/// - 可选 [`LeaseProjection`] 提供崩溃 / 重启恢复（[`CredentialPool::restore`]）。
///
/// **Crash-consistency（P18-4 主审修复）**：`release`/`reclaim_expired`/`restore`
/// 全部 durable-first——先持久化转换成功，才提交内存状态与额度；投影失败返回显式
/// `Err`，内存与 durable 均保持原状态（`Acquired`），由 TTL / 下次回收收敛。同一
/// lease 同一时刻至多一条「持久化中」的转换（`inflight` 标记），并发 double-release
/// 只产生一条 Released 事件，durable 状态永不回退。
#[derive(Clone)]
pub struct InMemoryCredentialPool {
    inner: Arc<Mutex<PoolState>>,
    id_generator: Arc<LeaseIdGenerator>,
    clock: Arc<dyn LeaseClock>,
    projection: Arc<dyn LeaseProjection>,
    picker: Arc<dyn CredentialPicker>,
}

/// 池内部状态（单一互斥锁保护，临界区内无任何 await）。
struct PoolState {
    config: PoolConfig,
    /// 按 `(tenant, account)` 键控——跨租户同名账号绝不再错误共享 active/health。
    accounts: HashMap<(TenantId, AccountId), PoolAccountState>,
    /// 每租户当前活跃 lease 数（per-tenant cap 判定）。
    tenants: HashMap<TenantId, u64>,
    /// canonical lease 记录（versioned）；终态后由回收扫描移除。
    leases: HashMap<LeaseId, LeaseRecord>,
    /// durable-pending 状态转换（release/expire/reclaim 已计算 next 但尚未持久化成功）。
    ///
    /// 存在即表示该 lease 正处在「durable-first」提交窗口内：其他转换（并发
    /// double-release、expire、reclaim）必须跳过它。这保证同一 lease 同一时刻至多
    /// 一条持久化中的转换——事件不重复、durable 状态不回退（不会出现 Released 覆盖
    /// 已持久化的 Reclaimed 等版本倒灌）。投影失败时移除标记，内存与 durable 均保持
    /// 原状态（Acquired），由 TTL / 下次回收收敛。
    inflight: HashMap<LeaseId, LeaseRecord>,
}

/// 单个账号的运行时状态。
#[derive(Default)]
struct PoolAccountState {
    active: u64,
    consecutive_failures: u64,
    cancelled_count: u64,
}

/// 获取锁，对 poisoned mutex 恢复（避免 panic 中断服务）。
fn lock(inner: &Mutex<PoolState>) -> std::sync::MutexGuard<'_, PoolState> {
    inner
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl InMemoryCredentialPool {
    /// 创建按账号并发上限为 `max_concurrency_per_account` 的内存池
    /// （系统时钟、空投影、legacy picker、默认 1h TTL）。
    pub fn new(max_concurrency_per_account: u64) -> Self {
        Self::with_config(PoolConfig::new(max_concurrency_per_account))
    }

    /// 以完整配置构造（系统时钟、空投影、legacy picker）。
    pub fn with_config(config: PoolConfig) -> Self {
        Self::build(
            config,
            Arc::new(SystemLeaseClock),
            Arc::new(NullLeaseProjection),
            Arc::new(LegacyCredentialPicker),
        )
    }

    /// 注入固定时钟（确定性过期 / 回收测试）。
    pub fn with_clock(config: PoolConfig, clock: Arc<dyn LeaseClock>) -> Self {
        Self::build(
            config,
            clock,
            Arc::new(NullLeaseProjection),
            Arc::new(LegacyCredentialPicker),
        )
    }

    /// 注入持久化投影（崩溃 / 重启恢复）。
    pub fn with_projection(
        config: PoolConfig,
        clock: Arc<dyn LeaseClock>,
        projection: Arc<dyn LeaseProjection>,
    ) -> Self {
        Self::build(config, clock, projection, Arc::new(LegacyCredentialPicker))
    }

    /// 完整构造（配置 + 时钟 + 投影 + 选择器）。
    pub fn build(
        config: PoolConfig,
        clock: Arc<dyn LeaseClock>,
        projection: Arc<dyn LeaseProjection>,
        picker: Arc<dyn CredentialPicker>,
    ) -> Self {
        Self::build_with_generator(
            config,
            clock,
            projection,
            picker,
            Arc::new(LeaseIdGenerator::system()),
        )
    }

    /// 全量构造并注入 lease_id 生成器（确定性测试用）。
    pub fn build_with_generator(
        config: PoolConfig,
        clock: Arc<dyn LeaseClock>,
        projection: Arc<dyn LeaseProjection>,
        picker: Arc<dyn CredentialPicker>,
        id_generator: Arc<LeaseIdGenerator>,
    ) -> Self {
        Self {
            inner: Arc::new(Mutex::new(PoolState {
                config,
                accounts: HashMap::new(),
                tenants: HashMap::new(),
                leases: HashMap::new(),
                inflight: HashMap::new(),
            })),
            id_generator,
            clock,
            projection,
            picker,
        }
    }
}

#[async_trait]
impl CredentialPool for InMemoryCredentialPool {
    async fn acquire(&self, req: AcquireRequest) -> Result<CredentialLease, PoolError> {
        let account = req
            .account_id
            .clone()
            .unwrap_or_else(|| AccountId::new("local/default"));
        let provider = req
            .provider_id
            .clone()
            .unwrap_or_else(|| ProviderId::new("default"));
        // credential 在锁外选择（picker 不依赖池状态；与 self.inner 的 disjoint borrow）。
        let credential = self.picker.pick(&req.tenant_id, &account, &provider)?;

        // 临界区内只做准入判定与计数；事件持久化在锁外（projection.apply 可 await）。
        // 注意：**不在 acquire 热路径做懒过期**（P18-4 主审修复）。懒过期会先归还内存额度
        // 再 best-effort 持久化，持久化失败时 durable 仍 Acquired → 重启错误恢复 active
        // （split-brain）。过期 lease 由 `reclaim_expired`（心跳 / 启动）crash-consistently
        // 回收；acquire 只看当前计数，额度满则返回 `ConcurrencyExhausted`。
        let (record, requested_ev, acquired_ev) = {
            let mut state = lock(&self.inner);

            // per-tenant cap。
            if let Some(max_t) = state.config.max_concurrency_per_tenant {
                let tenant_active = *state.tenants.get(&req.tenant_id).unwrap_or(&0);
                if tenant_active >= max_t {
                    return Err(PoolError::TenantConcurrencyExhausted {
                        tenant: req.tenant_id.clone(),
                        active: tenant_active,
                        max: max_t,
                    });
                }
            }
            // per-account cap（canonical 键为 `(tenant, account)`：跨租户同名账号不再共享）。
            let key = (req.tenant_id.clone(), account.clone());
            let max_a = state.config.max_for(&req.tenant_id, &account);
            let active_now = state.accounts.get(&key).map_or(0, |a| a.active);
            if active_now >= max_a {
                return Err(PoolError::ConcurrencyExhausted {
                    account: account.clone(),
                    active: active_now,
                    max: max_a,
                });
            }

            // 全部准入通过：物化 versioned lease + 自增计数，并收集 canonical 事件（不再丢弃）。
            let lease_id = self.id_generator.next();
            let (record, requested_ev, acquired_ev) = LeaseRecord::open(
                &req,
                lease_id,
                credential.clone(),
                &*self.clock,
                state.config.default_ttl_ms,
            );
            *state.tenants.entry(req.tenant_id.clone()).or_insert(0) += 1;
            state.accounts.entry(key).or_default().active += 1;
            state.leases.insert(record.lease_id.clone(), record.clone());
            (record, requested_ev, acquired_ev)
        };

        // 事务化持久化本次 acquire 的 snapshot + 两条事件；失败回滚内存态（额度不泄漏）。
        if let Err(err) = self
            .projection
            .apply(&record, &[requested_ev.clone(), acquired_ev.clone()])
            .await
        {
            tracing::error!(
                lease_id = %record.lease_id,
                error = %err,
                "lease projection apply failed; rolling back in-memory acquire"
            );
            self.rollback_acquire(&record);
            return Err(PoolError::Projection(err));
        }
        tracing::trace!(
            lease_id = %record.lease_id,
            account = %record.account_id,
            "credential lease acquired"
        );
        Ok(record.to_public_lease())
    }

    async fn acquire_guard(&self, req: AcquireRequest) -> Result<LeaseGuard, PoolError> {
        let lease = self.acquire(req).await?;
        Ok(LeaseGuard {
            lease: Some(lease),
            // Fail-safe default: callers must explicitly mark Completed/Cancelled.
            // A panic, task abort, or early return must never be accounted as success.
            outcome: LeaseOutcome::Failed,
            pool: Arc::new(self.clone()),
        })
    }

    async fn release(
        &self,
        lease_id: LeaseId,
        outcome: LeaseOutcome,
    ) -> Result<ReleaseReceipt, PoolError> {
        // Phase 1（锁内，纯计算）：校验 Acquired 且不在转换中，计算 Released 转换并
        // 登记 inflight。**不改 leases / 计数**——durable 成功前内存保持 Acquired。
        let (next, event, original_version) = {
            let mut state = lock(&self.inner);
            let Some(record) = state.leases.get(&lease_id).cloned() else {
                return Ok(ReleaseReceipt {
                    lease_id,
                    already_released: true,
                    outcome,
                });
            };
            if record.state.is_settled() || state.inflight.contains_key(&lease_id) {
                return Ok(ReleaseReceipt {
                    lease_id,
                    already_released: true,
                    outcome,
                });
            }
            let original_version = record.version;
            let (next, event) = match record.release(outcome, &*self.clock) {
                Ok(v) => v,
                // 防御：锁内已确认 Acquired，状态机转换不应失败。
                Err(_) => {
                    return Ok(ReleaseReceipt {
                        lease_id,
                        already_released: true,
                        outcome,
                    });
                }
            };
            state.inflight.insert(lease_id.clone(), next.clone());
            (next, event, original_version)
        };

        // Phase 2（锁外）：durable-first——先持久化 Released snapshot + 事件。
        if let Err(err) = self
            .projection
            .apply(&next, std::slice::from_ref(&event))
            .await
        {
            self.abort_inflight(&next.lease_id);
            tracing::error!(
                lease_id = %next.lease_id,
                error = %err,
                "lease projection apply failed on release; memory/durable stay Acquired, \
                 will converge via TTL/reclaim_expired"
            );
            return Err(PoolError::Projection(err));
        }

        // Phase 3（锁内）：durable 成功才提交内存并归还额度（CAS 校验版本未被并发改动）。
        {
            let mut state = lock(&self.inner);
            state.inflight.remove(&lease_id);
            let still_ours = state
                .leases
                .get(&lease_id)
                .is_some_and(|r| r.version == original_version);
            if still_ours {
                state.leases.insert(lease_id.clone(), next.clone());
                let key = (next.tenant_id.clone(), next.account_id.clone());
                if let Some(account_state) = state.accounts.get_mut(&key) {
                    account_state.active = account_state.active.saturating_sub(1);
                    match outcome {
                        LeaseOutcome::Cancelled => account_state.cancelled_count += 1,
                        LeaseOutcome::Failed => account_state.consecutive_failures += 1,
                        LeaseOutcome::Completed | LeaseOutcome::Released => {}
                    }
                }
                if let Some(tenant_active) = state.tenants.get_mut(&next.tenant_id) {
                    *tenant_active = tenant_active.saturating_sub(1);
                }
            }
        }
        tracing::trace!(lease_id = %lease_id, ?outcome, "credential lease released");
        Ok(ReleaseReceipt {
            lease_id,
            already_released: false,
            outcome,
        })
    }

    /// 账号当前活跃 lease 数（跨租户同名账号求和的 legacy 聚合视图）。
    fn active_count(&self, account: &AccountId) -> u64 {
        lock(&self.inner)
            .accounts
            .iter()
            .filter(|((_, acct), _)| acct == account)
            .map(|(_, state)| state.active)
            .sum()
    }

    /// 账号健康状态（跨租户同名账号求和的 legacy 聚合视图）。
    fn account_health(&self, account: &AccountId) -> AccountHealth {
        let mut aggregated = AccountHealth::default();
        for ((_, acct), state) in lock(&self.inner).accounts.iter() {
            if acct == account {
                aggregated.active_leases += state.active;
                aggregated.consecutive_failures += state.consecutive_failures;
                aggregated.cancelled_count += state.cancelled_count;
            }
        }
        aggregated
    }

    /// 账号当前活跃 lease 数（tenant-scoped canonical 视图）。
    fn active_count_for(&self, tenant: &TenantId, account: &AccountId) -> u64 {
        lock(&self.inner)
            .accounts
            .get(&(tenant.clone(), account.clone()))
            .map_or(0, |state| state.active)
    }

    /// 账号健康状态（tenant-scoped canonical 视图）。
    fn account_health_for(&self, tenant: &TenantId, account: &AccountId) -> AccountHealth {
        lock(&self.inner)
            .accounts
            .get(&(tenant.clone(), account.clone()))
            .map_or_else(AccountHealth::default, |state| AccountHealth {
                active_leases: state.active,
                consecutive_failures: state.consecutive_failures,
                cancelled_count: state.cancelled_count,
            })
    }

    /// 回收过期 / 已释放 lease 到终态 `Reclaimed`（幂等，无永久泄漏）。
    ///
    /// 扫描 `Acquired` 且已过 TTL 的 lease → `Expired`（归还并发额度），再把所有
    /// `Released`/`Expired` 回收到 `Reclaimed`，并事务化持久化每个 lease 的终态
    /// snapshot 与累计事件。异步：投影 `apply` 可在真实后端 await。
    ///
    /// **Crash-consistency（P18-4 主审修复）**：逐条 durable-first——先持久化转换
    /// 成功，才提交内存状态 / 归还额度；任一投影失败立即返回 `Err`（不吞错），
    /// 该 lease 内存与 durable 均保持原状态，未处理的 lease 留给下次回收。
    async fn reclaim_expired(&self) -> Result<ReclaimReport, PoolError> {
        let mut report = ReclaimReport::default();

        // Phase 1：过 TTL 的 Acquired → Expired（归还额度），逐条 durable-first。
        while let Some((next, event, original_version)) = self.plan_expire_one() {
            if let Err(err) = self
                .projection
                .apply(&next, std::slice::from_ref(&event))
                .await
            {
                self.abort_inflight(&next.lease_id);
                tracing::error!(
                    lease_id = %next.lease_id,
                    error = %err,
                    "lease expire persist failed; memory/durable stay Acquired, retry next cycle"
                );
                return Err(PoolError::Projection(err));
            }
            self.commit_expire(&next, original_version);
            report.expired += 1;
        }

        // Phase 2：settled（Released/Expired）→ Reclaimed（终态回收），逐条 durable-first。
        while let Some((next, event, original_version)) = self.plan_reclaim_one() {
            if let Err(err) = self
                .projection
                .apply(&next, std::slice::from_ref(&event))
                .await
            {
                self.abort_inflight(&next.lease_id);
                tracing::error!(
                    lease_id = %next.lease_id,
                    error = %err,
                    "lease reclaim persist failed; memory stays settled, retry next cycle"
                );
                return Err(PoolError::Projection(err));
            }
            self.commit_reclaim(&next, original_version);
            report.reclaimed += 1;
            report.lease_ids.push(next.lease_id.clone());
        }
        Ok(report)
    }

    fn lease_state(&self, lease_id: &LeaseId) -> Option<LeaseState> {
        lock(&self.inner)
            .leases
            .get(lease_id)
            .map(|record| record.state)
    }

    async fn restore(&self) -> Result<ReclaimReport, PoolError> {
        let records = self
            .projection
            .load_outstanding()
            .await
            .map_err(PoolError::Projection)?;
        // 纯计算：划分为「需持久化的终态转换」与「存活 lease」。
        let (report, terminal_batch, live) = plan_restore(records, &*self.clock);
        // durable-first：先持久化全部终态转换；任一失败 → Err，内存保持空（不提交
        // 任何状态，重启可重试）。终态转换幂等，重复执行不产生额外事件。
        for (snapshot, events) in &terminal_batch {
            if let Err(err) = self.projection.apply(snapshot, events).await {
                tracing::error!(
                    lease_id = %snapshot.lease_id,
                    error = %err,
                    "restore persist failed; pool stays empty, retry on next restore"
                );
                return Err(PoolError::Projection(err));
            }
        }
        // durable 全部成功后才提交内存（重建 active 计数）。
        {
            let mut state = lock(&self.inner);
            for record in live {
                *state.tenants.entry(record.tenant_id.clone()).or_insert(0) += 1;
                state
                    .accounts
                    .entry((record.tenant_id.clone(), record.account_id.clone()))
                    .or_default()
                    .active += 1;
                state.leases.insert(record.lease_id.clone(), record);
            }
        }
        Ok(report)
    }
}

impl InMemoryCredentialPool {
    /// 从给定记录重建池状态（纯内存，不做投影 I/O；测试 / 低层恢复入口）。
    ///
    /// 与 [`CredentialPool::restore`] 的差异：不持久化终态转换（纯内存池 / 调用方
    /// 自行持久化）。报告语义与 `restore` 一致（expired/reclaimed 计数、存活 lease
    /// 重建 active）。
    pub fn recover_records(&self, records: Vec<LeaseRecord>) -> ReclaimReport {
        let (report, _terminal_batch, live) = plan_restore(records, &*self.clock);
        let mut state = lock(&self.inner);
        for record in live {
            *state.tenants.entry(record.tenant_id.clone()).or_insert(0) += 1;
            state
                .accounts
                .entry((record.tenant_id.clone(), record.account_id.clone()))
                .or_default()
                .active += 1;
            state.leases.insert(record.lease_id.clone(), record);
        }
        report
    }

    /// 锁内登记 inflight 标记；投影失败时调用（内存 / durable 均保持原状态）。
    fn abort_inflight(&self, lease_id: &LeaseId) {
        lock(&self.inner).inflight.remove(lease_id);
    }

    /// 锁内选一个「过 TTL 且不在转换中」的 Acquired lease，计算 Expired 转换并登记
    /// inflight；无候选返回 `None`。不改 leases / 计数（durable 成功后由
    /// [`Self::commit_expire`] 提交）。
    fn plan_expire_one(&self) -> Option<(LeaseRecord, LeaseEvent, u64)> {
        let mut state = lock(&self.inner);
        let now = self.clock.now();
        let id = state
            .leases
            .iter()
            .find(|(id, record)| record.is_past_ttl(now) && !state.inflight.contains_key(id))
            .map(|(id, _)| id.clone())?;
        let record = state.leases.get(&id).cloned()?;
        let original_version = record.version;
        let (next, event) = record.clone().expire(&*self.clock).ok()?;
        state.inflight.insert(id, next.clone());
        Some((next, event, original_version))
    }

    /// 锁内提交 Expired 转换（durable 成功后调用）：更新 leases、归还 account/tenant 额度。
    fn commit_expire(&self, next: &LeaseRecord, original_version: u64) {
        let mut state = lock(&self.inner);
        state.inflight.remove(&next.lease_id);
        let still_ours = state
            .leases
            .get(&next.lease_id)
            .is_some_and(|r| r.version == original_version);
        if !still_ours {
            return;
        }
        let key = (next.tenant_id.clone(), next.account_id.clone());
        if let Some(account_state) = state.accounts.get_mut(&key) {
            account_state.active = account_state.active.saturating_sub(1);
        }
        if let Some(tenant_active) = state.tenants.get_mut(&next.tenant_id) {
            *tenant_active = tenant_active.saturating_sub(1);
        }
        state.leases.insert(next.lease_id.clone(), next.clone());
    }

    /// 锁内选一个「settled 且不在转换中」的 lease，计算 Reclaimed 转换并登记
    /// inflight；无候选返回 `None`。不改 leases（durable 成功后由
    /// [`Self::commit_reclaim`] 提交）。
    fn plan_reclaim_one(&self) -> Option<(LeaseRecord, LeaseEvent, u64)> {
        let mut state = lock(&self.inner);
        let id = state
            .leases
            .iter()
            .find(|(id, record)| record.state.is_settled() && !state.inflight.contains_key(id))
            .map(|(id, _)| id.clone())?;
        let record = state.leases.get(&id).cloned()?;
        let original_version = record.version;
        let (next, event) = record.clone().reclaim(&*self.clock).ok()?;
        state.inflight.insert(id, next.clone());
        Some((next, event, original_version))
    }

    /// 锁内提交 Reclaimed 转换（durable 成功后调用）：终态 lease 移出内存
    /// （事件保留在 durable，审计闭环）。
    fn commit_reclaim(&self, next: &LeaseRecord, original_version: u64) {
        let mut state = lock(&self.inner);
        state.inflight.remove(&next.lease_id);
        let still_ours = state
            .leases
            .get(&next.lease_id)
            .is_some_and(|r| r.version == original_version);
        if still_ours {
            state.leases.remove(&next.lease_id);
        }
    }

    /// 回滚一次未持久化成功的 acquire：移除 lease、归还 tenant/account 计数。
    fn rollback_acquire(&self, record: &LeaseRecord) {
        let mut state = lock(&self.inner);
        let key = (record.tenant_id.clone(), record.account_id.clone());
        state.leases.remove(&record.lease_id);
        if let Some(account_state) = state.accounts.get_mut(&key) {
            account_state.active = account_state.active.saturating_sub(1);
        }
        if let Some(tenant_active) = state.tenants.get_mut(&record.tenant_id) {
            *tenant_active = tenant_active.saturating_sub(1);
        }
    }
}

/// 恢复规划结果：`(报告, 需持久化的终态转换批次, 存活 lease)`。
type RestorePlan = (
    ReclaimReport,
    Vec<(LeaseRecord, Vec<LeaseEvent>)>,
    Vec<LeaseRecord>,
);

/// 纯计算：把恢复输入记录划分为「存活 lease（需重建 active）」「需持久化的终态
/// 转换批次」与报告。不改任何状态（`restore` 与 `recover_records` 共用）。
///
/// - `Acquired` 且未过 TTL：存活 → 重建 active 计数；
/// - `Acquired` 且已过 TTL：→ Expired → Reclaimed（不重建 active），事件两条；
/// - `Released` / `Expired`：→ Reclaimed，事件一条；
/// - `Requested` / `Reclaimed`：不应出现在 outstanding，忽略。
fn plan_restore(records: Vec<LeaseRecord>, clock: &dyn LeaseClock) -> RestorePlan {
    let now = clock.now();
    let mut expired = 0u64;
    let mut reclaimed_ids = Vec::new();
    let mut terminal_batch = Vec::new();
    let mut live = Vec::new();
    for record in records {
        match record.state {
            LeaseState::Acquired if !record.is_past_ttl(now) => live.push(record),
            LeaseState::Acquired => {
                // 过 TTL 的孤儿：直接走到 Reclaimed，不重建 active。
                let mut events = Vec::new();
                if let Ok((expired_record, ev)) = record.clone().expire(clock) {
                    events.push(ev);
                    if let Ok((reclaimed_record, ev2)) = expired_record.reclaim(clock) {
                        events.push(ev2);
                        reclaimed_ids.push(reclaimed_record.lease_id.clone());
                        terminal_batch.push((reclaimed_record, events));
                        expired += 1;
                    }
                }
            }
            LeaseState::Released | LeaseState::Expired => {
                if let Ok((reclaimed_record, ev)) = record.clone().reclaim(clock) {
                    reclaimed_ids.push(reclaimed_record.lease_id.clone());
                    terminal_batch.push((reclaimed_record, vec![ev]));
                }
            }
            _ => {
                // Requested / Reclaimed：不应出现在 outstanding，忽略。
            }
        }
    }
    let report = ReclaimReport {
        expired,
        reclaimed: reclaimed_ids.len() as u64,
        lease_ids: reclaimed_ids,
        persist_errors: 0,
    };
    (report, terminal_batch, live)
}

#[cfg(test)]
mod tests {
    use super::*;

    // 池代码本身不再命名 `Timestamp` 类型，但测试仍用 `FixedLeaseClock` 构造时间。
    use agent_domain::Timestamp;

    /// 构造最小样例请求；`account` 为 `None` 时由池使用默认账号。
    fn sample_request(account: Option<AccountId>) -> AcquireRequest {
        AcquireRequest {
            tenant_id: TenantId::new("tenant-a"),
            principal_id: PrincipalId::new("principal-a"),
            session_id: SessionId::new("session-a"),
            agent_id: AgentId::new("agent-a"),
            provider_id: None,
            account_id: account,
            trace_id: None,
        }
    }

    fn sample_request_for(tenant: TenantId, account: Option<AccountId>) -> AcquireRequest {
        let mut req = sample_request(account);
        req.tenant_id = tenant;
        req
    }

    #[tokio::test]
    async fn acquire_then_release_decrements_active() {
        let pool = InMemoryCredentialPool::new(4);
        let account = AccountId::new("acct-1");

        let lease = pool
            .acquire(sample_request(Some(account.clone())))
            .await
            .unwrap();
        assert_eq!(lease.tenant_id, TenantId::new("tenant-a"));
        assert_eq!(lease.account_id, account);
        assert_eq!(pool.active_count(&account), 1);
        assert_eq!(pool.active_count(&AccountId::new("acct-other")), 0);

        let receipt = pool
            .release(lease.lease_id.clone(), LeaseOutcome::Completed)
            .await
            .expect("release 成功");
        assert!(!receipt.already_released);
        assert_eq!(receipt.lease_id, lease.lease_id);
        assert_eq!(receipt.outcome, LeaseOutcome::Completed);
        assert_eq!(pool.active_count(&account), 0);
    }

    #[tokio::test]
    async fn concurrency_exhaustion_blocks_and_keeps_count() {
        let pool = InMemoryCredentialPool::new(1);
        let account = AccountId::new("acct-1");

        let first = pool
            .acquire(sample_request(Some(account.clone())))
            .await
            .unwrap();
        assert_eq!(pool.active_count(&account), 1);

        // 超出上限：返回 ConcurrencyExhausted，活跃计数保持在上限。
        let err = pool
            .acquire(sample_request(Some(account.clone())))
            .await
            .unwrap_err();
        match err {
            PoolError::ConcurrencyExhausted {
                account: a,
                active,
                max,
            } => {
                assert_eq!(a, account);
                assert_eq!(active, 1);
                assert_eq!(max, 1);
            }
            other => panic!("expected ConcurrencyExhausted, got {other:?}"),
        }
        assert_eq!(pool.active_count(&account), 1);

        // 其它账号不受影响。
        let other_account = AccountId::new("acct-2");
        let second = pool
            .acquire(sample_request(Some(other_account.clone())))
            .await
            .unwrap();
        assert_eq!(pool.active_count(&other_account), 1);

        pool.release(first.lease_id.clone(), LeaseOutcome::Completed)
            .await
            .expect("release 成功");
        pool.release(second.lease_id.clone(), LeaseOutcome::Completed)
            .await
            .expect("release 成功");
        assert_eq!(pool.active_count(&account), 0);
        assert_eq!(pool.active_count(&other_account), 0);
    }

    #[tokio::test]
    async fn double_release_is_idempotent() {
        let pool = InMemoryCredentialPool::new(2);
        let account = AccountId::new("acct-1");
        let lease = pool
            .acquire(sample_request(Some(account.clone())))
            .await
            .unwrap();

        let first = pool
            .release(lease.lease_id.clone(), LeaseOutcome::Completed)
            .await
            .expect("release 成功");
        assert!(!first.already_released);
        assert_eq!(pool.active_count(&account), 0);

        // 第二次释放：未知 lease，幂等返回，不重复计数、不惩罚健康。
        let second = pool
            .release(lease.lease_id.clone(), LeaseOutcome::Failed)
            .await
            .expect("release 成功");
        assert!(second.already_released);
        assert_eq!(second.outcome, LeaseOutcome::Failed);
        assert_eq!(pool.active_count(&account), 0);
        assert_eq!(pool.account_health(&account).consecutive_failures, 0);
    }

    #[tokio::test]
    async fn cancelled_does_not_penalize_health() {
        let pool = InMemoryCredentialPool::new(2);
        let account = AccountId::new("acct-1");
        let lease = pool
            .acquire(sample_request(Some(account.clone())))
            .await
            .unwrap();

        let receipt = pool
            .release(lease.lease_id.clone(), LeaseOutcome::Cancelled)
            .await
            .expect("release 成功");
        assert!(!receipt.already_released);

        let health = pool.account_health(&account);
        assert_eq!(health.active_leases, 0);
        assert_eq!(health.consecutive_failures, 0);
        assert_eq!(health.cancelled_count, 1);
    }

    #[tokio::test]
    async fn failed_increments_consecutive_failures() {
        let pool = InMemoryCredentialPool::new(2);
        let account = AccountId::new("acct-1");
        let lease = pool
            .acquire(sample_request(Some(account.clone())))
            .await
            .unwrap();

        pool.release(lease.lease_id.clone(), LeaseOutcome::Failed)
            .await
            .expect("release 成功");

        let health = pool.account_health(&account);
        assert_eq!(health.active_leases, 0);
        assert_eq!(health.consecutive_failures, 1);
        assert_eq!(health.cancelled_count, 0);
    }

    #[tokio::test]
    async fn guard_drop_releases_default_outcome_then_active_zero() {
        let pool = InMemoryCredentialPool::new(2);
        let account = AccountId::new("acct-1");

        {
            let guard = pool
                .acquire_guard(sample_request(Some(account.clone())))
                .await
                .unwrap();
            assert!(guard.lease().is_some());
            assert_eq!(pool.active_count(&account), 1);
        } // Drop：以 fail-safe 默认 outcome（Failed）同步释放。

        assert_eq!(pool.active_count(&account), 0);
        let health = pool.account_health(&account);
        assert_eq!(health.active_leases, 0);
        assert_eq!(health.consecutive_failures, 1);
        assert_eq!(health.cancelled_count, 0);
    }

    #[tokio::test]
    async fn lease_has_no_secret_field() {
        let pool = InMemoryCredentialPool::new(1);
        let lease = pool
            .acquire(sample_request(Some(AccountId::new("acct-1"))))
            .await
            .unwrap();

        // 编译期检查：穷尽解构全部 6 个字段；若新增或重命名字段则无法编译。
        let CredentialLease {
            lease_id,
            schema_version,
            credential_id,
            account_id,
            provider_id,
            agent_id,
            session_id,
            principal_id,
            tenant_id,
            acquired_at_ms,
            expires_at_ms,
            version,
        } = &lease;
        assert_eq!(lease_id, &lease.lease_id);
        assert_eq!(schema_version, &lease.schema_version);
        assert_eq!(credential_id, &lease.credential_id);
        assert_eq!(account_id, &lease.account_id);
        assert_eq!(provider_id, &lease.provider_id);
        assert_eq!(agent_id, &lease.agent_id);
        assert_eq!(session_id, &lease.session_id);
        assert_eq!(principal_id, &lease.principal_id);
        assert_eq!(tenant_id, &lease.tenant_id);
        assert_eq!(acquired_at_ms, &lease.acquired_at_ms);
        assert_eq!(expires_at_ms, &lease.expires_at_ms);
        assert_eq!(version, &lease.version);

        // 运行时检查：序列化结果不得包含任何 secret 类字段。
        // `credential_id` 是 opaque 定位符（同 account_id），非 secret，允许存在。
        let json = serde_json::to_value(&lease).unwrap();
        let object = json
            .as_object()
            .expect("CredentialLease 序列化结果应为 JSON 对象");
        for forbidden in ["secret", "token", "api_key", "password", "secret_ref"] {
            assert!(
                !object.contains_key(forbidden),
                "CredentialLease 不得包含字段 `{forbidden}`"
            );
        }
        assert_eq!(object.len(), 12);
    }

    #[tokio::test]
    async fn acquire_guard_manual_release_active_zero() {
        let pool = InMemoryCredentialPool::new(2);
        let account = AccountId::new("acct-1");

        let mut guard = pool
            .acquire_guard(sample_request(Some(account.clone())))
            .await
            .unwrap();
        assert_eq!(pool.active_count(&account), 1);

        *guard.outcome_mut() = LeaseOutcome::Cancelled;

        // into_lease 取走 lease：Drop 不再产生释放副作用，需要手动释放。
        let lease = guard.into_lease().unwrap();
        assert_eq!(lease.account_id, account);
        assert_eq!(pool.active_count(&account), 1);

        let receipt = pool
            .release(lease.lease_id.clone(), LeaseOutcome::Cancelled)
            .await
            .expect("release 成功");
        assert!(!receipt.already_released);
        assert_eq!(pool.active_count(&account), 0);
        let health = pool.account_health(&account);
        assert_eq!(health.cancelled_count, 1);
        assert_eq!(health.consecutive_failures, 0);
    }

    // ===== P18-4 并发 / 过期 / 恢复测试 =====

    fn fixed_pool_with_ttl(account_max: u64, ttl_ms: u64) -> InMemoryCredentialPool {
        // 用 SystemClock 的池测试 TTL 会 flaky，这里换 FixedLeaseClock 但构造时注入一个
        // 永不过期的初始时间；各测试需要推进时间时直接重用同一固定时间断言。
        InMemoryCredentialPool::with_clock(
            PoolConfig::new(account_max).with_ttl_ms(ttl_ms),
            std::sync::Arc::new(FixedLeaseClock::new(Timestamp::from_unix_millis(1_000))),
        )
    }

    #[tokio::test]
    async fn lease_carries_credential_term_and_version() {
        let pool = InMemoryCredentialPool::new(2);
        let lease = pool
            .acquire(sample_request(Some(AccountId::new("acct-1"))))
            .await
            .unwrap();
        // 绑定 credential（legacy picker 默认 default），含期限与 version。
        assert_eq!(lease.credential_id, CredentialId::new("default"));
        assert_eq!(lease.schema_version, CONTROL_PLANE_SCHEMA_VERSION);
        assert_eq!(lease.version, 2);
        assert!(lease.expires_at_ms > lease.acquired_at_ms);
        assert_eq!(lease.principal_id.as_str(), "principal-a");
        // lease_state 可观测：Acquired。
        assert_eq!(
            pool.lease_state(&lease.lease_id),
            Some(LeaseState::Acquired)
        );
    }

    #[tokio::test]
    async fn per_tenant_cap_blocks_beyond_global_limit() {
        // per-tenant cap = 1，但两个账号各自 cap = 4；第二个账号 acquire 应被租户上限挡住。
        let pool = InMemoryCredentialPool::with_config(PoolConfig::new(4).with_tenant_cap(1));
        let req_a = sample_request(Some(AccountId::new("acct-1")));
        let req_b = sample_request(Some(AccountId::new("acct-2")));
        // 同 tenant（tenant-a）
        let _g = pool.acquire_guard(req_a).await.unwrap();
        let err = pool.acquire(req_b.clone()).await.unwrap_err();
        assert!(matches!(
            err,
            PoolError::TenantConcurrencyExhausted {
                active: 1,
                max: 1,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn concurrent_acquire_never_exceeds_account_cap() {
        // 多任务并发 acquire 同一账号：成功数 <= cap，其余得 ConcurrencyExhausted。
        let pool = std::sync::Arc::new(InMemoryCredentialPool::new(4));
        let account = AccountId::new("acct-contended");
        // 收集 guard 到共享集合，使其存活到计数之后（否则 task 返回即 Drop 释放 slot）。
        let guards = std::sync::Arc::new(tokio::sync::Mutex::new(Vec::<LeaseGuard>::new()));

        let mut handles = Vec::new();
        for _ in 0..64 {
            let pool = pool.clone();
            let guards = guards.clone();
            let req = sample_request(Some(account.clone()));
            handles.push(tokio::spawn(async move {
                match pool.acquire_guard(req).await {
                    Ok(guard) => {
                        guards.lock().await.push(guard);
                        true
                    }
                    Err(_) => false,
                }
            }));
        }
        let mut acquired = 0usize;
        for handle in handles {
            if handle.await.unwrap() {
                acquired += 1;
            }
        }
        assert_eq!(acquired, 4, "exactly the cap may be active");
        assert_eq!(pool.active_count(&account), 4);
        // 显式 drop guards，验证 active 归零。
        drop(guards);
        assert_eq!(pool.active_count(&account), 0);
    }

    #[tokio::test]
    async fn concurrent_release_is_idempotent_and_never_undershoots() {
        // 持有 N 个 lease 的并发 release，加上重复 release，最终 active 必为 0。
        let pool = std::sync::Arc::new(InMemoryCredentialPool::new(8));
        let account = AccountId::new("acct-rel");

        let mut lease_ids = Vec::new();
        for _ in 0..8 {
            let lease = pool
                .acquire(sample_request(Some(account.clone())))
                .await
                .unwrap();
            lease_ids.push(lease.lease_id);
        }
        assert_eq!(pool.active_count(&account), 8);

        let mut handles = Vec::new();
        // 每个 lease 释放 3 次（含重复）：幂等，仅首次真正归还额度。
        for id in lease_ids.iter().take(8) {
            for _ in 0..3 {
                let pool = pool.clone();
                let id = id.clone();
                handles.push(tokio::spawn(async move {
                    pool.release(id, LeaseOutcome::Completed).await
                }));
            }
        }
        let mut first_released = 0;
        let mut already_released = 0;
        for handle in handles {
            let receipt = handle.await.unwrap().expect("release 成功");
            if receipt.already_released {
                already_released += 1;
            } else {
                first_released += 1;
            }
        }
        assert_eq!(first_released, 8);
        assert_eq!(already_released, 8 * 3 - 8);
        assert_eq!(pool.active_count(&account), 0);
        // 健康未被重复 release 累加（Failed 才累加失败计数，这里全 Completed）。
        let health = pool.account_health(&account);
        assert_eq!(health.active_leases, 0);
        assert_eq!(health.consecutive_failures, 0);
    }

    #[tokio::test]
    async fn cancelled_release_does_not_block_admission_after_recovery() {
        // 取消（Cancelled）归还额度，下一次 acquire 立即可成功。
        let pool = fixed_pool_with_ttl(1, 60_000);
        let account = AccountId::new("acct-cancel");
        let lease = pool
            .acquire(sample_request(Some(account.clone())))
            .await
            .unwrap();
        let receipt = pool
            .release(lease.lease_id, LeaseOutcome::Cancelled)
            .await
            .expect("release 成功");
        assert!(!receipt.already_released);
        // 立即重新 acquire 成功。
        let lease2 = pool
            .acquire(sample_request(Some(account.clone())))
            .await
            .unwrap();
        assert_eq!(pool.active_count(&account), 1);
        // Cancelled 不惩罚健康。
        let health = pool.account_health(&account);
        assert_eq!(health.consecutive_failures, 0);
        assert_eq!(health.cancelled_count, 1);
        drop(lease2);
    }

    #[tokio::test]
    async fn reclaim_expired_frees_orphans_and_advances_to_reclaimed() {
        // TTL 极短：acquire 后推进时钟，reclaim_expired 应过期并回收，无永久泄漏。
        let clock = std::sync::Arc::new(FixedLeaseClock::new(Timestamp::from_unix_millis(1_000)));
        let pool =
            InMemoryCredentialPool::with_clock(PoolConfig::new(2).with_ttl_ms(100), clock.clone());
        let account = AccountId::new("acct-ttl");
        let lease = pool
            .acquire(sample_request(Some(account.clone())))
            .await
            .unwrap();
        assert_eq!(pool.active_count(&account), 1);

        // 推进时钟越过 TTL（构造一个新池替换 clock 比较麻烦；这里直接调用 recover
        // 风格的懒过期：把同一池的 lease 视为过期需要时钟前进——
        // 我们换用一个时钟已前进的新池，把旧 lease 的 record 当作恢复输入）。
        let advanced_clock =
            std::sync::Arc::new(FixedLeaseClock::new(Timestamp::from_unix_millis(2_000)));
        let advanced_pool = InMemoryCredentialPool::with_clock(
            PoolConfig::new(2).with_ttl_ms(100),
            advanced_clock.clone(),
        );
        // 从原池取出 record，注入到新池作为崩溃恢复输入。
        let record = {
            let state = lock(&pool.inner);
            state
                .leases
                .get(&lease.lease_id)
                .cloned()
                .expect("lease present")
        };
        let report = advanced_pool.recover_records(vec![record]);
        assert_eq!(report.expired, 1);
        assert_eq!(report.reclaimed, 1);
        // 新池没把孤儿计入 active。
        assert_eq!(advanced_pool.active_count(&account), 0);
    }

    #[tokio::test]
    async fn recover_rebuilds_live_leases_and_settles_orphans() {
        // 投影里有 3 个 lease：1 个仍存活（未过 TTL）、1 个过 TTL、1 个 Released。
        // restore 后：存活 lease 计入 active；另外 2 个被 settle。
        let projection = std::sync::Arc::new(InMemoryLeaseProjection::new());
        let clock = std::sync::Arc::new(FixedLeaseClock::new(Timestamp::from_unix_millis(5_000)));

        // 用 clock=5_000 构造 lease（TTL=10_000，存活；TTL=1_000，过期）。
        let req = sample_request(Some(AccountId::new("acct-rec")));
        let (live, _, _) = LeaseRecord::open(
            &req,
            LeaseId::new("live"),
            CredentialId::new("c"),
            &*clock,
            10_000,
        );
        let (stale, _, _) = LeaseRecord::open(
            &req,
            LeaseId::new("stale"),
            CredentialId::new("c"),
            &*clock,
            1_000,
        );
        let (released, _, _) = LeaseRecord::open(
            &req,
            LeaseId::new("released"),
            CredentialId::new("c"),
            &*clock,
            10_000,
        );
        let (released, _) = released.release(LeaseOutcome::Completed, &*clock).unwrap();
        projection.apply(&live, &[]).await.unwrap();
        projection.apply(&stale, &[]).await.unwrap();
        projection.apply(&released, &[]).await.unwrap();
        assert_eq!(projection.len(), 3);

        // 新池时钟前进到 6_000：live 未过 TTL（5_000+10_000=15_000 > 6_000），
        // stale 已过 TTL（5_000+1_000=6_000，> 严格大于才过期）→ 用 6_001。
        let advanced =
            std::sync::Arc::new(FixedLeaseClock::new(Timestamp::from_unix_millis(6_001)));
        let pool = InMemoryCredentialPool::build(
            PoolConfig::new(8),
            advanced.clone(),
            projection.clone(),
            std::sync::Arc::new(LegacyCredentialPicker),
        );
        let report = pool.restore().await.expect("restore 成功");
        // stale 过期 + released 已释放 → 2 个 settle；live 重建 active。
        assert_eq!(report.expired, 1);
        assert_eq!(report.reclaimed, 2);
        assert_eq!(pool.active_count(&AccountId::new("acct-rec")), 1);
        assert_eq!(projection.len(), 1, "仅存活的 live 留在投影");
    }

    #[tokio::test]
    async fn double_acquire_release_cycle_no_leak() {
        // 反复 acquire/release 同一账号，active 计数严格回到 0，无泄漏。
        let pool = InMemoryCredentialPool::new(2);
        let account = AccountId::new("acct-cycle");
        for _ in 0..20 {
            let l1 = pool
                .acquire(sample_request(Some(account.clone())))
                .await
                .unwrap();
            let l2 = pool
                .acquire(sample_request(Some(account.clone())))
                .await
                .unwrap();
            assert_eq!(pool.active_count(&account), 2);
            pool.release(l1.lease_id, LeaseOutcome::Completed)
                .await
                .expect("release 成功");
            pool.release(l2.lease_id, LeaseOutcome::Completed)
                .await
                .expect("release 成功");
            assert_eq!(pool.active_count(&account), 0);
        }
        let health = pool.account_health(&account);
        assert_eq!(health.active_leases, 0);
        assert_eq!(health.consecutive_failures, 0);
    }

    // ===== P18-4 主审修复定向测试 =====

    /// #1：跨租户同名账号必须隔离——`(tenant, account)` 键控，互不挤占额度。
    #[tokio::test]
    async fn cross_tenant_same_account_name_is_isolated() {
        let pool = InMemoryCredentialPool::new(1);
        let account = AccountId::new("shared-acct");
        let tenant_a = TenantId::new("tenant-a");
        let tenant_b = TenantId::new("tenant-b");

        let _ga = pool
            .acquire_guard(sample_request_for(tenant_a.clone(), Some(account.clone())))
            .await
            .unwrap();
        // tenant-b 同名账号仍可 acquire（不被 tenant-a 占用）。
        let _gb = pool
            .acquire_guard(sample_request_for(tenant_b.clone(), Some(account.clone())))
            .await
            .unwrap();
        assert_eq!(pool.active_count_for(&tenant_a, &account), 1);
        assert_eq!(pool.active_count_for(&tenant_b, &account), 1);
        // tenant-a 自身第三次同名 acquire 被其 cap=1 挡住（隔离不等于无限）。
        let err = pool
            .acquire(sample_request_for(tenant_a.clone(), Some(account.clone())))
            .await
            .unwrap_err();
        assert!(matches!(err, PoolError::ConcurrencyExhausted { .. }));
    }

    /// #3：acquire 的事务化投影必须保存 snapshot 且事件不再被丢弃。
    #[tokio::test]
    async fn acquire_persists_snapshot_and_events_to_projection() {
        let projection = Arc::new(InMemoryLeaseProjection::new());
        let pool = InMemoryCredentialPool::with_projection(
            PoolConfig::new(2),
            Arc::new(SystemLeaseClock),
            projection.clone(),
        );
        let _lease = pool
            .acquire(sample_request(Some(AccountId::new("acct-ev"))))
            .await
            .unwrap();
        assert_eq!(projection.len(), 1, "snapshot 落库");
        assert_eq!(
            projection.event_count(),
            2,
            "Requested + Acquired 事件不再被丢弃"
        );
    }

    /// #6：重启后 lease_id 不得因 pid+counter 复用而碰撞。
    #[tokio::test]
    async fn lease_ids_do_not_collide_across_simulated_restart() {
        let pool_a = InMemoryCredentialPool::new(1);
        let pool_b = InMemoryCredentialPool::new(1);
        let id_a = pool_a
            .acquire(sample_request(Some(AccountId::new("a"))))
            .await
            .unwrap()
            .lease_id;
        let id_b = pool_b
            .acquire(sample_request(Some(AccountId::new("b"))))
            .await
            .unwrap()
            .lease_id;
        assert_ne!(id_a, id_b, "重启后 lease_id 不得碰撞");
        let id_a2 = pool_a
            .acquire(sample_request(Some(AccountId::new("a2"))))
            .await
            .unwrap()
            .lease_id;
        assert_ne!(id_a, id_a2);
    }

    /// 可推进时钟（测试用），驱动 reclaim_expired / 恢复的 TTL 判定。
    struct MutableClock(std::sync::Arc<std::sync::atomic::AtomicU64>);
    impl LeaseClock for MutableClock {
        fn now(&self) -> Timestamp {
            Timestamp::from_unix_millis(self.0.load(std::sync::atomic::Ordering::Relaxed))
        }
    }

    /// #2 + #3：expiry 闭区间（now >= expires），且 reclaim_expired 异步持久化终态事件。
    #[tokio::test]
    async fn reclaim_expired_async_persists_terminal_events() {
        let now_ms = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(1_000));
        let clock = Arc::new(MutableClock(now_ms.clone()));
        let projection = Arc::new(InMemoryLeaseProjection::new());
        let pool = InMemoryCredentialPool::build(
            PoolConfig::new(2).with_ttl_ms(100),
            clock,
            projection.clone(),
            Arc::new(LegacyCredentialPicker),
        );
        let account = AccountId::new("acct-reclaim");
        let _lease = pool
            .acquire(sample_request(Some(account.clone())))
            .await
            .unwrap();
        assert_eq!(pool.active_count(&account), 1);
        assert_eq!(projection.event_count(), 2);
        // 前进时钟到恰好 expires（1_000 + 100 = 1_100）：闭区间 → 视为过期。
        now_ms.store(1_100, std::sync::atomic::Ordering::Relaxed);
        let report = pool.reclaim_expired().await.expect("reclaim 成功");
        assert_eq!(report.expired, 1);
        assert_eq!(report.reclaimed, 1);
        assert_eq!(pool.active_count(&account), 0, "过期 lease 额度已归还");
        // 事件累计：Requested + Acquired (2) + Expired + Reclaimed (2) = 4。
        assert_eq!(projection.event_count(), 4, "终态事件被事务化持久化");
        assert_eq!(projection.len(), 0, "终态 snapshot 移出 outstanding");
    }

    /// #5：投影在 `.await` 挂起时，Drop 必须经 detached task 可靠完成释放，杜绝额度泄漏。
    #[tokio::test]
    async fn drop_release_completes_via_detached_task_when_projection_yields() {
        use std::sync::atomic::AtomicUsize;
        struct YieldingProjection {
            apply_calls: std::sync::Arc<AtomicUsize>,
        }
        #[async_trait]
        impl LeaseProjection for YieldingProjection {
            async fn apply(
                &self,
                _: &LeaseRecord,
                _: &[LeaseEvent],
            ) -> Result<(), LeaseProjectionError> {
                self.apply_calls
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                tokio::task::yield_now().await;
                Ok(())
            }
            async fn settle(&self, _: &LeaseId) -> Result<(), LeaseProjectionError> {
                Ok(())
            }
            async fn load_outstanding(&self) -> Result<Vec<LeaseRecord>, LeaseProjectionError> {
                Ok(Vec::new())
            }
        }
        let apply_calls = std::sync::Arc::new(AtomicUsize::new(0));
        let projection: Arc<dyn LeaseProjection> = Arc::new(YieldingProjection {
            apply_calls: apply_calls.clone(),
        });
        let pool = InMemoryCredentialPool::build(
            PoolConfig::new(2),
            Arc::new(SystemLeaseClock),
            projection,
            Arc::new(LegacyCredentialPicker),
        );
        let account = AccountId::new("acct-yield");
        let tenant = TenantId::new("tenant-a");
        {
            let _guard = pool
                .acquire_guard(sample_request(Some(account.clone())))
                .await
                .unwrap();
            assert_eq!(pool.active_count_for(&tenant, &account), 1);
        } // guard Drop：release 内 projection.apply yield → 不能同步完成 → detached。
        for _ in 0..100 {
            tokio::task::yield_now().await;
            if pool.active_count_for(&tenant, &account) == 0 {
                break;
            }
        }
        assert_eq!(
            pool.active_count_for(&tenant, &account),
            0,
            "detached release 必须最终归还额度，禁止永久泄漏"
        );
        assert!(
            apply_calls.load(std::sync::atomic::Ordering::Relaxed) >= 2,
            "acquire + release 两次 apply 都应被调用"
        );
    }

    // ===== P18-4 最后 crash-consistency 修复：durable-first + 并发串行化 =====

    /// 可控失败的投影包装：默认代理到 `InMemoryLeaseProjection`，`fail_apply` 打开时
    /// 所有 `apply` 返回 `Backend` 错误（模拟 DatabaseActor 持久化失败）。
    #[derive(Clone)]
    struct FailingProjection {
        inner: Arc<InMemoryLeaseProjection>,
        fail_apply: Arc<std::sync::atomic::AtomicBool>,
    }

    impl FailingProjection {
        fn new() -> Self {
            Self {
                inner: Arc::new(InMemoryLeaseProjection::new()),
                fail_apply: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            }
        }

        fn set_fail(&self, fail: bool) {
            self.fail_apply
                .store(fail, std::sync::atomic::Ordering::Relaxed);
        }
    }

    #[async_trait]
    impl LeaseProjection for FailingProjection {
        async fn apply(
            &self,
            snapshot: &LeaseRecord,
            events: &[LeaseEvent],
        ) -> Result<(), LeaseProjectionError> {
            if self.fail_apply.load(std::sync::atomic::Ordering::Relaxed) {
                return Err(LeaseProjectionError::Backend(
                    "injected apply failure".into(),
                ));
            }
            self.inner.apply(snapshot, events).await
        }

        async fn settle(&self, lease_id: &LeaseId) -> Result<(), LeaseProjectionError> {
            self.inner.settle(lease_id).await
        }

        async fn load_outstanding(&self) -> Result<Vec<LeaseRecord>, LeaseProjectionError> {
            self.inner.load_outstanding().await
        }
    }

    /// 每次 `apply` 先 yield 一次的投影（制造真实并发交错，事件序列仍须 canonical）。
    struct YieldingProjection(Arc<InMemoryLeaseProjection>);

    #[async_trait]
    impl LeaseProjection for YieldingProjection {
        async fn apply(
            &self,
            snapshot: &LeaseRecord,
            events: &[LeaseEvent],
        ) -> Result<(), LeaseProjectionError> {
            tokio::task::yield_now().await;
            self.0.apply(snapshot, events).await
        }

        async fn settle(&self, lease_id: &LeaseId) -> Result<(), LeaseProjectionError> {
            self.0.settle(lease_id).await
        }

        async fn load_outstanding(&self) -> Result<Vec<LeaseRecord>, LeaseProjectionError> {
            self.0.load_outstanding().await
        }
    }

    /// release 投影失败：显式 Err、内存/durable 均保持 Acquired、额度不变；
    /// 恢复后重试成功且 Released 事件恰好一条（不重复）。
    #[tokio::test]
    async fn release_projection_failure_keeps_acquired_and_retry_succeeds() {
        let projection = FailingProjection::new();
        let pool = InMemoryCredentialPool::with_projection(
            PoolConfig::new(2),
            Arc::new(SystemLeaseClock),
            Arc::new(projection.clone()),
        );
        let account = AccountId::new("acct-rel-fail");
        let lease = pool
            .acquire(sample_request(Some(account.clone())))
            .await
            .unwrap();
        assert_eq!(pool.active_count(&account), 1);

        // durable-first：投影失败 → release 显式 Err，状态与额度不变。
        projection.set_fail(true);
        let err = pool
            .release(lease.lease_id.clone(), LeaseOutcome::Completed)
            .await
            .unwrap_err();
        assert!(
            matches!(err, PoolError::Projection(_)),
            "投影失败必须显式传播，不得假成功"
        );
        assert_eq!(pool.active_count(&account), 1, "投影失败不得归还额度");
        assert_eq!(
            pool.lease_state(&lease.lease_id),
            Some(LeaseState::Acquired),
            "投影失败内存必须保持 Acquired"
        );
        assert_eq!(
            projection.inner.event_count(),
            2,
            "失败路径不得产生 Released 事件"
        );

        // 恢复后重试：成功，Released 事件恰好一条。
        projection.set_fail(false);
        let receipt = pool
            .release(lease.lease_id.clone(), LeaseOutcome::Completed)
            .await
            .unwrap();
        assert!(!receipt.already_released);
        assert_eq!(pool.active_count(&account), 0);
        assert_eq!(
            projection.inner.event_count(),
            3,
            "Released 事件恰好一条（并发 double-release 也不重复）"
        );
    }

    /// reclaim_expired 投影失败：显式 Err、内存仍 Acquired、额度未归还；
    /// 恢复后同一次回收成功。
    #[tokio::test]
    async fn reclaim_expired_projection_failure_keeps_memory_consistent() {
        let now_ms = Arc::new(std::sync::atomic::AtomicU64::new(1_000));
        let clock = Arc::new(MutableClock(now_ms.clone()));
        let projection = FailingProjection::new();
        let pool = InMemoryCredentialPool::build(
            PoolConfig::new(2).with_ttl_ms(100),
            clock,
            Arc::new(projection.clone()),
            Arc::new(LegacyCredentialPicker),
        );
        let account = AccountId::new("acct-reclaim-fail");
        let _lease = pool
            .acquire(sample_request(Some(account.clone())))
            .await
            .unwrap();
        now_ms.store(1_100, std::sync::atomic::Ordering::Relaxed);

        projection.set_fail(true);
        let err = pool.reclaim_expired().await.unwrap_err();
        assert!(matches!(err, PoolError::Projection(_)));
        assert_eq!(pool.active_count(&account), 1, "durable-first：额度未归还");
        assert_eq!(
            projection.inner.event_count(),
            2,
            "无 Expired/Reclaimed 事件"
        );

        projection.set_fail(false);
        let report = pool.reclaim_expired().await.expect("reclaim 成功");
        assert_eq!(report.expired, 1);
        assert_eq!(report.reclaimed, 1);
        assert_eq!(pool.active_count(&account), 0);
        assert_eq!(projection.inner.event_count(), 4);
        assert_eq!(projection.inner.len(), 0);
    }

    /// 重启一致性 #1：release 投影失败后崩溃——durable 仍是 Acquired，重启恢复时
    /// 一致重建 active（内存与 durable 从未分歧，无 split-brain）。
    #[tokio::test]
    async fn failed_release_then_restart_recovers_acquired_consistently() {
        let projection = FailingProjection::new();
        let pool1 = InMemoryCredentialPool::with_projection(
            PoolConfig::new(2),
            Arc::new(SystemLeaseClock),
            Arc::new(projection.clone()),
        );
        let account = AccountId::new("acct-restart-fail");
        let lease = pool1
            .acquire(sample_request(Some(account.clone())))
            .await
            .unwrap();
        projection.set_fail(true);
        let err = pool1
            .release(lease.lease_id.clone(), LeaseOutcome::Completed)
            .await
            .unwrap_err();
        assert!(matches!(err, PoolError::Projection(_)));
        projection.set_fail(false);
        drop(pool1); // 模拟崩溃：内存丢失，durable 保留 Acquired snapshot。

        let pool2 = InMemoryCredentialPool::with_projection(
            PoolConfig::new(2),
            Arc::new(SystemLeaseClock),
            Arc::new(projection.clone()),
        );
        let report = pool2.restore().await.expect("restore 成功");
        assert_eq!(report.expired, 0, "durable 仍 Acquired，无孤儿可回收");
        assert_eq!(report.reclaimed, 0);
        assert_eq!(
            pool2.active_count(&account),
            1,
            "durable Acquired 一致恢复（与失败时内存状态相同）"
        );
        assert_eq!(
            projection.inner.event_count(),
            2,
            "失败路径从未持久化 Released 事件"
        );
    }

    /// 重启一致性 #2：成功 release 后崩溃——durable 为 Released，重启 restore
    /// 直接回收，lease 绝不复活为 Acquired（无永久额度泄漏）。
    #[tokio::test]
    async fn successful_release_survives_restart_without_resurrection() {
        let projection = Arc::new(InMemoryLeaseProjection::new());
        let pool1 = InMemoryCredentialPool::with_projection(
            PoolConfig::new(2),
            Arc::new(SystemLeaseClock),
            projection.clone(),
        );
        let account = AccountId::new("acct-restart-ok");
        let lease = pool1
            .acquire(sample_request(Some(account.clone())))
            .await
            .unwrap();
        pool1
            .release(lease.lease_id.clone(), LeaseOutcome::Completed)
            .await
            .unwrap();
        assert_eq!(pool1.active_count(&account), 0);
        drop(pool1); // 模拟崩溃。

        let pool2 = InMemoryCredentialPool::with_projection(
            PoolConfig::new(2),
            Arc::new(SystemLeaseClock),
            projection.clone(),
        );
        let report = pool2.restore().await.expect("restore 成功");
        assert_eq!(report.expired, 0);
        assert_eq!(report.reclaimed, 1, "Released lease 在重启后被回收");
        assert_eq!(
            pool2.active_count(&account),
            0,
            "成功释放的 lease 不得在重启后复活占用额度"
        );
        assert_eq!(
            projection.event_count(),
            4,
            "Requested+Acquired+Released+Reclaimed（restore 回收时追加终态事件）"
        );
        assert_eq!(projection.len(), 0, "Reclaimed 终态快照移出 outstanding");
    }

    /// 并发 double-release：16 个并发 release（含交错 yield）只产生一条 Released
    /// 事件、恰好一次真正归还额度，其余幂等 already_released。
    #[tokio::test]
    async fn concurrent_double_release_emits_single_event() {
        let projection = Arc::new(InMemoryLeaseProjection::new());
        let pool = Arc::new(InMemoryCredentialPool::build(
            PoolConfig::new(8),
            Arc::new(SystemLeaseClock),
            Arc::new(YieldingProjection(projection.clone())),
            Arc::new(LegacyCredentialPicker),
        ));
        let account = AccountId::new("acct-double");
        let lease = pool
            .acquire(sample_request(Some(account.clone())))
            .await
            .unwrap();
        let lease_id = lease.lease_id.clone();

        let mut handles = Vec::new();
        for _ in 0..16 {
            let pool = pool.clone();
            let lease_id = lease_id.clone();
            handles.push(tokio::spawn(async move {
                pool.release(lease_id, LeaseOutcome::Completed)
                    .await
                    .expect("release 不应因并发而 Err")
            }));
        }
        let mut first = 0u32;
        let mut already = 0u32;
        for handle in handles {
            let receipt = handle.await.unwrap();
            if receipt.already_released {
                already += 1;
            } else {
                first += 1;
            }
        }
        assert_eq!(first, 1, "恰好一次真正释放");
        assert_eq!(already, 15);
        assert_eq!(pool.active_count(&account), 0, "额度最终归还，无泄漏");
        assert_eq!(
            projection.event_count(),
            3,
            "Requested+Acquired+Released：Released 事件不重复"
        );
    }

    /// 并发 release × reclaim_expired（过 TTL）：无论交错顺序，durable 事件序列
    /// 必须保持 canonical（version 严格递增、无 Released 出现在 Expired/Reclaimed
    /// 之后、无版本倒灌），最终无额度泄漏。
    #[tokio::test]
    async fn concurrent_release_and_reclaim_keep_canonical_order() {
        for _ in 0..20 {
            let now_ms = Arc::new(std::sync::atomic::AtomicU64::new(1_000));
            let clock = Arc::new(MutableClock(now_ms.clone()));
            let projection = Arc::new(InMemoryLeaseProjection::new());
            let pool = Arc::new(InMemoryCredentialPool::build(
                PoolConfig::new(4).with_ttl_ms(100),
                clock,
                projection.clone(),
                Arc::new(LegacyCredentialPicker),
            ));
            let account = AccountId::new("acct-race");
            let lease = pool
                .acquire(sample_request(Some(account.clone())))
                .await
                .unwrap();
            let lease_id = lease.lease_id.clone();
            now_ms.store(1_100, std::sync::atomic::Ordering::Relaxed); // 过 TTL。

            let rel_pool = pool.clone();
            let rec_pool = pool.clone();
            let rel =
                tokio::spawn(
                    async move { rel_pool.release(lease_id, LeaseOutcome::Completed).await },
                );
            let rec = tokio::spawn(async move { rec_pool.reclaim_expired().await });
            let rel_result = rel.await.unwrap();
            let rec_result = rec.await.unwrap();
            let _ = (rel_result, rec_result); // 两种结果（Ok / already_released / Err）都合法。

            // durable 事件序列必须 canonical：version 严格递增，kind 是
            // Requested → Acquired → (Released | Expired) → Reclaimed 的前缀路径。
            let events = projection.events();
            let lease_events: Vec<LeaseEvent> = events
                .into_iter()
                .filter(|event| event.lease_id() == &lease.lease_id)
                .collect();
            let mut previous_version = 0u64;
            let mut saw_released = false;
            let mut saw_expired = false;
            let mut saw_reclaimed = false;
            for event in &lease_events {
                let version = event.version();
                assert!(
                    version > previous_version,
                    "version 必须严格递增（无重复/倒灌事件）: {lease_events:?}"
                );
                previous_version = version;
                match event {
                    LeaseEvent::Released { .. } => {
                        assert!(!saw_expired && !saw_released && !saw_reclaimed);
                        saw_released = true;
                    }
                    LeaseEvent::Expired { .. } => {
                        assert!(!saw_released && !saw_expired && !saw_reclaimed);
                        saw_expired = true;
                    }
                    LeaseEvent::Reclaimed { .. } => {
                        assert!(
                            saw_released || saw_expired,
                            "Reclaimed 只能跟在 Released/Expired 之后"
                        );
                        assert!(!saw_reclaimed);
                        saw_reclaimed = true;
                    }
                    LeaseEvent::Requested { .. } | LeaseEvent::Acquired { .. } => {
                        assert!(!saw_released && !saw_expired && !saw_reclaimed);
                    }
                }
            }
            assert!(previous_version >= 3, "至少经历一次终态转换");
            assert_eq!(
                pool.active_count(&account),
                0,
                "并发 race 后额度必须归零（无泄漏）"
            );
            assert_eq!(projection.len(), 0, "终态 lease 已移出 outstanding");
        }
    }
}
