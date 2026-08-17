//! 控制面持久化实体（纯类型草案，ADR-033/P18-1；P18-3 扩展独立生命周期）。
//!
//! P18-3 在 P18-1 草案之上把「账号状态」从凭据中剥离：账号侧承载
//! priority/weight/max_concurrency/state，凭据侧只持有 [`SecretRef`]（opaque
//! Keychain 引用，绝非明文）/state/refresh_state/过期。二者生命周期独立，为多
//! 账号、多凭据、健康与 lease 奠定数据模型。
//!
//! 这些类型是控制面表的 Rust 镜像；**不含任何 secret 字段**（明文 API Key 存
//! 于 OS Keychain，见 ADR-014；本结构只持有脱敏定位 / 元数据）。每个实体携带
//! `schema_version`，便于版本化迁移与重放兼容。tenant-scoped repository 与
//! secret resolver 见 [`crate::repository`] / [`crate::credential`]。

use pawork_domain::{AccountId, CredentialId, PrincipalId, ProviderId, TenantId, Timestamp};
use serde::{Deserialize, Serialize};

use crate::routing::RoutingStrategy;
use crate::{legacy, CONTROL_PLANE_SCHEMA_VERSION};

/// 凭据种类（脱敏枚举，**绝非凭据值**）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialKind {
    /// API Key（存于 OS Keychain，此处仅记录种类）。
    ApiKey,
    /// OAuth 令牌（同上）。
    OAuth,
    /// 其它种类；具体协议由 Provider 侧解释。
    Other,
}

impl CredentialKind {
    /// 冻结的持久化字符串（与 `app-database` 控制面 schema 对齐）。
    pub fn as_db_str(self) -> &'static str {
        match self {
            CredentialKind::ApiKey => "api_key",
            CredentialKind::OAuth => "oauth",
            CredentialKind::Other => "other",
        }
    }
}

/// 账号生命周期状态（与凭据状态独立）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountState {
    /// 可用：参与路由与 lease。
    #[default]
    Active,
    /// 已禁用：不参与路由，但保留行（可审计、可重新启用）。
    Disabled,
}

impl AccountState {
    /// 冻结的持久化字符串（与 `app-database` 控制面 schema v2 对齐）。
    pub fn as_db_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Disabled => "disabled",
        }
    }

    /// 由持久化字符串反解；未知值返回 `None`。
    pub fn from_db_str(value: &str) -> Option<Self> {
        match value {
            "active" => Some(Self::Active),
            "disabled" => Some(Self::Disabled),
            _ => None,
        }
    }
}

/// 凭据生命周期状态（与账号状态独立）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialState {
    /// 可用：可被 resolver 解析。
    #[default]
    Active,
    /// 已禁用：不参与 lease / 解析。
    Disabled,
    /// 已过期（按 `expires_at` 判定或显式标记）。
    Expired,
    /// 已吊销（删除前的终态，或显式 revoke）。
    Revoked,
}

impl CredentialState {
    /// 冻结的持久化字符串（与 `app-database` 控制面 schema v2 对齐）。
    pub fn as_db_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Disabled => "disabled",
            Self::Expired => "expired",
            Self::Revoked => "revoked",
        }
    }

    /// 由持久化字符串反解；未知值返回 `None`。
    pub fn from_db_str(value: &str) -> Option<Self> {
        match value {
            "active" => Some(Self::Active),
            "disabled" => Some(Self::Disabled),
            "expired" => Some(Self::Expired),
            "revoked" => Some(Self::Revoked),
            _ => None,
        }
    }

    /// 是否仍可被解析 / 参与 lease。
    pub fn is_usable(self) -> bool {
        matches!(self, Self::Active)
    }
}

/// 凭据刷新状态（P18-3：versioned refresh lifecycle）。
///
/// 与 [`CredentialState`] 正交：`Active` 的凭据仍可能处于 `Refreshing` / `Failed`
/// 而不适合取用（factory compose / test_credential fail-closed）。serde 与 DB
/// 字符串稳定冻结（snake_case），未知值 fail-closed。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RefreshState {
    /// 不可刷新（静态 API Key 等）；默认值。
    #[default]
    NotRefreshable,
    /// 已就绪：最近一次刷新成功，可正常取用。
    Ready,
    /// 刷新中：暂不适合取用（避免取到半旧令牌）。
    Refreshing,
    /// 刷新失败：暂不适合取用，等待重试 / 人工介入。
    Failed,
}

impl RefreshState {
    /// 冻结的持久化字符串（与 `app-database` 控制面 schema v2 对齐）。
    pub fn as_db_str(self) -> &'static str {
        match self {
            Self::NotRefreshable => "not_refreshable",
            Self::Ready => "ready",
            Self::Refreshing => "refreshing",
            Self::Failed => "failed",
        }
    }

    /// 由持久化字符串反解；未知值返回 `None`（fail-closed）。
    pub fn from_db_str(value: &str) -> Option<Self> {
        match value {
            "not_refreshable" => Some(Self::NotRefreshable),
            "ready" => Some(Self::Ready),
            "refreshing" => Some(Self::Refreshing),
            "failed" => Some(Self::Failed),
            _ => None,
        }
    }
}

/// 凭据不可取用的脱敏原因（factory compose / test_credential 共用，
/// **绝不携带明文或定位对**）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotUsableReason {
    /// `state == Disabled`。
    Disabled,
    /// `state == Expired` 或 `expires_at` 已到期。
    Expired,
    /// `state == Revoked`。
    Revoked,
    /// `refresh_state == Refreshing`。
    Refreshing,
    /// `refresh_state == Failed`。
    RefreshFailed,
}
impl std::fmt::Display for NotUsableReason {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let wire = match self {
            Self::Disabled => "disabled",
            Self::Expired => "expired",
            Self::Revoked => "revoked",
            Self::Refreshing => "refreshing",
            Self::RefreshFailed => "refresh_failed",
        };
        formatter.write_str(wire)
    }
}

/// 可注入的时钟（factory / repository 据 `expires_at` 判定取用准入）。
///
/// 测试注入 [`FixedClock`]（固定时间，避免 flaky）；生产注入 [`SystemClock`]。
/// 不依赖全局可变状态，避免隐式时序耦合。
pub trait Clock: Send + Sync {
    /// 当前时间（Unix 毫秒）。
    fn now(&self) -> Timestamp;
}

/// 固定时钟（测试用）：始终返回构造时给定的时间。
#[derive(Clone, Copy, Debug)]
pub struct FixedClock(Timestamp);

impl FixedClock {
    /// 以固定时间构造。
    pub const fn new(timestamp: Timestamp) -> Self {
        Self(timestamp)
    }
}

impl Clock for FixedClock {
    fn now(&self) -> Timestamp {
        self.0
    }
}

/// 系统时钟（生产用）：读取 OS 真实墙钟时间。
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Timestamp {
        Timestamp::from_unix_millis(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_millis() as u64)
                .unwrap_or(0),
        )
    }
}

/// Opaque Secret 引用：定位 OS Keychain 中的明文，**本身不含任何明文**。
///
/// 与 ADR-014 一致：数据库 / 日志只持有 `(service, account)` 定位元数据；
/// 明文由注入的 [`crate::credential::CredentialResolver`] 在运行时短生命周期
/// 解析。序列化结果不得包含任何 secret 值。
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct SecretRef {
    /// OS Keychain `service`（脱敏定位，绝非明文 secret）。
    pub service: String,
    /// OS Keychain `account`（脱敏定位，绝非明文 secret）。
    pub account: String,
}

impl SecretRef {
    /// 以 `(service, account)` 构造 opaque 引用。
    pub fn new(service: impl Into<String>, account: impl Into<String>) -> Self {
        Self {
            service: service.into(),
            account: account.into(),
        }
    }

    /// 返回 `(service, account)` 借用视图，供 resolver 定位 Keychain。
    pub fn as_pair(&self) -> (&str, &str) {
        (self.service.as_str(), self.account.as_str())
    }
}

/// Provider 账号记录（`provider_accounts` 表镜像，versioned、tenant-bound）。
///
/// 账号侧承载 priority / weight / max_concurrency / state；凭据侧不再塞状态，
/// 二者生命周期独立（ADR-033）。P18-3 新增字段 `#[serde(default)]` 以兼容 v1
/// 序列化数据与未升级读取方。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct ProviderAccountRecord {
    /// 实体 schema 版本。
    pub schema_version: u32,
    pub tenant_id: TenantId,
    pub account_id: AccountId,
    pub provider_id: ProviderId,
    /// 归属主体（谁拥有此账号）。
    pub principal_id: PrincipalId,
    pub display_name: String,
    pub routing_strategy: RoutingStrategy,
    /// 路由优先级（数字越小越优先；`SingleCandidate` 时忽略）。
    #[serde(default)]
    pub priority: u32,
    /// 加权轮询权重（0 表示不参与加权；默认 1）。
    #[serde(default = "default_weight")]
    pub weight: u32,
    /// 账号并发上限（active lease 不得超过此值；P18-4 池据此准入）。
    #[serde(default = "default_concurrency")]
    pub max_concurrency: u64,
    /// 账号生命周期状态。
    #[serde(default)]
    pub state: AccountState,
}

fn default_weight() -> u32 {
    1
}
fn default_concurrency() -> u64 {
    1
}

/// 凭据元数据（`credentials` 表镜像，versioned、tenant-bound，**无 secret 字段**）。
///
/// P18-3：canonical 持久实体类型为 [`CredentialMetadata`]；凭据只持有
/// [`SecretRef`]（opaque Keychain 引用）、过期、状态与刷新状态；明文绝不
/// 入库（ADR-014）。新增字段 `#[serde(default)]` 以兼容 v1 数据。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct CredentialMetadata {
    /// 实体 schema 版本。
    pub schema_version: u32,
    pub tenant_id: TenantId,
    pub credential_id: CredentialId,
    pub account_id: AccountId,
    pub provider_id: ProviderId,
    /// 凭据种类（脱敏枚举，绝非凭据值）。
    pub kind: CredentialKind,
    /// 是否为 legacy 合成凭据（迁移自动包装）。
    pub synthetic: bool,
    /// Opaque Secret 引用（定位 Keychain，绝非明文）。
    #[serde(default)]
    pub secret_ref: SecretRef,
    /// 凭据生命周期状态。
    #[serde(default)]
    pub state: CredentialState,
    /// 过期时间（Unix 毫秒）；`None` 表示不过期。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<Timestamp>,
    /// 刷新状态（versioned refresh lifecycle）。`Refreshing` / `Failed` 的凭据
    /// 不参与取用（factory / test_credential fail-closed）。
    #[serde(default)]
    pub refresh_state: RefreshState,
}

/// 兼容 alias：历史导入路径 `CredentialRecord`（review 项：canonical 名为
/// [`CredentialMetadata`]，即真实的持久实体 struct）。
pub type CredentialRecord = CredentialMetadata;

impl ProviderAccountRecord {
    /// 构造 legacy 合成默认账号（与 `legacy::SyntheticDefaultAccount::legacy_default` 对齐）。
    pub fn legacy_synthetic_default() -> Self {
        Self::legacy_synthetic_default_with_version(CONTROL_PLANE_SCHEMA_VERSION)
    }

    /// 以指定 schema 版本构造 legacy 合成默认账号（供迁移测试断言 v1/v2 形态）。
    pub fn legacy_synthetic_default_with_version(schema_version: u32) -> Self {
        Self {
            schema_version,
            tenant_id: TenantId::new(legacy::LEGACY_TENANT),
            account_id: AccountId::new(legacy::LEGACY_ACCOUNT),
            provider_id: ProviderId::new(legacy::LEGACY_PROVIDER),
            principal_id: PrincipalId::new(legacy::LEGACY_PRINCIPAL),
            display_name: "Legacy default account".to_string(),
            routing_strategy: RoutingStrategy::SingleCandidate,
            priority: 0,
            weight: 1,
            max_concurrency: 1,
            state: AccountState::Active,
        }
    }
}

impl CredentialMetadata {
    /// 构造 legacy 合成默认凭据（`Credential(default)`，无 secret）。
    pub fn legacy_synthetic_default() -> Self {
        Self::legacy_synthetic_default_with_version(CONTROL_PLANE_SCHEMA_VERSION)
    }

    /// 以指定 schema 版本构造 legacy 合成默认凭据。
    ///
    /// 合成凭据的 `secret_ref` 为 sentinel（`default` service + `legacy-default`
    /// account）。它不指向真实明文——resolver 在未由宿主回灌真实 key 时
    /// fail-closed（合成凭据只表达「旧单 credential 自动包装」，真实 key 由宿主
    /// 从 legacy 配置回灌到 Keychain）。
    pub fn legacy_synthetic_default_with_version(schema_version: u32) -> Self {
        Self {
            schema_version,
            tenant_id: TenantId::new(legacy::LEGACY_TENANT),
            credential_id: CredentialId::new(legacy::LEGACY_CREDENTIAL),
            account_id: AccountId::new(legacy::LEGACY_ACCOUNT),
            provider_id: ProviderId::new(legacy::LEGACY_PROVIDER),
            kind: CredentialKind::ApiKey,
            synthetic: true,
            secret_ref: SecretRef::new(legacy::LEGACY_PROVIDER, "legacy-default"),
            state: CredentialState::Active,
            expires_at: None,
            refresh_state: RefreshState::NotRefreshable,
        }
    }

    /// 判定是否可被取用（factory compose / test_credential 共用的 fail-closed
    /// 准入闸门）。三者全满足才放行：`state` 为 `Active`、`refresh_state` 非
    /// `Refreshing`/`Failed`、`expires_at` 未到期。
    ///
    /// 不可用时返回脱敏 [`NotUsableReason`]（**绝不携带明文或定位对**）。
    pub fn usable_for_take(&self, now: Timestamp) -> Result<(), NotUsableReason> {
        match self.state {
            CredentialState::Disabled => Err(NotUsableReason::Disabled),
            CredentialState::Expired => Err(NotUsableReason::Expired),
            CredentialState::Revoked => Err(NotUsableReason::Revoked),
            CredentialState::Active => match self.refresh_state {
                RefreshState::Refreshing => Err(NotUsableReason::Refreshing),
                RefreshState::Failed => Err(NotUsableReason::RefreshFailed),
                RefreshState::NotRefreshable | RefreshState::Ready => {
                    if let Some(expires_at) = self.expires_at {
                        if now.as_unix_millis() >= expires_at.as_unix_millis() {
                            return Err(NotUsableReason::Expired);
                        }
                    }
                    Ok(())
                }
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_synthetic_records_carry_version_and_adr_033_scope() {
        let account = ProviderAccountRecord::legacy_synthetic_default();
        assert_eq!(account.schema_version, CONTROL_PLANE_SCHEMA_VERSION);
        assert_eq!(account.tenant_id.as_str(), "local/default");
        assert_eq!(account.account_id.as_str(), "local/default");
        assert_eq!(account.principal_id.as_str(), "local/user");
        assert_eq!(account.provider_id.as_str(), "default");
        assert_eq!(account.routing_strategy, RoutingStrategy::SingleCandidate);

        let credential = CredentialMetadata::legacy_synthetic_default();
        assert_eq!(credential.schema_version, CONTROL_PLANE_SCHEMA_VERSION);
        assert_eq!(credential.credential_id.as_str(), "default");
        assert_eq!(credential.account_id.as_str(), "local/default");
        assert!(credential.synthetic);
        assert_eq!(credential.kind, CredentialKind::ApiKey);
        assert_eq!(credential.state, CredentialState::Active);
        assert_eq!(credential.expires_at, None);
        assert_eq!(credential.refresh_state, RefreshState::NotRefreshable);
    }

    #[test]
    fn with_version_preserves_input_schema_version_for_migration_fixtures() {
        // review 项：`legacy_synthetic_default_with_version` 必须保留传入版本——
        // 迁移测试用 v1 fixture 断言「v1 数据库里的合成记录仍是 schema_version=1」。
        // 旧实现 `CURRENT.max(input)` 会把 v1 静默上抬成 v2，掩盖迁移断言。
        let v1_account = ProviderAccountRecord::legacy_synthetic_default_with_version(1);
        assert_eq!(v1_account.schema_version, 1, "v1 fixture must stay v1");
        let v1_credential = CredentialMetadata::legacy_synthetic_default_with_version(1);
        assert_eq!(v1_credential.schema_version, 1, "v1 fixture must stay v1");
        // 其余字段仍为 ADR-033 legacy 作用域。
        assert_eq!(v1_account.tenant_id.as_str(), "local/default");
        assert_eq!(
            v1_credential.secret_ref.as_pair(),
            ("default", "legacy-default")
        );
        // 无参入口仍用当前版本。
        assert_eq!(
            ProviderAccountRecord::legacy_synthetic_default().schema_version,
            CONTROL_PLANE_SCHEMA_VERSION
        );
        assert_eq!(
            CredentialMetadata::legacy_synthetic_default().schema_version,
            CONTROL_PLANE_SCHEMA_VERSION
        );
    }

    #[test]
    fn account_carries_independent_lifecycle_fields() {
        let account = ProviderAccountRecord::legacy_synthetic_default();
        assert_eq!(account.priority, 0);
        assert_eq!(account.weight, 1);
        assert_eq!(account.max_concurrency, 1);
        assert_eq!(account.state, AccountState::Active);

        // 编译期穷尽：ProviderAccountRecord 全部 11 个字段均为脱敏定位/元数据字段。
        let ProviderAccountRecord {
            schema_version,
            tenant_id,
            account_id,
            provider_id,
            principal_id,
            display_name,
            routing_strategy,
            priority,
            weight,
            max_concurrency,
            state,
        } = &account;
        assert_eq!(*schema_version, CONTROL_PLANE_SCHEMA_VERSION);
        assert_eq!(tenant_id.as_str(), "local/default");
        assert_eq!(account_id.as_str(), "local/default");
        assert_eq!(provider_id.as_str(), "default");
        assert_eq!(principal_id.as_str(), "local/user");
        assert!(!display_name.is_empty());
        assert_eq!(*routing_strategy, RoutingStrategy::SingleCandidate);
        assert_eq!(*priority, 0);
        assert_eq!(*weight, 1);
        assert_eq!(*max_concurrency, 1);
        assert_eq!(*state, AccountState::Active);
    }

    #[test]
    fn account_and_credential_state_db_strings_are_stable() {
        for state in [AccountState::Active, AccountState::Disabled] {
            let wire = state.as_db_str();
            assert_eq!(AccountState::from_db_str(wire), Some(state));
        }
        assert_eq!(AccountState::from_db_str("unknown"), None);

        for state in [
            CredentialState::Active,
            CredentialState::Disabled,
            CredentialState::Expired,
            CredentialState::Revoked,
        ] {
            let wire = state.as_db_str();
            assert_eq!(CredentialState::from_db_str(wire), Some(state));
        }
        assert_eq!(CredentialState::from_db_str("unknown"), None);
        assert!(CredentialState::Active.is_usable());
        assert!(!CredentialState::Disabled.is_usable());
        assert!(!CredentialState::Expired.is_usable());
        assert!(!CredentialState::Revoked.is_usable());
    }

    #[test]
    fn refresh_state_db_strings_are_stable_and_round_trip() {
        for state in [
            RefreshState::NotRefreshable,
            RefreshState::Ready,
            RefreshState::Refreshing,
            RefreshState::Failed,
        ] {
            let wire = state.as_db_str();
            assert_eq!(RefreshState::from_db_str(wire), Some(state));
        }
        // 未知值 fail-closed。
        assert_eq!(RefreshState::from_db_str("unknown"), None);
        // 冻结字符串稳定（不得随意改名，否则破坏 DB 兼容）。
        assert_eq!(RefreshState::NotRefreshable.as_db_str(), "not_refreshable");
        assert_eq!(RefreshState::Ready.as_db_str(), "ready");
        assert_eq!(RefreshState::Refreshing.as_db_str(), "refreshing");
        assert_eq!(RefreshState::Failed.as_db_str(), "failed");
    }

    #[test]
    fn secret_ref_holds_only_opaque_locators_not_plaintext() {
        let secret_ref = SecretRef::new("pawork.openai", "cred_abc");
        let (service, account) = secret_ref.as_pair();
        assert_eq!(service, "pawork.openai");
        assert_eq!(account, "cred_abc");

        let json = serde_json::to_value(&secret_ref).expect("serialize secret ref");
        // 编译期穷尽：SecretRef 只有 service/account 两个定位字段。
        let SecretRef {
            service: s,
            account: a,
        } = &secret_ref;
        assert_eq!(s, "pawork.openai");
        assert_eq!(a, "cred_abc");
        for forbidden in ["secret", "token", "api_key", "password", "value"] {
            assert!(
                !json
                    .as_object()
                    .is_some_and(|object| object.contains_key(forbidden)),
                "SecretRef 不得包含字段 `{forbidden}`"
            );
        }
    }

    #[test]
    fn legacy_synthetic_credential_secret_ref_is_sentinel_and_fail_safe() {
        let credential = CredentialMetadata::legacy_synthetic_default();
        // 合成凭据指向 sentinel ref，绝非明文；resolver 未回灌时 fail-closed。
        let (service, account) = credential.secret_ref.as_pair();
        assert_eq!(service, "default");
        assert_eq!(account, "legacy-default");
        assert_eq!(credential.schema_version, CONTROL_PLANE_SCHEMA_VERSION);
    }

    #[test]
    fn credential_metadata_destructure_has_no_secret_field() {
        // 编译期穷尽：CredentialMetadata 全部 11 个字段均为脱敏定位/元数据字段，
        // secret_ref 只是 opaque 定位（service/account），绝非明文 token。
        let credential = CredentialMetadata::legacy_synthetic_default();
        let CredentialMetadata {
            schema_version,
            tenant_id,
            credential_id,
            account_id,
            provider_id,
            kind,
            synthetic,
            secret_ref,
            state,
            expires_at,
            refresh_state,
        } = &credential;
        assert_eq!(*schema_version, CONTROL_PLANE_SCHEMA_VERSION);
        assert_eq!(tenant_id.as_str(), "local/default");
        assert_eq!(credential_id.as_str(), "default");
        assert_eq!(account_id.as_str(), "local/default");
        assert_eq!(provider_id.as_str(), "default");
        assert_eq!(*kind, CredentialKind::ApiKey);
        assert!(*synthetic);
        // secret_ref 是 opaque 定位对，不是明文。
        let (svc, acct) = secret_ref.as_pair();
        assert_eq!(svc, "default");
        assert_eq!(acct, "legacy-default");
        assert_eq!(*state, CredentialState::Active);
        assert!(expires_at.is_none());
        assert_eq!(*refresh_state, RefreshState::NotRefreshable);
    }

    #[test]
    fn credential_kind_db_string_is_stable() {
        assert_eq!(CredentialKind::ApiKey.as_db_str(), "api_key");
        assert_eq!(CredentialKind::OAuth.as_db_str(), "oauth");
        assert_eq!(CredentialKind::Other.as_db_str(), "other");
    }

    #[test]
    fn versioned_records_round_trip_with_stable_snake_case_wire_values() {
        let account = ProviderAccountRecord::legacy_synthetic_default();
        let account_json = serde_json::to_value(&account).expect("serialize account");
        assert_eq!(account_json["tenant_id"], "local/default");
        assert_eq!(account_json["routing_strategy"], "single_candidate");
        assert_eq!(account_json["state"], "active");
        assert_eq!(
            serde_json::from_value::<ProviderAccountRecord>(account_json).expect("decode account"),
            account
        );

        let credential = CredentialMetadata::legacy_synthetic_default();
        let credential_json = serde_json::to_value(&credential).expect("serialize credential");
        assert_eq!(credential_json["tenant_id"], "local/default");
        assert_eq!(credential_json["kind"], "api_key");
        assert_eq!(credential_json["state"], "active");
        assert_eq!(credential_json["refresh_state"], "not_refreshable");
        // expires_at 为 None 时 skip，旧 v1 数据缺该字段仍可解码。
        assert!(credential_json.get("expires_at").is_none());
        assert_eq!(
            serde_json::from_value::<CredentialMetadata>(credential_json)
                .expect("decode credential"),
            credential
        );
    }

    #[test]
    fn versioned_records_reject_unknown_fields_and_enum_values() {
        let mut account = serde_json::to_value(ProviderAccountRecord::legacy_synthetic_default())
            .expect("serialize account");
        account["future_field"] = serde_json::json!(true);
        assert!(serde_json::from_value::<ProviderAccountRecord>(account).is_err());

        let mut credential = serde_json::to_value(CredentialMetadata::legacy_synthetic_default())
            .expect("serialize credential");
        credential["kind"] = serde_json::json!("future_kind");
        assert!(serde_json::from_value::<CredentialMetadata>(credential).is_err());

        // 非法状态枚举值同样 fail-closed。
        let mut bad_state = serde_json::to_value(CredentialMetadata::legacy_synthetic_default())
            .expect("serialize credential");
        bad_state["state"] = serde_json::json!("future_state");
        assert!(serde_json::from_value::<CredentialMetadata>(bad_state).is_err());

        // 非法 refresh_state 枚举值同样 fail-closed。
        let mut bad_refresh = serde_json::to_value(CredentialMetadata::legacy_synthetic_default())
            .expect("serialize credential");
        bad_refresh["refresh_state"] = serde_json::json!("future_refresh");
        assert!(serde_json::from_value::<CredentialMetadata>(bad_refresh).is_err());
    }

    #[test]
    fn v1_wire_without_p18_3_fields_decodes_with_defaults() {
        // 模拟 v1（P18-1）序列化数据：缺 priority/weight/max_concurrency/state/
        // secret_ref/expires_at/refresh_state。带 #[serde(default)] 必须可解码为 v2 形态。
        let v1_account = serde_json::json!({
            "schema_version": 1,
            "tenant_id": "local/default",
            "account_id": "local/default",
            "provider_id": "default",
            "principal_id": "local/user",
            "display_name": "Legacy default account",
            "routing_strategy": "single_candidate",
        });
        let account: ProviderAccountRecord =
            serde_json::from_value(v1_account).expect("v1 account decodes");
        assert_eq!(account.priority, 0);
        assert_eq!(account.weight, 1);
        assert_eq!(account.max_concurrency, 1);
        assert_eq!(account.state, AccountState::Active);

        let v1_credential = serde_json::json!({
            "schema_version": 1,
            "tenant_id": "local/default",
            "credential_id": "default",
            "account_id": "local/default",
            "provider_id": "default",
            "kind": "api_key",
            "synthetic": true,
        });
        let credential: CredentialMetadata =
            serde_json::from_value(v1_credential).expect("v1 credential decodes");
        assert_eq!(credential.state, CredentialState::Active);
        assert!(credential.expires_at.is_none());
        assert_eq!(credential.refresh_state, RefreshState::NotRefreshable);
        // v1 数据缺 secret_ref → 默认空串 sentinel；解析时 fail-closed。
        let (svc, _acct) = credential.secret_ref.as_pair();
        assert_eq!(svc, "");
    }

    #[test]
    fn usable_for_take_fail_closes_on_state_refresh_and_expiry() {
        let now = Timestamp::from_unix_millis(1_000_000);
        let mut credential = CredentialMetadata::legacy_synthetic_default();

        // Active + NotRefreshable + 无过期 → 可取用。
        assert!(credential.usable_for_take(now).is_ok());

        // 非 Active 状态 → 对应原因。
        credential.state = CredentialState::Disabled;
        assert_eq!(
            credential.usable_for_take(now),
            Err(NotUsableReason::Disabled)
        );
        credential.state = CredentialState::Expired;
        assert_eq!(
            credential.usable_for_take(now),
            Err(NotUsableReason::Expired)
        );
        credential.state = CredentialState::Revoked;
        assert_eq!(
            credential.usable_for_take(now),
            Err(NotUsableReason::Revoked)
        );

        // Active 但 refresh_state 不可取用。
        credential.state = CredentialState::Active;
        credential.refresh_state = RefreshState::Refreshing;
        assert_eq!(
            credential.usable_for_take(now),
            Err(NotUsableReason::Refreshing)
        );
        credential.refresh_state = RefreshState::Failed;
        assert_eq!(
            credential.usable_for_take(now),
            Err(NotUsableReason::RefreshFailed)
        );

        // Active + Ready + 已过期（注入固定 clock，避免 flaky）。
        credential.refresh_state = RefreshState::Ready;
        credential.expires_at = Some(Timestamp::from_unix_millis(999_999));
        assert_eq!(
            credential.usable_for_take(now),
            Err(NotUsableReason::Expired)
        );

        // 恰好未过期（now < expires_at）→ 可取用。
        credential.expires_at = Some(Timestamp::from_unix_millis(1_000_001));
        assert!(credential.usable_for_take(now).is_ok());
    }

    #[test]
    fn clock_is_injectable_for_deterministic_expiry_tests() {
        // 固定时钟使过期断言确定（非 wall-clock 依赖）。
        let clock = FixedClock::new(Timestamp::from_unix_millis(5_000_000));
        let mut credential = CredentialMetadata::legacy_synthetic_default();
        credential.refresh_state = RefreshState::Ready;
        credential.expires_at = Some(Timestamp::from_unix_millis(4_999_999));
        assert_eq!(
            credential.usable_for_take(clock.now()),
            Err(NotUsableReason::Expired)
        );
        // SystemClock 不 panic（生产路径）。
        let _ = SystemClock.now();
    }
}
