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

/// 类型安全的账号标识（`agent-domain` 尚无 `AccountId`，此处本地定义）。
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AccountId(String);

impl AccountId {
    /// 从任意可转换为 `String` 的值构造。
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// 返回内部字符串的借用视图。
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for AccountId {
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
    /// 被占用的账号。
    pub account_id: AccountId,
    /// 使用的 Provider。
    pub provider_id: ProviderId,
    /// 持有该 lease 的 Agent。
    pub agent_id: AgentId,
    /// 持有该 lease 的会话。
    pub session_id: SessionId,
    /// 所属租户。
    pub tenant_id: TenantId,
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
    /// 租户被拒绝。
    #[error("tenant denied: {reason}")]
    TenantDenied {
        /// 拒绝原因。
        reason: String,
    },
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
/// 实现约定：`release` 的 async 体不得包含真实挂起点（须单次 poll 即完成），
/// 以保证 [`LeaseGuard`] 的 `Drop` 路径可以同步完成释放。
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
    async fn release(&self, lease_id: LeaseId, outcome: LeaseOutcome) -> ReleaseReceipt;

    /// 账号当前活跃 lease 数；账号不存在时返回 0。
    fn active_count(&self, account: &AccountId) -> u64;

    /// 账号健康状态；账号不存在时返回 [`AccountHealth::default`]。
    fn account_health(&self, account: &AccountId) -> AccountHealth;
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
        // 契约要求 release 的 async 体同步完成（无真实挂起点），
        // 因此在 Drop 中直接单次 poll 即可完成释放。
        let mut future = self.pool.release(lease_id.clone(), outcome);
        match poll_now(&mut future) {
            Some(receipt) => {
                tracing::trace!(
                    lease_id = %receipt.lease_id,
                    already_released = receipt.already_released,
                    "lease released on guard drop"
                );
            }
            None => {
                tracing::warn!(
                    lease_id = %lease_id,
                    "release did not complete synchronously on guard drop; pool impl violates contract"
                );
            }
        }
    }
}

/// 永不唤醒的空 waker，用于在 `Drop` 中单次同步 poll 无挂起点的 future。
/// 用空 waker 对 `Unpin` future 单次 poll；立即完成则返回输出，否则返回 `None`。
fn poll_now<F: std::future::Future + Unpin>(future: &mut F) -> Option<F::Output> {
    // `Waker::noop()` 自 Rust 1.85 稳定（= workspace MSRV），替代手写 NoopWaker。
    let waker = std::task::Waker::noop();
    let mut context = std::task::Context::from_waker(waker);
    match Pin::new(future).poll(&mut context) {
        std::task::Poll::Ready(output) => Some(output),
        std::task::Poll::Pending => None,
    }
}

/// 进程内默认凭据池：按账号限制并发，释放幂等，取消不惩罚健康。
///
/// 每个账号的状态位于 `std::sync::Mutex` 之后，锁绝不在 `.await` 上保持。
#[derive(Clone)]
pub struct InMemoryCredentialPool {
    inner: Arc<Mutex<PoolState>>,
    next_lease_id: Arc<AtomicU64>,
}

/// 池内部状态（单一互斥锁保护，临界区内无任何 await）。
struct PoolState {
    max_concurrency_per_account: u64,
    accounts: HashMap<AccountId, AccountState>,
    leases: HashMap<LeaseId, AccountId>,
}

/// 单个账号的运行时状态。
#[derive(Default)]
struct AccountState {
    active: u64,
    consecutive_failures: u64,
    cancelled_count: u64,
}

impl InMemoryCredentialPool {
    /// 创建按账号并发上限为 `max_concurrency_per_account` 的内存池。
    pub fn new(max_concurrency_per_account: u64) -> Self {
        Self {
            inner: Arc::new(Mutex::new(PoolState {
                max_concurrency_per_account,
                accounts: HashMap::new(),
                leases: HashMap::new(),
            })),
            next_lease_id: Arc::new(AtomicU64::new(0)),
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

        let mut state = self.inner.lock().unwrap();
        let max = state.max_concurrency_per_account;
        let account_state = state.accounts.entry(account.clone()).or_default();
        if account_state.active >= max {
            return Err(PoolError::ConcurrencyExhausted {
                account,
                active: account_state.active,
                max,
            });
        }
        account_state.active += 1;

        let lease_id = LeaseId::new(format!(
            "lease-{}-{}",
            std::process::id(),
            self.next_lease_id.fetch_add(1, Ordering::Relaxed)
        ));
        state.leases.insert(lease_id.clone(), account.clone());
        drop(state);

        tracing::trace!(lease_id = %lease_id, account = %account, "credential lease acquired");
        Ok(CredentialLease {
            lease_id,
            account_id: account,
            provider_id: provider,
            agent_id: req.agent_id,
            session_id: req.session_id,
            tenant_id: req.tenant_id,
        })
    }

    async fn acquire_guard(&self, req: AcquireRequest) -> Result<LeaseGuard, PoolError> {
        let lease = self.acquire(req).await?;
        Ok(LeaseGuard {
            lease: Some(lease),
            outcome: LeaseOutcome::Completed,
            pool: Arc::new(self.clone()),
        })
    }

    async fn release(&self, lease_id: LeaseId, outcome: LeaseOutcome) -> ReleaseReceipt {
        let mut state = self.inner.lock().unwrap();
        let Some(account_id) = state.leases.remove(&lease_id) else {
            tracing::trace!(lease_id = %lease_id, "release of unknown lease ignored (already released)");
            return ReleaseReceipt {
                lease_id,
                already_released: true,
                outcome,
            };
        };

        let account_state = state
            .accounts
            .get_mut(&account_id)
            .expect("lease account state must exist (created on acquire)");
        account_state.active = account_state.active.saturating_sub(1);
        match outcome {
            LeaseOutcome::Cancelled => account_state.cancelled_count += 1,
            LeaseOutcome::Failed => account_state.consecutive_failures += 1,
            LeaseOutcome::Completed | LeaseOutcome::Released => {}
        }

        tracing::trace!(lease_id = %lease_id, account = %account_id, ?outcome, "credential lease released");
        ReleaseReceipt {
            lease_id,
            already_released: false,
            outcome,
        }
    }

    fn active_count(&self, account: &AccountId) -> u64 {
        let state = self.inner.lock().unwrap();
        state
            .accounts
            .get(account)
            .map_or(0, |account_state| account_state.active)
    }

    fn account_health(&self, account: &AccountId) -> AccountHealth {
        let state = self.inner.lock().unwrap();
        state
            .accounts
            .get(account)
            .map_or_else(AccountHealth::default, |account_state| AccountHealth {
                active_leases: account_state.active,
                consecutive_failures: account_state.consecutive_failures,
                cancelled_count: account_state.cancelled_count,
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
            .await;
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
            .await;
        pool.release(second.lease_id.clone(), LeaseOutcome::Completed)
            .await;
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
            .await;
        assert!(!first.already_released);
        assert_eq!(pool.active_count(&account), 0);

        // 第二次释放：未知 lease，幂等返回，不重复计数、不惩罚健康。
        let second = pool
            .release(lease.lease_id.clone(), LeaseOutcome::Failed)
            .await;
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
            .await;
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
            .await;

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
        } // Drop：以默认 outcome（Completed）同步释放。

        assert_eq!(pool.active_count(&account), 0);
        let health = pool.account_health(&account);
        assert_eq!(health.active_leases, 0);
        assert_eq!(health.consecutive_failures, 0);
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
            account_id,
            provider_id,
            agent_id,
            session_id,
            tenant_id,
        } = &lease;
        assert_eq!(lease_id, &lease.lease_id);
        assert_eq!(account_id, &lease.account_id);
        assert_eq!(provider_id, &lease.provider_id);
        assert_eq!(agent_id, &lease.agent_id);
        assert_eq!(session_id, &lease.session_id);
        assert_eq!(tenant_id, &lease.tenant_id);

        // 运行时检查：序列化结果不得包含任何 secret 类字段。
        let json = serde_json::to_value(&lease).unwrap();
        let object = json
            .as_object()
            .expect("CredentialLease 序列化结果应为 JSON 对象");
        for forbidden in ["secret", "token", "api_key", "password", "credential"] {
            assert!(
                !object.contains_key(forbidden),
                "CredentialLease 不得包含字段 `{forbidden}`"
            );
        }
        assert_eq!(object.len(), 6);
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
            .await;
        assert!(!receipt.already_released);
        assert_eq!(pool.active_count(&account), 0);
        let health = pool.account_health(&account);
        assert_eq!(health.cancelled_count, 1);
        assert_eq!(health.consecutive_failures, 0);
    }
}
