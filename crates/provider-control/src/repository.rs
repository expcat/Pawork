//! Tenant-scoped ProviderAccount / Credential 仓库（P18-3，ADR-033）。
//!
//! 所有查询与变更都强制 `TenantId` 作用域：跨租户的 account/credential 查询返回
//! `None` / 空 / `NotFound`，绝不会泄漏或复用其它租户的记录。账号与凭据生命周期
//! （list / create / disable / delete / test）独立，返回脱敏视图（opaque ID +
//! masked status），供 CLI / GUI 安全管理面消费。
//!
//! [`ProviderAccountRepository::list_bindings`] 按 provider 列出绑定 account /
//! credential，是 `QuotaOverview` 批量聚合的**事实源**：无绑定时返回空、不做默认
//! provider 推测（P14-8 / ADR-033）。
//!
//! 本模块为 backend-agnostic trait + 进程内默认实现，不依赖数据库 / 网络 /
//! Secret 存储；SQLite 投影由宿主在组合层注入（与 `app-database` 控制面迁移
//! 对齐 schema_version）。

use std::collections::BTreeMap;
use std::sync::Mutex;

use agent_domain::{AccountId, CredentialId, ProviderId, TenantId, Timestamp};
use async_trait::async_trait;
use provider_api::CredentialKind as ProviderCredentialKind;
use thiserror::Error;

use crate::account::{
    AccountState, CredentialKind, CredentialMetadata, CredentialRecord, CredentialState,
    NotUsableReason, ProviderAccountRecord, RefreshState,
};
use crate::credential::{CredentialResolver, ResolveError};

/// 仓库错误。任何变体都不得携带明文 secret。
#[derive(Debug, Error)]
pub enum RepoError {
    /// 指定 tenant 内未找到 account / credential。
    #[error("not found in tenant {tenant}")]
    NotFound { tenant: TenantId },
    /// account / credential 已存在（同 tenant 内 ID 冲突）。
    #[error("already exists in tenant {tenant}")]
    AlreadyExists { tenant: TenantId },
    /// 记录的 tenant 与调用方 tenant 不一致（防御性，避免跨租户写入）。
    #[error("record tenant {record} does not match caller tenant {caller}")]
    TenantMismatch { record: TenantId, caller: TenantId },
    /// 凭据归属的 account 在同 tenant 内不存在（外键约束）。
    #[error("credential account {account} not found in tenant {tenant}")]
    AccountMissing {
        tenant: TenantId,
        account: AccountId,
    },
}

/// 账号脱敏摘要：仅 opaque ID + masked status，**无 secret_ref**。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderAccountSummary {
    pub tenant_id: TenantId,
    pub account_id: AccountId,
    pub provider_id: ProviderId,
    pub display_name: String,
    pub state: AccountState,
    pub priority: u32,
    pub weight: u32,
    pub max_concurrency: u64,
    pub routing_strategy: &'static str,
    pub credential_count: usize,
}

impl ProviderAccountSummary {
    /// 由 account record 与其凭据计数构造脱敏摘要。
    pub fn from_account(account: &ProviderAccountRecord, credential_count: usize) -> Self {
        Self {
            tenant_id: account.tenant_id.clone(),
            account_id: account.account_id.clone(),
            provider_id: account.provider_id.clone(),
            display_name: account.display_name.clone(),
            state: account.state,
            priority: account.priority,
            weight: account.weight,
            max_concurrency: account.max_concurrency,
            routing_strategy: account.routing_strategy.as_db_str(),
            credential_count,
        }
    }
}

/// 凭据脱敏摘要：opaque ID + 种类 + 状态 + 过期，**绝不暴露 `secret_ref`**
///（review 项：management list / binding 返回此类型，secret_ref 仅由内部
/// metadata lookup 在 resolver / factory 路径消费）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CredentialSummary {
    pub tenant_id: TenantId,
    pub credential_id: CredentialId,
    pub account_id: AccountId,
    pub provider_id: ProviderId,
    pub kind: CredentialKind,
    pub synthetic: bool,
    pub state: CredentialState,
    pub refresh_state: RefreshState,
    pub expires_at: Option<Timestamp>,
}

impl CredentialSummary {
    /// 由凭据元数据构造脱敏摘要（丢弃 `secret_ref`）。
    pub fn from_metadata(metadata: &CredentialMetadata) -> Self {
        Self {
            tenant_id: metadata.tenant_id.clone(),
            credential_id: metadata.credential_id.clone(),
            account_id: metadata.account_id.clone(),
            provider_id: metadata.provider_id.clone(),
            kind: metadata.kind,
            synthetic: metadata.synthetic,
            state: metadata.state,
            refresh_state: metadata.refresh_state,
            expires_at: metadata.expires_at,
        }
    }
}

/// `test_credential` 的脱敏结果（**绝不携带明文**）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CredentialTestStatus {
    /// 成功解析到短生命周期 credential（未保留明文）。
    Resolved,
    /// 凭据状态不可用（disabled / expired / revoked）。
    NotUsable(NotUsableReason),
    /// 凭据不存在（跨租户或已删除）。
    NotFound,
    /// `SecretRef` 无法解析（已删除 / 未回灌 / 合成 sentinel）。
    Unresolvable,
}

/// 按 provider 的 account / credential 绑定（binding 枚举事实源）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccountBinding {
    pub account: ProviderAccountRecord,
    /// 该 account 下属于同一 provider 的凭据脱敏摘要（含状态，**无 secret_ref / 明文**）。
    pub credentials: Vec<CredentialSummary>,
}

/// Tenant-scoped ProviderAccount / Credential 仓库。
///
/// 每个方法的第一参数都是 `tenant: &TenantId`，实现必须据此过滤，禁止返回或
/// 修改其它租户的记录。
#[async_trait]
pub trait ProviderAccountRepository: Send + Sync {
    /// 列出 tenant 内的 account 摘要（可按 provider 过滤）；脱敏，无 secret_ref。
    async fn list_accounts(
        &self,
        tenant: &TenantId,
        provider: Option<&ProviderId>,
    ) -> Vec<ProviderAccountSummary>;

    /// 取得单个 account 完整记录；跨租户或不存在返回 `None`。
    async fn get_account(
        &self,
        tenant: &TenantId,
        account_id: &AccountId,
    ) -> Option<ProviderAccountRecord>;

    /// 列出 account 下的凭据脱敏摘要（同 tenant 作用域；**无 secret_ref / 明文**，
    /// review 项：management list 返回 [`CredentialSummary`]）。
    async fn list_credential_metadata(
        &self,
        tenant: &TenantId,
        account_id: &AccountId,
    ) -> Vec<CredentialSummary>;

    /// 内部凭据元数据查询（同 tenant 作用域；含 `secret_ref`，**仅供 resolver /
    /// factory 路径消费，不得返回给 management / CLI / GUI**）。
    ///
    /// review 项：内部 metadata lookup 单独命名，与脱敏 [`list_credential_metadata`]
    /// 区分。
    async fn get_credential_metadata(
        &self,
        tenant: &TenantId,
        credential_id: &CredentialId,
    ) -> Option<CredentialMetadata>;

    /// 创建 account（review 项：显式 caller tenant，记录 tenant 不一致返回
    /// [`RepoError::TenantMismatch`]；ID 冲突返回 `AlreadyExists`）。
    async fn create_account(
        &self,
        caller_tenant: &TenantId,
        account: ProviderAccountRecord,
    ) -> Result<(), RepoError>;

    /// 创建 credential（显式 caller tenant，记录 tenant 不一致拒绝；归属 account
    /// 必须存在）。
    async fn create_credential(
        &self,
        caller_tenant: &TenantId,
        credential: CredentialMetadata,
    ) -> Result<(), RepoError>;

    /// 禁用 account（state → Disabled）；返回是否找到并修改。
    async fn disable_account(
        &self,
        tenant: &TenantId,
        account_id: &AccountId,
    ) -> Result<bool, RepoError>;

    /// 禁用 credential（state → Disabled）；返回是否找到并修改。
    async fn disable_credential(
        &self,
        tenant: &TenantId,
        credential_id: &CredentialId,
    ) -> Result<bool, RepoError>;

    /// 删除 account 及其下全部 credential（级联）；返回是否找到并删除。
    async fn delete_account(
        &self,
        tenant: &TenantId,
        account_id: &AccountId,
    ) -> Result<bool, RepoError>;

    /// 删除单个 credential；返回是否找到并删除。
    async fn delete_credential(
        &self,
        tenant: &TenantId,
        credential_id: &CredentialId,
    ) -> Result<bool, RepoError>;

    /// 测试 credential：经注入 resolver 解析，返回脱敏状态（**绝不返回明文**）。
    async fn test_credential(
        &self,
        tenant: &TenantId,
        credential_id: &CredentialId,
        resolver: &dyn CredentialResolver,
        now: Timestamp,
    ) -> Result<CredentialTestStatus, RepoError>;

    /// 按 provider 列出 account / credential 绑定（`QuotaOverview` 批量聚合事实源）。
    ///
    /// 无绑定时返回空 `Vec`，**不做默认 provider 推测**（ADR-033 / P14-8）。
    async fn list_bindings(&self, tenant: &TenantId, provider: &ProviderId) -> Vec<AccountBinding>;
}

/// 进程内默认仓库：`BTreeMap` 按 `(tenant, id)` 索引，强制 tenant scope。
pub struct InMemoryProviderAccountRepository {
    accounts: Mutex<BTreeMap<(TenantId, AccountId), ProviderAccountRecord>>,
    credentials: Mutex<BTreeMap<(TenantId, CredentialId), CredentialRecord>>,
}

impl Default for InMemoryProviderAccountRepository {
    fn default() -> Self {
        Self::new()
    }
}

impl InMemoryProviderAccountRepository {
    /// 创建空仓库。
    pub fn new() -> Self {
        Self {
            accounts: Mutex::new(BTreeMap::new()),
            credentials: Mutex::new(BTreeMap::new()),
        }
    }

    /// 以 legacy 合成默认 account / credential 预填（升级后行为不变）。
    pub fn with_legacy_default() -> Self {
        let repo = Self::new();
        let account = ProviderAccountRecord::legacy_synthetic_default();
        let credential = CredentialRecord::legacy_synthetic_default();
        {
            let mut accounts = repo.accounts.lock().expect("accounts mutex poisoned");
            accounts.insert(
                (account.tenant_id.clone(), account.account_id.clone()),
                account,
            );
        }
        {
            let mut credentials = repo.credentials.lock().expect("credentials mutex poisoned");
            credentials.insert(
                (
                    credential.tenant_id.clone(),
                    credential.credential_id.clone(),
                ),
                credential,
            );
        }
        repo
    }

    fn credential_count_locked(
        credentials: &BTreeMap<(TenantId, CredentialId), CredentialRecord>,
        tenant: &TenantId,
        account_id: &AccountId,
    ) -> usize {
        credentials
            .values()
            .filter(|credential| {
                &credential.tenant_id == tenant && &credential.account_id == account_id
            })
            .count()
    }
}

#[async_trait]
impl ProviderAccountRepository for InMemoryProviderAccountRepository {
    async fn list_accounts(
        &self,
        tenant: &TenantId,
        provider: Option<&ProviderId>,
    ) -> Vec<ProviderAccountSummary> {
        let accounts = self.accounts.lock().expect("accounts mutex poisoned");
        let credentials = self.credentials.lock().expect("credentials mutex poisoned");
        accounts
            .values()
            .filter(|account| &account.tenant_id == tenant)
            .filter(|account| {
                provider.is_none_or(|provider_id| &account.provider_id == provider_id)
            })
            .map(|account| {
                let count =
                    Self::credential_count_locked(&credentials, tenant, &account.account_id);
                ProviderAccountSummary::from_account(account, count)
            })
            .collect()
    }

    async fn get_account(
        &self,
        tenant: &TenantId,
        account_id: &AccountId,
    ) -> Option<ProviderAccountRecord> {
        let accounts = self.accounts.lock().expect("accounts mutex poisoned");
        accounts.get(&(tenant.clone(), account_id.clone())).cloned()
    }

    async fn list_credential_metadata(
        &self,
        tenant: &TenantId,
        account_id: &AccountId,
    ) -> Vec<CredentialSummary> {
        let credentials = self.credentials.lock().expect("credentials mutex poisoned");
        credentials
            .values()
            .filter(|credential| {
                &credential.tenant_id == tenant && &credential.account_id == account_id
            })
            .map(CredentialSummary::from_metadata)
            .collect()
    }

    async fn get_credential_metadata(
        &self,
        tenant: &TenantId,
        credential_id: &CredentialId,
    ) -> Option<CredentialMetadata> {
        let credentials = self.credentials.lock().expect("credentials mutex poisoned");
        credentials
            .get(&(tenant.clone(), credential_id.clone()))
            .cloned()
    }

    async fn create_account(
        &self,
        caller_tenant: &TenantId,
        account: ProviderAccountRecord,
    ) -> Result<(), RepoError> {
        // review 项：显式 caller tenant；记录 tenant 与 caller 不一致即拒绝（防御性，
        // 避免跨租户写入）。caller/record 来自两个独立输入，必须交叉校验。
        if &account.tenant_id != caller_tenant {
            return Err(RepoError::TenantMismatch {
                record: account.tenant_id.clone(),
                caller: caller_tenant.clone(),
            });
        }
        let key = (account.tenant_id.clone(), account.account_id.clone());
        let mut accounts = self.accounts.lock().expect("accounts mutex poisoned");
        if accounts.contains_key(&key) {
            return Err(RepoError::AlreadyExists {
                tenant: caller_tenant.clone(),
            });
        }
        accounts.insert(key, account);
        Ok(())
    }

    async fn create_credential(
        &self,
        caller_tenant: &TenantId,
        credential: CredentialMetadata,
    ) -> Result<(), RepoError> {
        // review 项：记录 tenant 与 caller 不一致即拒绝。
        if &credential.tenant_id != caller_tenant {
            return Err(RepoError::TenantMismatch {
                record: credential.tenant_id.clone(),
                caller: caller_tenant.clone(),
            });
        }
        // 归属 account 必须存在于同一 tenant。
        let accounts = self.accounts.lock().expect("accounts mutex poisoned");
        if !accounts.contains_key(&(credential.tenant_id.clone(), credential.account_id.clone())) {
            return Err(RepoError::AccountMissing {
                tenant: caller_tenant.clone(),
                account: credential.account_id.clone(),
            });
        }
        drop(accounts);

        let key = (
            credential.tenant_id.clone(),
            credential.credential_id.clone(),
        );
        let mut credentials = self.credentials.lock().expect("credentials mutex poisoned");
        if credentials.contains_key(&key) {
            return Err(RepoError::AlreadyExists {
                tenant: caller_tenant.clone(),
            });
        }
        credentials.insert(key, credential);
        Ok(())
    }

    async fn disable_account(
        &self,
        tenant: &TenantId,
        account_id: &AccountId,
    ) -> Result<bool, RepoError> {
        let mut accounts = self.accounts.lock().expect("accounts mutex poisoned");
        match accounts.get_mut(&(tenant.clone(), account_id.clone())) {
            Some(account) => {
                let changed = account.state != AccountState::Disabled;
                account.state = AccountState::Disabled;
                Ok(changed)
            }
            None => Err(RepoError::NotFound {
                tenant: tenant.clone(),
            }),
        }
    }

    async fn disable_credential(
        &self,
        tenant: &TenantId,
        credential_id: &CredentialId,
    ) -> Result<bool, RepoError> {
        let mut credentials = self.credentials.lock().expect("credentials mutex poisoned");
        match credentials.get_mut(&(tenant.clone(), credential_id.clone())) {
            Some(credential) => {
                let changed = credential.state != CredentialState::Disabled;
                credential.state = CredentialState::Disabled;
                Ok(changed)
            }
            None => Err(RepoError::NotFound {
                tenant: tenant.clone(),
            }),
        }
    }

    async fn delete_account(
        &self,
        tenant: &TenantId,
        account_id: &AccountId,
    ) -> Result<bool, RepoError> {
        let mut accounts = self.accounts.lock().expect("accounts mutex poisoned");
        let removed = accounts
            .remove(&(tenant.clone(), account_id.clone()))
            .is_some();
        if removed {
            // 级联删除同 tenant 下该 account 的全部 credential。
            let mut credentials = self.credentials.lock().expect("credentials mutex poisoned");
            credentials.retain(|(cred_tenant, _), credential| {
                !(cred_tenant == tenant && &credential.account_id == account_id)
            });
        }
        Ok(removed)
    }

    async fn delete_credential(
        &self,
        tenant: &TenantId,
        credential_id: &CredentialId,
    ) -> Result<bool, RepoError> {
        let mut credentials = self.credentials.lock().expect("credentials mutex poisoned");
        let removed = credentials
            .remove(&(tenant.clone(), credential_id.clone()))
            .is_some();
        Ok(removed)
    }

    async fn test_credential(
        &self,
        tenant: &TenantId,
        credential_id: &CredentialId,
        resolver: &dyn CredentialResolver,
        now: Timestamp,
    ) -> Result<CredentialTestStatus, RepoError> {
        // 在独立块内持锁并提取 owned 值，确保 MutexGuard 在 `.await` 之前释放
        //（async fn 必须 Send：临界区内不得跨 await 持有非 Send 的 guard）。
        // 复制整个 metadata 以使用 usable_for_take 闸门（review 项：fail-closed
        // 拒绝非 Active / refreshing|failed / expires_at 已过期）。
        let metadata = {
            let credentials = self.credentials.lock().expect("credentials mutex poisoned");
            let Some(credential) = credentials.get(&(tenant.clone(), credential_id.clone())) else {
                return Ok(CredentialTestStatus::NotFound);
            };
            credential.clone()
        };
        if let Err(reason) = metadata.usable_for_take(now) {
            return Ok(CredentialTestStatus::NotUsable(reason));
        }

        let kind = map_credential_kind(metadata.kind);
        match resolver.resolve(&metadata.secret_ref, kind).await {
            Ok(_) => Ok(CredentialTestStatus::Resolved),
            Err(ResolveError::NotFound) => Ok(CredentialTestStatus::Unresolvable),
            Err(ResolveError::Backend(_)) => Ok(CredentialTestStatus::Unresolvable),
        }
    }

    async fn list_bindings(&self, tenant: &TenantId, provider: &ProviderId) -> Vec<AccountBinding> {
        let accounts = self.accounts.lock().expect("accounts mutex poisoned");
        let credentials = self.credentials.lock().expect("credentials mutex poisoned");
        accounts
            .values()
            .filter(|account| &account.tenant_id == tenant && &account.provider_id == provider)
            .map(|account| {
                let bound: Vec<CredentialSummary> = credentials
                    .values()
                    .filter(|credential| {
                        &credential.tenant_id == tenant
                            && credential.account_id == account.account_id
                            && &credential.provider_id == provider
                    })
                    .map(CredentialSummary::from_metadata)
                    .collect();
                AccountBinding {
                    account: account.clone(),
                    credentials: bound,
                }
            })
            .collect()
    }
}

/// 把控制面 [`CredentialKind`] 映射到 provider-api 的 canonical 凭据种类。
fn map_credential_kind(kind: CredentialKind) -> ProviderCredentialKind {
    match kind {
        CredentialKind::ApiKey => ProviderCredentialKind::ApiKey,
        CredentialKind::OAuth => ProviderCredentialKind::OAuthBearer,
        CredentialKind::Other => ProviderCredentialKind::SessionToken,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::credential::InMemoryCredentialResolver;
    use agent_domain::PrincipalId;

    fn tenant_a() -> TenantId {
        TenantId::new("tenant-a")
    }
    fn tenant_b() -> TenantId {
        TenantId::new("tenant-b")
    }

    fn sample_account(tenant: &TenantId, account: &str, provider: &str) -> ProviderAccountRecord {
        ProviderAccountRecord {
            schema_version: crate::CONTROL_PLANE_SCHEMA_VERSION,
            tenant_id: tenant.clone(),
            account_id: AccountId::new(account),
            provider_id: ProviderId::new(provider),
            principal_id: PrincipalId::new("owner"),
            display_name: account.to_string(),
            routing_strategy: crate::routing::RoutingStrategy::Priority,
            priority: 1,
            weight: 2,
            max_concurrency: 4,
            state: AccountState::Active,
        }
    }

    fn sample_credential(
        tenant: &TenantId,
        account: &str,
        cred: &str,
        provider: &str,
    ) -> CredentialRecord {
        CredentialRecord {
            schema_version: crate::CONTROL_PLANE_SCHEMA_VERSION,
            tenant_id: tenant.clone(),
            credential_id: CredentialId::new(cred),
            account_id: AccountId::new(account),
            provider_id: ProviderId::new(provider),
            kind: CredentialKind::ApiKey,
            synthetic: false,
            secret_ref: crate::account::SecretRef::new(format!("pawork.{provider}"), cred),
            state: CredentialState::Active,
            expires_at: None,
            refresh_state: RefreshState::NotRefreshable,
        }
    }

    /// 在 `repo` 中创建 account，caller tenant = account 自身 tenant（测试便利）。
    async fn create_account(
        repo: &InMemoryProviderAccountRepository,
        account: ProviderAccountRecord,
    ) {
        let tenant = account.tenant_id.clone();
        repo.create_account(&tenant, account).await.unwrap();
    }

    /// 在 `repo` 中创建 credential，caller tenant = credential 自身 tenant。
    async fn create_credential(
        repo: &InMemoryProviderAccountRepository,
        credential: CredentialMetadata,
    ) {
        let tenant = credential.tenant_id.clone();
        repo.create_credential(&tenant, credential).await.unwrap();
    }

    #[tokio::test]
    async fn create_list_get_round_trip_within_tenant() {
        let repo = InMemoryProviderAccountRepository::new();
        let tenant = tenant_a();
        let account = sample_account(&tenant, "acct-1", "openai");
        create_account(&repo, account.clone()).await;
        create_credential(
            &repo,
            sample_credential(&tenant, "acct-1", "cred-1", "openai"),
        )
        .await;

        let summaries = repo.list_accounts(&tenant, None).await;
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].account_id, AccountId::new("acct-1"));
        assert_eq!(summaries[0].credential_count, 1);
        // 摘要脱敏：无 secret_ref 字段（编译期穷尽 ProviderAccountSummary 字段）。
        let ProviderAccountSummary {
            tenant_id,
            account_id,
            provider_id,
            display_name,
            state,
            priority,
            weight,
            max_concurrency,
            routing_strategy,
            credential_count,
        } = &summaries[0];
        assert_eq!(tenant_id, &tenant);
        assert_eq!(account_id, &AccountId::new("acct-1"));
        assert_eq!(provider_id.as_str(), "openai");
        assert_eq!(*state, AccountState::Active);
        assert_eq!(*priority, 1);
        assert_eq!(*weight, 2);
        assert_eq!(*max_concurrency, 4);
        assert_eq!(*routing_strategy, "priority");
        assert_eq!(*credential_count, 1);
        assert!(!display_name.contains("secret"));

        let fetched = repo.get_account(&tenant, &AccountId::new("acct-1")).await;
        assert_eq!(fetched, Some(account));
        let creds = repo
            .list_credential_metadata(&tenant, &AccountId::new("acct-1"))
            .await;
        assert_eq!(creds.len(), 1);
    }

    #[tokio::test]
    async fn cross_tenant_queries_are_isolated() {
        let repo = InMemoryProviderAccountRepository::new();
        create_account(&repo, sample_account(&tenant_a(), "acct-1", "openai")).await;
        create_credential(
            &repo,
            sample_credential(&tenant_a(), "acct-1", "cred-1", "openai"),
        )
        .await;

        // Tenant B 看不到 Tenant A 的 account / credential / binding。
        assert!(repo.list_accounts(&tenant_b(), None).await.is_empty());
        assert_eq!(
            repo.get_account(&tenant_b(), &AccountId::new("acct-1"))
                .await,
            None
        );
        assert!(repo
            .list_credential_metadata(&tenant_b(), &AccountId::new("acct-1"))
            .await
            .is_empty());
        assert!(repo
            .list_bindings(&tenant_b(), &ProviderId::new("openai"))
            .await
            .is_empty());

        // 跨租户 disable / delete 视为 NotFound（不泄漏存在性以外的信息）。
        assert!(matches!(
            repo.disable_account(&tenant_b(), &AccountId::new("acct-1"))
                .await,
            Err(RepoError::NotFound { .. })
        ));
        assert!(!repo
            .delete_account(&tenant_b(), &AccountId::new("acct-1"))
            .await
            .unwrap());
        // Tenant A 的记录完好。
        assert_eq!(repo.list_accounts(&tenant_a(), None).await.len(), 1);
    }

    #[tokio::test]
    async fn duplicate_create_rejected_and_cascade_delete() {
        let repo = InMemoryProviderAccountRepository::new();
        let tenant = tenant_a();
        create_account(&repo, sample_account(&tenant, "acct-1", "openai")).await;
        create_credential(
            &repo,
            sample_credential(&tenant, "acct-1", "cred-1", "openai"),
        )
        .await;
        create_credential(
            &repo,
            sample_credential(&tenant, "acct-1", "cred-2", "openai"),
        )
        .await;

        // 重复 account / credential ID 冲突。
        assert!(matches!(
            repo.create_account(&tenant, sample_account(&tenant, "acct-1", "openai"))
                .await,
            Err(RepoError::AlreadyExists { .. })
        ));
        assert!(matches!(
            repo.create_credential(
                &tenant,
                sample_credential(&tenant, "acct-1", "cred-1", "openai")
            )
            .await,
            Err(RepoError::AlreadyExists { .. })
        ));

        // 删除 account 级联删除其凭据。
        assert!(repo
            .delete_account(&tenant, &AccountId::new("acct-1"))
            .await
            .unwrap());
        assert!(repo
            .list_credential_metadata(&tenant, &AccountId::new("acct-1"))
            .await
            .is_empty());
        assert!(repo.list_accounts(&tenant, None).await.is_empty());
    }

    #[tokio::test]
    async fn credential_requires_existing_account_in_same_tenant() {
        let repo = InMemoryProviderAccountRepository::new();
        // account 属 Tenant A，credential 试图挂在 Tenant B 的不存在 account 上。
        let cred = sample_credential(&tenant_b(), "acct-x", "cred-1", "openai");
        assert!(matches!(
            repo.create_credential(&tenant_b(), cred).await,
            Err(RepoError::AccountMissing { .. })
        ));
    }

    #[tokio::test]
    async fn create_rejects_caller_tenant_mismatch() {
        // review 项：create account/credential 显式 caller tenant；记录 tenant 与
        // caller 不一致即 TenantMismatch（防御性，避免跨租户写入）。
        let repo = InMemoryProviderAccountRepository::new();
        let account_b = sample_account(&tenant_b(), "acct-1", "openai");
        // account 属 tenant-b，但以 tenant-a 调用 → TenantMismatch，不写入。
        match repo.create_account(&tenant_a(), account_b.clone()).await {
            Err(RepoError::TenantMismatch { record, caller }) => {
                assert_eq!(record, tenant_b());
                assert_eq!(caller, tenant_a());
            }
            other => panic!("expected TenantMismatch, got {other:?}"),
        }
        assert!(repo.list_accounts(&tenant_a(), None).await.is_empty());
        assert!(repo.list_accounts(&tenant_b(), None).await.is_empty());

        let cred_b = sample_credential(&tenant_b(), "acct-1", "cred-1", "openai");
        assert!(matches!(
            repo.create_credential(&tenant_a(), cred_b).await,
            Err(RepoError::TenantMismatch { .. })
        ));
    }

    #[tokio::test]
    async fn credential_summary_excludes_secret_ref() {
        // review 项：list_credential_metadata / list_bindings 返回 CredentialSummary，
        // 绝不暴露 secret_ref（编译期穷尽 CredentialSummary 字段无 secret_ref）。
        let repo = InMemoryProviderAccountRepository::with_legacy_default();
        let tenant = TenantId::new("local/default");
        let creds = repo
            .list_credential_metadata(&tenant, &AccountId::new("local/default"))
            .await;
        assert_eq!(creds.len(), 1);
        let CredentialSummary {
            tenant_id,
            credential_id,
            account_id,
            provider_id,
            kind,
            synthetic,
            state,
            refresh_state,
            expires_at,
        } = &creds[0];
        assert_eq!(tenant_id, &tenant);
        assert_eq!(credential_id.as_str(), "default");
        assert_eq!(account_id.as_str(), "local/default");
        assert_eq!(provider_id.as_str(), "default");
        assert!(*synthetic);
        assert_eq!(*state, CredentialState::Active);
        assert_eq!(*refresh_state, RefreshState::NotRefreshable);
        assert!(expires_at.is_none());
        assert_eq!(*kind, CredentialKind::ApiKey);
        // CredentialSummary 无 secret_ref 字段（编译期保证）。

        // 内部 metadata lookup 仍含 secret_ref（仅 resolver/factory 路径）。
        let metadata = repo
            .get_credential_metadata(&tenant, &CredentialId::new("default"))
            .await
            .expect("metadata present");
        let (svc, acct) = metadata.secret_ref.as_pair();
        assert_eq!(svc, "default");
        assert_eq!(acct, "legacy-default");
    }

    #[tokio::test]
    async fn disable_marks_state_without_deleting() {
        let repo = InMemoryProviderAccountRepository::new();
        let tenant = tenant_a();
        create_account(&repo, sample_account(&tenant, "acct-1", "openai")).await;
        create_credential(
            &repo,
            sample_credential(&tenant, "acct-1", "cred-1", "openai"),
        )
        .await;

        assert!(repo
            .disable_account(&tenant, &AccountId::new("acct-1"))
            .await
            .unwrap());
        assert!(repo
            .disable_credential(&tenant, &CredentialId::new("cred-1"))
            .await
            .unwrap());
        // 再次 disable：仍成功但 changed=false。
        assert!(!repo
            .disable_account(&tenant, &AccountId::new("acct-1"))
            .await
            .unwrap());

        let account = repo
            .get_account(&tenant, &AccountId::new("acct-1"))
            .await
            .unwrap();
        assert_eq!(account.state, AccountState::Disabled);
        let creds = repo
            .list_credential_metadata(&tenant, &AccountId::new("acct-1"))
            .await;
        assert_eq!(creds[0].state, CredentialState::Disabled);
    }

    #[tokio::test]
    async fn test_credential_returns_redacted_status_and_never_plaintext() {
        let repo = InMemoryProviderAccountRepository::new();
        let tenant = tenant_a();
        let resolver = InMemoryCredentialResolver::new();
        create_account(&repo, sample_account(&tenant, "acct-1", "openai")).await;
        let cred = sample_credential(&tenant, "acct-1", "cred-1", "openai");
        resolver.put(&cred.secret_ref, "sk-real-secret-1234567890");
        create_credential(&repo, cred).await;

        // Active + 可解析 → Resolved（结果不携带明文）。
        let status = repo
            .test_credential(
                &tenant,
                &CredentialId::new("cred-1"),
                &resolver,
                Timestamp::from_unix_millis(0),
            )
            .await
            .unwrap();
        assert_eq!(status, CredentialTestStatus::Resolved);
        assert!(!format!("{status:?}").contains("sk-real-secret"));

        // 跨租户 → NotFound。
        assert_eq!(
            repo.test_credential(
                &tenant_b(),
                &CredentialId::new("cred-1"),
                &resolver,
                Timestamp::from_unix_millis(0)
            )
            .await
            .unwrap(),
            CredentialTestStatus::NotFound
        );

        // disabled → NotUsable。
        repo.disable_credential(&tenant, &CredentialId::new("cred-1"))
            .await
            .unwrap();
        assert_eq!(
            repo.test_credential(
                &tenant,
                &CredentialId::new("cred-1"),
                &resolver,
                Timestamp::from_unix_millis(0)
            )
            .await
            .unwrap(),
            CredentialTestStatus::NotUsable(NotUsableReason::Disabled)
        );

        // 重新激活后，secret 未回灌 → Unresolvable。
        {
            let mut credentials = repo.credentials.lock().unwrap();
            credentials
                .get_mut(&(tenant.clone(), CredentialId::new("cred-1")))
                .unwrap()
                .state = CredentialState::Active;
            credentials
                .get_mut(&(tenant.clone(), CredentialId::new("cred-1")))
                .unwrap()
                .secret_ref = crate::account::SecretRef::new("pawork.openai", "never-stored");
        }
        assert_eq!(
            repo.test_credential(
                &tenant,
                &CredentialId::new("cred-1"),
                &resolver,
                Timestamp::from_unix_millis(0)
            )
            .await
            .unwrap(),
            CredentialTestStatus::Unresolvable
        );
    }

    #[tokio::test]
    async fn list_bindings_is_quota_overview_source_of_truth() {
        let repo = InMemoryProviderAccountRepository::with_legacy_default();
        let tenant = TenantId::new("local/default");

        // 按 provider=default 列出绑定：legacy 合成 account + credential。
        let bindings = repo
            .list_bindings(&tenant, &ProviderId::new("default"))
            .await;
        assert_eq!(bindings.len(), 1);
        assert_eq!(bindings[0].account.account_id.as_str(), "local/default");
        assert_eq!(bindings[0].credentials.len(), 1);
        assert_eq!(bindings[0].credentials[0].credential_id.as_str(), "default");

        // 无绑定的 provider 返回空，不做默认推测。
        let empty = repo
            .list_bindings(&tenant, &ProviderId::new("unconfigured"))
            .await;
        assert!(empty.is_empty());

        // 跨租户：另一 tenant 看不到 local/default 的绑定。
        let cross = repo
            .list_bindings(&TenantId::new("remote"), &ProviderId::new("default"))
            .await;
        assert!(cross.is_empty());
    }

    #[tokio::test]
    async fn list_accounts_filter_by_provider() {
        let repo = InMemoryProviderAccountRepository::new();
        let tenant = tenant_a();
        create_account(&repo, sample_account(&tenant, "acct-openai", "openai")).await;
        create_account(
            &repo,
            sample_account(&tenant, "acct-anthropic", "anthropic"),
        )
        .await;

        let all = repo.list_accounts(&tenant, None).await;
        assert_eq!(all.len(), 2);
        let only_openai = repo
            .list_accounts(&tenant, Some(&ProviderId::new("openai")))
            .await;
        assert_eq!(only_openai.len(), 1);
        assert_eq!(only_openai[0].provider_id.as_str(), "openai");
    }

    #[tokio::test]
    async fn repository_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<InMemoryProviderAccountRepository>();
        assert_send_sync::<&dyn ProviderAccountRepository>();
    }
}
