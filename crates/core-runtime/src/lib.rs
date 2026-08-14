//! CoreRuntime（P13-2）：完整 Core 的进程内装配。
//!
//! 装配 [`AppService`]（含 [`CommandRouter`]）与 [`EventHub`]，并运行
//! `EventPump` 后台任务：以固定间隔（默认 10ms）轮询
//! `router.drain_events()` / `supervisor.drain_events()`（Run 监督器的限流合并
//! 输出），发布到 Event Hub——CLI 渲染器与未来 GUI 订阅到同一份全局连续序列
//! 的事件流（连续性由 Hub 的强制重写保证）。
//!
//! P18-4 生产接线：`with_persistent_control_plane` 打开 app-database Actor、
//! 迁移 lease 投影、装配 [`SqliteLeaseProjection`] 组合层适配并注入持久
//! `CredentialPool`，启动时 `restore` 回收孤儿 lease，并启动周期 reclaim
//! 任务（随 shutdown 结束；失败只告警并等下一周期，不回退内存池 / 不复制
//! provider-control 的 lease 状态机）。

pub mod lease_projection;

pub use lease_projection::SqliteLeaseProjection;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use agent_domain::{AccountId, CredentialId, PrincipalId, ProviderId, TenantId, Timestamp};
use app_database::{DatabaseActor, DatabaseError};
use app_service::AppService;
use app_service::UserHookHost;
use provider_api::ModelProvider;
use provider_control::{
    AccountState, CredentialKind, CredentialMetadata, CredentialPool, CredentialState,
    InMemoryCredentialPool, InMemoryProviderAccountRepository, PoolConfig, PoolError,
    ProviderAccountRecord, ProviderAccountRepository, ReclaimReport, RefreshState,
    RepositoryCredentialPicker, RoutingStrategy, SecretRef, SystemLeaseClock,
};
use subscription_hub::{EventHub, DEFAULT_HUB_CAPACITY};
use thiserror::Error;
use tokio::sync::watch;
use tracing::debug;

/// 持久控制面装配错误（P18-4）：任何一步失败都 fail loud，不静默降级
/// （迁移失败 / restore 失败 / 账本打开失败均阻止启动）。
#[derive(Debug, Error)]
pub enum ControlPlaneError {
    /// 数据库 Actor 打开失败。
    #[error(transparent)]
    Database(#[from] DatabaseError),
    /// lease 投影迁移失败。
    #[error(transparent)]
    Migration(#[from] app_database::MigrationError),
    /// 启动恢复（restore）失败。
    #[error("credential pool restore failed: {0}")]
    Restore(#[from] PoolError),
    /// 持久 usage/cost 账本打开失败。
    #[error(transparent)]
    Ledger(#[from] usage_ledger::UsageLedgerError),
    /// durable Team store 打开或重放失败。
    #[error(transparent)]
    Team(#[from] app_service::TeamError),
    /// 从 SQLite 控制面表加载账号 / 凭据失败（严格解析：未知枚举 / 坏行 / 类型
    /// 不匹配）。fail loud：坏数据阻止启动，绝不静默丢弃或回退 legacy 种子。
    #[error("control plane repository load failed: {0}")]
    RepositoryLoad(String),
}

/// 持久控制面运行产物（P18-4）：数据库 Actor 由池（投影 → 仓库）持有，与
/// CoreRuntime 生命周期一致。
pub struct ControlPlaneRuntime {
    /// 打开的 SQLite Actor（`credential_leases` 等投影所在库）。
    pub database: DatabaseActor,
    /// 控制面 schema 迁移报告（`provider_accounts` / `credentials` 表）。
    /// 字段名保留为 `migration_report` 以兼容既有调用方；lease 投影迁移见
    /// [`ControlPlaneRuntime::lease_migration_report`]。
    pub migration_report: app_database::MigrationReport,
    /// Lease 投影迁移报告（`credential_leases` 表）。
    pub lease_migration_report: app_database::MigrationReport,
    /// 启动恢复报告（回收孤儿 lease 计数）。
    pub restore_report: ReclaimReport,
    /// 持久化凭据池（已 restore）。
    pub pool: Arc<dyn CredentialPool>,
    /// 与池 picker 同源的账号仓库：从控制面表严格解析加载，不再 hardcode
    /// legacy 种子。
    pub account_repository: Arc<InMemoryProviderAccountRepository>,
}

/// CoreRuntime 配置。
#[derive(Clone, Debug)]
pub struct CoreRuntimeConfig {
    /// Core 实例名（默认 `default`；命名实例拥有独立 Endpoint 与状态）。
    pub instance: String,
    /// EventPump 轮询间隔（默认 10ms）。
    pub pump_interval: Duration,
    /// Event Hub ring buffer / 广播容量（默认 4096）。
    pub hub_capacity: usize,
    /// Team canonical event store。`None` 仅用于测试/嵌入；正式 `pawork`
    /// 必须传入实例目录中的持久路径。
    pub team_db_path: Option<PathBuf>,
    /// P17-1 User Hooks 宿主（正式宿主装配后注入；`None` 时 run 不回调 hooks）。
    pub user_hooks: Option<Arc<UserHookHost>>,
    /// 持久 CredentialPool 的周期 reclaim 间隔（P18-4 审查补救）；`None` =
    /// 不启周期回收（默认，测试 / 嵌入式装配不变）。生产持久控制面装配
    /// （[`Self::with_persistent_control_plane`]）启用。
    pub reclaim_interval: Option<Duration>,
}

/// 生产持久池周期 reclaim 默认间隔（秒）：远小于默认 lease TTL（1h），
/// 崩溃遗留的过期 lease 在数分钟内归还并发额度。
pub const CREDENTIAL_RECLAIM_INTERVAL_SECS: u64 = 30;

impl Default for CoreRuntimeConfig {
    fn default() -> Self {
        Self {
            instance: "default".into(),
            pump_interval: Duration::from_millis(10),
            hub_capacity: DEFAULT_HUB_CAPACITY,
            team_db_path: None,
            user_hooks: None,
            reclaim_interval: None,
        }
    }
}

/// 完整 Core 运行时：AppService + EventHub + EventPump。
///
/// [`CoreRuntime::shutdown`] 停止 EventPump（幂等）；Run 任务本身由
/// `RunCancel` / 终态自行收敛，不随 pump 终止而取消（[ADR-026]）。
///
/// [ADR-026]: ../../docs/adr/ADR-026-gui-disconnect-safe.md
pub struct CoreRuntime {
    service: Arc<AppService>,
    hub: Arc<EventHub>,
    pump: tokio::task::JoinHandle<()>,
    /// P18-4 审查补救：持久池周期 reclaim 任务（随 shutdown 结束）。
    reclaim: Option<tokio::task::JoinHandle<()>>,
    shutdown: watch::Sender<bool>,
}

impl CoreRuntime {
    /// 以默认配置装配（实例名 + 10ms pump + 4096 Hub 容量 + 进程内
    /// Quota 运行时）。
    ///
    /// **仅供测试 / 嵌入式便捷使用**：进程内内存账本不跨进程，不是生产
    /// 构造（P18-8 review：生产 CLI 必须走 [`Self::with_persistent_ledger`]，
    /// 禁止以内存账本作为生产累计源）。现有调用点均为测试（core-runtime /
    /// cli-host / gui_serve 测试装配）。
    pub fn new(instance: impl Into<String>) -> Self {
        Self::with_config(CoreRuntimeConfig {
            instance: instance.into(),
            ..CoreRuntimeConfig::default()
        })
    }

    /// 以指定配置装配。默认携带进程内 Quota 运行时（共享
    /// [`app_service::QuotaRuntime`]：内存账本 + 系统时钟，唯一本地 ledger
    /// 适配器，构造与空查询不触发网络）；**仅供测试 / 嵌入式便捷使用，
    /// 不是生产构造**——生产 CLI 必须走 [`Self::with_persistent_ledger`]，
    /// 禁止以内存账本作为生产累计源（P18-8 review）。`from_parts` 注入的既有
    /// `AppService` 原样保留，不覆盖其 Quota 注入状态。
    pub fn with_config(config: CoreRuntimeConfig) -> Self {
        let service = match config.team_db_path.as_ref() {
            Some(path) => AppService::with_runtime_components(
                config.instance.clone(),
                None,
                app_service::QuotaRuntime::production_in_memory(),
                path,
            )
            .expect("configured durable Team store must open and replay"),
            None => AppService::with_quota_runtime(
                config.instance.clone(),
                None,
                app_service::QuotaRuntime::production_in_memory(),
            ),
        };
        let service = Arc::new(service);
        if let Some(user_hooks) = config.user_hooks.as_ref() {
            service.set_user_hooks(Arc::clone(user_hooks));
        }
        Self::from_parts(service, config)
    }

    /// 正式宿主使用的可失败装配。持久 Team store 无法打开或重放时返回错误，
    /// 调用方不得降级到内存空状态。
    pub fn try_with_config(config: CoreRuntimeConfig) -> Result<Self, app_service::TeamError> {
        let service = match config.team_db_path.as_ref() {
            Some(path) => AppService::with_runtime_components(
                config.instance.clone(),
                None,
                app_service::QuotaRuntime::production_in_memory(),
                path,
            )?,
            None => AppService::with_quota_runtime(
                config.instance.clone(),
                None,
                app_service::QuotaRuntime::production_in_memory(),
            ),
        };
        let service = Arc::new(service);
        if let Some(user_hooks) = config.user_hooks.as_ref() {
            service.set_user_hooks(Arc::clone(user_hooks));
        }
        Ok(Self::from_parts(service, config))
    }

    /// 以持久化账本装配（P18-8）：打开（必要时创建）`ledger_path` 指向的
    /// SQLite Usage/Cost 账本并注入生产 Quota 运行时；启动时 replay 历史
    /// 用量进本地 Quota 缓存。run 进程写入的用量因此可在新进程读取，
    /// 且同一账本驱动 quota 聚合（run→ledger→usage/quota 单一事实源）。
    ///
    /// 打开失败（如 schema 版本不兼容）返回
    /// [`usage_ledger::UsageLedgerError`]，不静默降级、不丢历史记录；
    /// 调用方（CLI 宿主）必须 fail loud（生产不得回退内存版）。
    pub async fn with_persistent_ledger(
        instance: impl Into<String>,
        ledger_path: impl AsRef<std::path::Path>,
    ) -> Result<Self, usage_ledger::UsageLedgerError> {
        let instance = instance.into();
        let quota_runtime = app_service::QuotaRuntime::production_persistent(ledger_path).await?;
        let service = Arc::new(AppService::with_quota_runtime(
            instance.clone(),
            None,
            quota_runtime,
        ));
        Ok(Self::from_parts(
            service,
            CoreRuntimeConfig {
                instance,
                ..CoreRuntimeConfig::default()
            },
        ))
    }

    /// 生产装配（P18-4）：持久 CredentialPool + 持久 usage/cost 账本。
    ///
    /// 打开（必要时创建）`control_plane_path` 指向的 SQLite 库，迁移
    /// `credential_leases` 投影，以 [`SqliteLeaseProjection`] 组合层适配装配
    /// 持久池，并在启动时 `restore`（回收 Released/Expired 孤儿 lease、重建
    /// 崩溃遗留的 active 计数）；同时以 `ledger_path` 装配持久 Quota 运行时。
    ///
    /// 任一步失败返回 [`ControlPlaneError`]，调用方（CLI 宿主）必须 fail
    /// loud——生产不得回退内存池 / 内存账本，否则跨进程 lease 恢复与
    /// run→ledger→usage/quota 事实源被静默破坏。
    pub async fn with_persistent_control_plane(
        instance: impl Into<String>,
        ledger_path: impl AsRef<std::path::Path>,
        control_plane_path: impl AsRef<std::path::Path>,
    ) -> Result<Self, ControlPlaneError> {
        let instance = instance.into();
        Self::with_persistent_control_plane_config(
            CoreRuntimeConfig {
                instance,
                reclaim_interval: Some(Duration::from_secs(CREDENTIAL_RECLAIM_INTERVAL_SECS)),
                ..CoreRuntimeConfig::default()
            },
            ledger_path,
            control_plane_path,
        )
        .await
    }

    /// P17 + P18 生产组合入口：在持久 usage/control-plane 之外保留 Team DB、
    /// User Hooks 与 pump/hub 配置。Team 打开/重放失败同样 fail loud。
    pub async fn with_persistent_control_plane_config(
        mut config: CoreRuntimeConfig,
        ledger_path: impl AsRef<std::path::Path>,
        control_plane_path: impl AsRef<std::path::Path>,
    ) -> Result<Self, ControlPlaneError> {
        if config.reclaim_interval.is_none() {
            config.reclaim_interval = Some(Duration::from_secs(CREDENTIAL_RECLAIM_INTERVAL_SECS));
        }
        let instance = config.instance.clone();
        let control_plane = open_control_plane_runtime(control_plane_path.as_ref()).await?;
        let quota_runtime = app_service::QuotaRuntime::production_persistent(ledger_path).await?;
        let service = match config.team_db_path.as_ref() {
            Some(path) => AppService::with_runtime_components_and_credential_pool(
                instance.clone(),
                None,
                quota_runtime,
                path,
                control_plane.pool.clone(),
            )?,
            None => AppService::with_credential_pool(
                instance.clone(),
                None,
                quota_runtime,
                control_plane.pool.clone(),
            ),
        };
        service.set_account_repository(control_plane.account_repository.clone());
        let service = Arc::new(service);
        if let Some(user_hooks) = config.user_hooks.as_ref() {
            service.set_user_hooks(Arc::clone(user_hooks));
        }
        tracing::info!(
            instance = %instance,
            migrations = ?control_plane.migration_report.applied_versions,
            lease_migrations = ?control_plane.lease_migration_report.applied_versions,
            expired = control_plane.restore_report.expired,
            reclaimed = control_plane.restore_report.reclaimed,
            "credential pool restored from persistent projection"
        );
        Ok(Self::from_parts(service, config))
    }

    /// 注入 builder：以既有 AppService 装配（测试 / 嵌入场景复用，保持
    /// `AppService` 现有 API 与测试不变）。
    pub fn from_parts(service: Arc<AppService>, config: CoreRuntimeConfig) -> Self {
        let hub = Arc::new(EventHub::with_capacity(config.hub_capacity));
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let pump = spawn_event_pump(
            Arc::clone(&service),
            Arc::clone(&hub),
            config.pump_interval,
            shutdown_rx.clone(),
        );
        // P18-4 审查补救：生产持久池装配时同时启动周期 reclaim 任务，与
        // EventPump 共享同一 shutdown watch。reclaim 逻辑完全在
        // provider-control 池内（不复制状态机），本层只按固定间隔调用并
        // 告警失败，绝不回退内存池或静默吞错。
        let reclaim = match config.reclaim_interval {
            Some(interval) => service
                .credential_pool()
                .map(|pool| spawn_credential_reclaim(pool, interval, shutdown_rx.clone())),
            None => None,
        };
        Self {
            service,
            hub,
            pump,
            reclaim,
            shutdown: shutdown_tx,
        }
    }

    pub fn service(&self) -> &Arc<AppService> {
        &self.service
    }

    /// 注入的持久凭据池（P18-4；未注入持久控制面时为 `None`）。
    pub fn credential_pool(&self) -> Option<Arc<dyn CredentialPool>> {
        self.service.credential_pool()
    }

    pub fn hub(&self) -> &Arc<EventHub> {
        &self.hub
    }

    /// Provider 注册透传（正式宿主后续由 provider-runtime / auth-service 注入）。
    pub fn register_provider(&self, provider: Arc<dyn ModelProvider>) -> agent_domain::ProviderId {
        self.service.register_provider(provider)
    }

    /// 停止 EventPump 与周期 reclaim 任务（幂等；P18-4 审查补救）。
    pub fn shutdown(&self) {
        let _ = self.shutdown.send(true);
    }

    /// pump 任务是否已结束（shutdown 后为 true；测试用）。
    pub fn pump_finished(&self) -> bool {
        self.pump.is_finished()
    }

    /// 周期 reclaim 任务是否已结束（shutdown 后为 true；测试用）。
    pub fn reclaim_finished(&self) -> bool {
        self.reclaim
            .as_ref()
            .is_some_and(|handle| handle.is_finished())
    }
}

/// 打开控制面库并完成迁移 + restore（P18-4 装配公共部分）。
async fn open_control_plane_runtime(
    path: &std::path::Path,
) -> Result<ControlPlaneRuntime, ControlPlaneError> {
    let existed = path.exists();
    let database = DatabaseActor::open(path).await?;
    let migration_report = app_database::migrate_control_plane(&database, path, existed).await?;
    // 两套迁移命名空间（control_plane / lease）共享同一物理 SQLite 文件。两个迁移
    // 的备份路径只按 from_version 命名（不区分命名空间），若都备份且 from_version
    // 相同，后者的备份会覆盖前者，留下「control_plane 已迁、lease 未迁」的中间态
    // 备份——作为回滚基线是危险的。
    //
    // 备份职责规则：control_plane 先迁，若它真的迁移了（backup_path 非空），那份备份
    // 已是完整的迁移前状态（含 lease 旧表），足以回滚两者，lease 不再备份。只有当
    // control_plane 无需迁移（库已最新，backup_path 为 None）而 lease 仍待迁移时，才
    // 由 lease 负责生成迁移前备份——保证 lease 待迁时一定有一份回滚基线。
    let lease_needs_backup = existed && migration_report.backup_path.is_none();
    let lease_migration_report =
        app_database::migrate_lease(&database, path, lease_needs_backup).await?;

    // 从控制面表严格解析账号 / 凭据并加载到共享仓库（fail loud：未知枚举 / 坏行
    // 阻止启动）。生产 picker 基于该仓库选 Active 凭据，不再 hardcode legacy 种子。
    let repository = Arc::new(InMemoryProviderAccountRepository::new());
    load_provider_accounts_from_control_plane(&database, repository.as_ref()).await?;

    let projection = Arc::new(SqliteLeaseProjection::new(
        app_database::LeaseRowRepository::new(database.clone()),
    ));
    let picker = Arc::new(RepositoryCredentialPicker::new(repository.clone()));
    let pool: Arc<dyn CredentialPool> = Arc::new(InMemoryCredentialPool::build(
        PoolConfig::default(),
        Arc::new(SystemLeaseClock),
        projection,
        picker,
    ));
    let restore_report = pool.restore().await?;
    Ok(ControlPlaneRuntime {
        database,
        migration_report,
        lease_migration_report,
        restore_report,
        pool,
        account_repository: repository,
    })
}

/// 从 SQLite 控制面表（provider_accounts / credentials）严格解析账号与凭据，
/// 加载到共享 [InMemoryProviderAccountRepository]。
///
/// - 未知枚举值、负整数、类型不匹配、外键缺失 -> 立即返回错误（fail loud），
///   阻止启动；绝不静默丢弃坏行或回退 legacy 种子。
/// - credentials 表本就无明文列（ADR-014）：此处仅加载脱敏 secret_ref 定位
///   符（service, account），绝不读取或加载明文 secret。
/// - 凭据外键要求同 tenant 账号已存在，故先建账号、后建凭据。
async fn load_provider_accounts_from_control_plane(
    database: &DatabaseActor,
    repository: &InMemoryProviderAccountRepository,
) -> Result<(), ControlPlaneError> {
    let accounts: Vec<ProviderAccountRecord> = database
        .call(|connection| -> Result<Vec<ProviderAccountRecord>, String> {
            let mut statement = connection
                .prepare(
                    "SELECT account_id, tenant_id, provider_id, principal_id, display_name, routing_strategy, schema_version, priority, weight, max_concurrency, state FROM provider_accounts",
                )
                .map_err(|error| format!("prepare provider_accounts: {error}"))?;
            let mut rows = statement
                .query(())
                .map_err(|error| format!("query provider_accounts: {error}"))?;
            let mut records = Vec::new();
            while let Some(row) = rows
                .next()
                .map_err(|error| format!("iterate provider_accounts: {error}"))?
            {
                let account_id: String =
                    row.get(0).map_err(|e| format!("provider_accounts account_id: {e}"))?;
                let tenant_id: String =
                    row.get(1).map_err(|e| format!("provider_accounts tenant_id: {e}"))?;
                let provider_id: String =
                    row.get(2).map_err(|e| format!("provider_accounts provider_id: {e}"))?;
                let principal_id: String =
                    row.get(3).map_err(|e| format!("provider_accounts principal_id: {e}"))?;
                let display_name: String =
                    row.get(4).map_err(|e| format!("provider_accounts display_name: {e}"))?;
                let routing_strategy_str: String = row
                    .get(5)
                    .map_err(|e| format!("provider_accounts routing_strategy: {e}"))?;
                let schema_version: i64 =
                    row.get(6).map_err(|e| format!("provider_accounts schema_version: {e}"))?;
                let priority: i64 =
                    row.get(7).map_err(|e| format!("provider_accounts priority: {e}"))?;
                let weight: i64 =
                    row.get(8).map_err(|e| format!("provider_accounts weight: {e}"))?;
                let max_concurrency: i64 = row
                    .get(9)
                    .map_err(|e| format!("provider_accounts max_concurrency: {e}"))?;
                let state_str: String =
                    row.get(10).map_err(|e| format!("provider_accounts state: {e}"))?;

                let routing_strategy = RoutingStrategy::from_db_str(&routing_strategy_str)
                    .ok_or_else(|| {
                        format!(
                            "provider_accounts {tenant_id}/{account_id}: unknown routing_strategy `{routing_strategy_str}`"
                        )
                    })?;
                let state = AccountState::from_db_str(&state_str).ok_or_else(|| {
                    format!("provider_accounts {tenant_id}/{account_id}: unknown state `{state_str}`")
                })?;
                let schema_version = u32::try_from(schema_version).map_err(|_| {
                    format!(
                        "provider_accounts {tenant_id}/{account_id}: schema_version {schema_version} out of u32 range"
                    )
                })?;
                let priority = u32::try_from(priority).map_err(|_| {
                    format!(
                        "provider_accounts {tenant_id}/{account_id}: priority {priority} out of u32 range"
                    )
                })?;
                let weight = u32::try_from(weight).map_err(|_| {
                    format!(
                        "provider_accounts {tenant_id}/{account_id}: weight {weight} out of u32 range"
                    )
                })?;
                let max_concurrency = u64::try_from(max_concurrency).map_err(|_| {
                    format!(
                        "provider_accounts {tenant_id}/{account_id}: max_concurrency {max_concurrency} out of u64 range"
                    )
                })?;

                records.push(ProviderAccountRecord {
                    schema_version,
                    tenant_id: TenantId::new(tenant_id),
                    account_id: AccountId::new(account_id),
                    provider_id: ProviderId::new(provider_id),
                    principal_id: PrincipalId::new(principal_id),
                    display_name,
                    routing_strategy,
                    priority,
                    weight,
                    max_concurrency,
                    state,
                });
            }
            Ok(records)
        })
        .await
        .map_err(|error| ControlPlaneError::RepositoryLoad(format!("provider_accounts: {error}")))?
        .map_err(ControlPlaneError::RepositoryLoad)?;

    let credentials: Vec<CredentialMetadata> = database
        .call(|connection| -> Result<Vec<CredentialMetadata>, String> {
            let mut statement = connection
                .prepare(
                    "SELECT credential_id, tenant_id, account_id, provider_id, credential_kind, synthetic, schema_version, secret_ref_service, secret_ref_account, state, refresh_state, expires_at_ms FROM credentials",
                )
                .map_err(|error| format!("prepare credentials: {error}"))?;
            let mut rows = statement
                .query(())
                .map_err(|error| format!("query credentials: {error}"))?;
            let mut records = Vec::new();
            while let Some(row) = rows
                .next()
                .map_err(|error| format!("iterate credentials: {error}"))?
            {
                let credential_id: String =
                    row.get(0).map_err(|e| format!("credentials credential_id: {e}"))?;
                let tenant_id: String =
                    row.get(1).map_err(|e| format!("credentials tenant_id: {e}"))?;
                let account_id: String =
                    row.get(2).map_err(|e| format!("credentials account_id: {e}"))?;
                let provider_id: String =
                    row.get(3).map_err(|e| format!("credentials provider_id: {e}"))?;
                let credential_kind_str: String = row
                    .get(4)
                    .map_err(|e| format!("credentials credential_kind: {e}"))?;
                let synthetic: i64 =
                    row.get(5).map_err(|e| format!("credentials synthetic: {e}"))?;
                let schema_version: i64 =
                    row.get(6).map_err(|e| format!("credentials schema_version: {e}"))?;
                let secret_ref_service: String = row
                    .get(7)
                    .map_err(|e| format!("credentials secret_ref_service: {e}"))?;
                let secret_ref_account: String = row
                    .get(8)
                    .map_err(|e| format!("credentials secret_ref_account: {e}"))?;
                let state_str: String =
                    row.get(9).map_err(|e| format!("credentials state: {e}"))?;
                let refresh_state_str: String = row
                    .get(10)
                    .map_err(|e| format!("credentials refresh_state: {e}"))?;
                let expires_at_ms: Option<i64> = row
                    .get(11)
                    .map_err(|e| format!("credentials expires_at_ms: {e}"))?;

                let kind = match credential_kind_str.as_str() {
                    "api_key" => CredentialKind::ApiKey,
                    "oauth" => CredentialKind::OAuth,
                    "other" => CredentialKind::Other,
                    unknown => {
                        return Err(format!(
                            "credentials {tenant_id}/{credential_id}: unknown credential_kind `{unknown}`"
                        ))
                    }
                };
                let state = CredentialState::from_db_str(&state_str).ok_or_else(|| {
                    format!("credentials {tenant_id}/{credential_id}: unknown state `{state_str}`")
                })?;
                let refresh_state = RefreshState::from_db_str(&refresh_state_str).ok_or_else(|| {
                    format!(
                        "credentials {tenant_id}/{credential_id}: unknown refresh_state `{refresh_state_str}`"
                    )
                })?;
                let schema_version = u32::try_from(schema_version).map_err(|_| {
                    format!(
                        "credentials {tenant_id}/{credential_id}: schema_version {schema_version} out of u32 range"
                    )
                })?;
                let synthetic = match synthetic {
                    0 => false,
                    1 => true,
                    value => {
                        return Err(format!(
                            "credentials {tenant_id}/{credential_id}: synthetic {value} must be 0 or 1"
                        ))
                    }
                };
                let expires_at = match expires_at_ms {
                    None => None,
                    Some(millis) => Some(Timestamp::from_unix_millis(u64::try_from(millis).map_err(
                        |_| {
                            format!(
                                "credentials {tenant_id}/{credential_id}: expires_at_ms {millis} out of u64 range"
                            )
                        },
                    )?)),
                };

                records.push(CredentialMetadata {
                    schema_version,
                    tenant_id: TenantId::new(tenant_id),
                    credential_id: CredentialId::new(credential_id),
                    account_id: AccountId::new(account_id),
                    provider_id: ProviderId::new(provider_id),
                    kind,
                    synthetic,
                    secret_ref: SecretRef::new(secret_ref_service, secret_ref_account),
                    state,
                    expires_at,
                    refresh_state,
                });
            }
            Ok(records)
        })
        .await
        .map_err(|error| ControlPlaneError::RepositoryLoad(format!("credentials: {error}")))?
        .map_err(ControlPlaneError::RepositoryLoad)?;

    // 先建账号、后建凭据（凭据外键要求同 tenant 账号已存在）；仓库内 ID 冲突
    // 视为数据损坏 fail loud。
    for account in accounts {
        let tenant = account.tenant_id.clone();
        repository
            .create_account(&tenant, account)
            .await
            .map_err(|error| {
                ControlPlaneError::RepositoryLoad(format!("insert account: {error}"))
            })?;
    }
    for credential in credentials {
        let tenant = credential.tenant_id.clone();
        repository
            .create_credential(&tenant, credential)
            .await
            .map_err(|error| {
                ControlPlaneError::RepositoryLoad(format!("insert credential: {error}"))
            })?;
    }
    Ok(())
}

/// EventPump 任务：固定间隔轮询 app-service 的事件队列并发布到 Hub。
fn spawn_event_pump(
    service: Arc<AppService>,
    hub: Arc<EventHub>,
    interval: Duration,
    mut shutdown: watch::Receiver<bool>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(interval);
        loop {
            tokio::select! {
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() {
                        break;
                    }
                }
                _ = ticker.tick() => {}
            }
            let drained = service.drain_events();
            if !drained.is_empty() {
                debug!(
                    count = drained.len(),
                    "event pump publishing drained events"
                );
            }
            for event in drained {
                hub.publish(event);
            }
        }
    })
}

/// 周期 reclaim 任务（P18-4 审查补救）：以固定间隔调用持久池的
/// [`CredentialPool::reclaim_expired`]，随 shutdown 结束。
///
/// 失败只 `warn` 并等待下一周期——**不回退**内存池、不静默吞错、不在此复制
/// provider-control 的 lease 状态机（回收语义完全由池实现，本层只做调度）。
fn spawn_credential_reclaim(
    pool: Arc<dyn CredentialPool>,
    interval: Duration,
    mut shutdown: watch::Receiver<bool>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(interval);
        // 跳过首个立即 tick：启动时 `restore` 已回收一次，首个周期从 interval 后开始。
        ticker.tick().await;
        loop {
            tokio::select! {
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() {
                        break;
                    }
                }
                _ = ticker.tick() => {}
            }
            match pool.reclaim_expired().await {
                Ok(report) => {
                    if report.expired > 0 || report.reclaimed > 0 {
                        tracing::info!(
                            expired = report.expired,
                            reclaimed = report.reclaimed,
                            "periodic credential lease reclaim"
                        );
                    }
                }
                Err(error) => {
                    tracing::warn!(
                        error = %error,
                        "periodic credential lease reclaim failed; keeping persistent pool, \
                         retrying next interval"
                    );
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_domain::{CommandId, CoreInstanceId, SessionId, Timestamp, WorkspaceId};
    use async_trait::async_trait;
    use core_api::{
        ActorIdentity, AppCommand, AppCommandEnvelope, AppResponse, CommandSource, RunState,
        API_VERSION,
    };
    use provider_control::{
        AccountHealth, AccountId, AcquireRequest, CredentialLease, CredentialPool,
        InMemoryCredentialPool, LeaseGuard, LeaseId, LeaseOutcome, PoolError, ReclaimReport,
        ReleaseReceipt,
    };
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering as AtomicOrdering};
    use std::time::{Instant, SystemTime, UNIX_EPOCH};
    use subscription_hub::HubError;
    use test_support::{MockProvider, MockScript};

    /// 计数 / 可注入失败的 reclaim 测试池（P18-4 审查补救）：其余方法委托
    /// 内存池，`reclaim_expired` 计数并可配置失败，用于验证周期任务重复调用、
    /// 失败告警不回退、随 shutdown 停止。
    #[derive(Clone)]
    struct CountingReclaimPool {
        inner: Arc<InMemoryCredentialPool>,
        calls: Arc<AtomicU64>,
        fail_reclaim: Arc<AtomicBool>,
    }

    impl CountingReclaimPool {
        fn new() -> Self {
            Self {
                inner: Arc::new(InMemoryCredentialPool::new(1)),
                calls: Arc::new(AtomicU64::new(0)),
                fail_reclaim: Arc::new(AtomicBool::new(false)),
            }
        }

        fn calls(&self) -> u64 {
            self.calls.load(AtomicOrdering::SeqCst)
        }

        fn set_fail(&self, fail: bool) {
            self.fail_reclaim.store(fail, AtomicOrdering::SeqCst);
        }
    }

    #[async_trait]
    impl CredentialPool for CountingReclaimPool {
        async fn acquire(&self, req: AcquireRequest) -> Result<CredentialLease, PoolError> {
            self.inner.acquire(req).await
        }

        async fn acquire_guard(&self, req: AcquireRequest) -> Result<LeaseGuard, PoolError> {
            self.inner.acquire_guard(req).await
        }

        async fn release(
            &self,
            lease_id: LeaseId,
            outcome: LeaseOutcome,
        ) -> Result<ReleaseReceipt, PoolError> {
            self.inner.release(lease_id, outcome).await
        }

        fn active_count(&self, account: &AccountId) -> u64 {
            self.inner.active_count(account)
        }

        fn account_health(&self, account: &AccountId) -> AccountHealth {
            self.inner.account_health(account)
        }

        async fn reclaim_expired(&self) -> Result<ReclaimReport, PoolError> {
            self.calls.fetch_add(1, AtomicOrdering::SeqCst);
            if self.fail_reclaim.load(AtomicOrdering::SeqCst) {
                return Err(PoolError::NoCandidate);
            }
            self.inner.reclaim_expired().await
        }
    }

    static NEXT_COMMAND_ID: AtomicU64 = AtomicU64::new(0);

    fn now_timestamp() -> Timestamp {
        let millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_millis() as u64)
            .unwrap_or(0);
        Timestamp::from_unix_millis(millis)
    }

    fn command(instance: &CoreInstanceId, command: AppCommand) -> AppCommandEnvelope {
        AppCommandEnvelope {
            api_version: API_VERSION,
            command_id: CommandId::from(format!(
                "{}-{}",
                instance,
                NEXT_COMMAND_ID.fetch_add(1, AtomicOrdering::SeqCst) + 1
            )),
            source: CommandSource::Automation,
            identity: ActorIdentity::Automation {
                name: "core-runtime-test".into(),
            },
            expected_revision: None,
            idempotency_key: None,
            issued_at: now_timestamp(),
            command,
        }
    }

    fn is_terminal(state: &RunState) -> bool {
        matches!(
            state,
            RunState::Completed | RunState::Cancelled | RunState::Failed | RunState::Interrupted
        )
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn pump_publishes_run_events_with_contiguous_global_sequence() {
        let runtime = CoreRuntime::new("pump-test");
        runtime.register_provider(Arc::new(MockProvider::new(
            MockScript::new().text("hello from mock").complete(),
        )));
        let service = runtime.service();
        let instance = CoreInstanceId::from("pump-test");

        // 订阅必须在 RunStart 之前建立，避免错过事件。
        let mut subscription = runtime.hub().subscribe();

        // 打开默认 workspace（SessionCreate 要求 workspace 已登记）。
        let workspace_add = service.dispatch_envelope(command(
            &instance,
            AppCommand::WorkspaceAdd {
                root_path: std::env::current_dir()
                    .map(|path| path.to_string_lossy().into_owned())
                    .unwrap_or_else(|_| ".".into()),
            },
        ));
        let workspace_id = match workspace_add.response {
            AppResponse::Data(value) => WorkspaceId::from(
                value
                    .get("id")
                    .and_then(|id| id.as_str())
                    .expect("workspace id"),
            ),
            other => panic!("workspace add failed: {other:?}"),
        };
        let session_response = service.dispatch_envelope(command(
            &instance,
            AppCommand::SessionCreate {
                workspace_id: workspace_id.clone(),
                title: Some("core-runtime pump test".into()),
            },
        ));
        let session_id = match session_response.response {
            AppResponse::Data(value) => SessionId::from(
                value
                    .get("session_id")
                    .and_then(|id| id.as_str())
                    .expect("session id"),
            ),
            other => panic!("session create failed: {other:?}"),
        };

        let run_response = service.dispatch_envelope(command(
            &instance,
            AppCommand::RunStart {
                session_id: session_id.clone(),
                user_message: "run a mock task".into(),
                model: None,
                profile: None,
            },
        ));
        assert!(
            matches!(run_response.response, AppResponse::Accepted { .. }),
            "run start failed: {:?}",
            run_response.response
        );

        // 等待 RunChanged 终态，期间校验全局序列连续。
        let mut events: Vec<core_api::AppEventEnvelope> = Vec::new();
        let mut observed_run_id = None;
        let mut saw_terminal = false;
        for _ in 0..10_000 {
            match tokio::time::timeout(Duration::from_secs(5), subscription.recv()).await {
                Ok(Ok(event)) => {
                    if let Some(previous) = events.last() {
                        assert!(
                            event
                                .global_sequence
                                .is_immediately_after(previous.global_sequence),
                            "hub global sequence must stay contiguous"
                        );
                    }
                    if let core_api::AppEvent::RunChanged { run_id, state, .. } = &event.payload {
                        observed_run_id.get_or_insert_with(|| run_id.clone());
                        if is_terminal(state) {
                            saw_terminal = true;
                            break;
                        }
                    }
                    events.push(event);
                }
                Ok(Err(
                    HubError::Lagged { .. } | HubError::Empty | HubError::ReplayUnavailable { .. },
                )) => continue,
                Ok(Err(HubError::Closed)) | Err(_) => break,
            }
        }
        assert!(saw_terminal, "run never reached a terminal state");
        assert!(
            observed_run_id.is_some(),
            "run id must be observable from the event stream"
        );

        // Hub replay 与订阅一致：全局序列从 1 连续到 current。
        let current = runtime.hub().current();
        let replayed = runtime
            .hub()
            .replay(core_api::GlobalSequence(1), Some(current))
            .expect("replay");
        assert_eq!(replayed.len(), current.0 as usize);
        for pair in replayed.windows(2) {
            assert!(
                pair[1]
                    .global_sequence
                    .is_immediately_after(pair[0].global_sequence),
                "hub global sequence must be contiguous in replay"
            );
        }

        // run 已终态：supervisor 无活跃任务。
        assert_eq!(service.router().supervisor().stats().active, 0);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn shutdown_stops_the_event_pump() {
        let runtime = CoreRuntime::new("shutdown-test");
        assert!(!runtime.pump_finished());
        runtime.shutdown();
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(runtime.pump_finished());
    }

    #[tokio::test]
    async fn from_parts_keeps_app_service_api_unchanged() {
        let service = Arc::new(AppService::new("parts-test"));
        let runtime = CoreRuntime::from_parts(
            Arc::clone(&service),
            CoreRuntimeConfig {
                instance: "parts-test".into(),
                ..CoreRuntimeConfig::default()
            },
        );
        assert!(Arc::ptr_eq(runtime.service(), &service));
        assert_eq!(runtime.hub().capacity(), DEFAULT_HUB_CAPACITY);
        assert!(
            runtime.service().quota_runtime().is_none(),
            "from_parts must keep the injected AppService exactly as given"
        );
    }

    #[tokio::test]
    async fn with_config_defaults_to_in_memory_quota_runtime() {
        let runtime = CoreRuntime::new("quota-wiring-test");
        assert!(
            runtime.service().quota_runtime().is_some(),
            "default CoreRuntime must carry an in-memory QuotaRuntime (test/embedded only)"
        );
        runtime.shutdown();
    }

    // —— P17-1 回归：CoreRuntimeConfig.user_hooks 注入可达 ——

    struct NoProviders;
    impl app_service::ProviderResolver for NoProviders {
        fn resolve(
            &self,
            _id: &agent_domain::ProviderId,
        ) -> Option<Arc<dyn provider_api::ModelProvider>> {
            None
        }
    }

    #[derive(Clone)]
    struct DefaultProfiles(app_service::EvalProfile);
    impl app_service::EvalProfileResolver for DefaultProfiles {
        fn resolve(
            &self,
            _workspace_id: Option<&agent_domain::WorkspaceId>,
            profile: &str,
        ) -> Option<app_service::EvalProfile> {
            if profile.is_empty() || profile == "default" {
                Some(self.0.clone())
            } else {
                None
            }
        }
    }

    #[derive(Default)]
    struct MemSecret(std::sync::Mutex<std::collections::HashMap<String, String>>);
    impl auth_service::SecretBackend for MemSecret {
        fn store(
            &self,
            service: &str,
            account: &str,
            secret: &str,
        ) -> Result<(), auth_service::AuthError> {
            self.0
                .lock()
                .expect("mem secret")
                .insert(format!("{service}/{account}"), secret.to_string());
            Ok(())
        }
        fn get(&self, service: &str, account: &str) -> Result<String, auth_service::AuthError> {
            self.0
                .lock()
                .expect("mem secret")
                .get(&format!("{service}/{account}"))
                .cloned()
                .ok_or(auth_service::AuthError::NotFound)
        }
        fn delete(&self, service: &str, account: &str) -> Result<(), auth_service::AuthError> {
            let mut map = self.0.lock().expect("mem secret");
            if map.remove(&format!("{service}/{account}")).is_some() {
                Ok(())
            } else {
                Err(auth_service::AuthError::NotFound)
            }
        }
    }

    #[tokio::test]
    async fn user_hooks_config_injection_reaches_service() {
        // 默认配置不注入 hooks。
        let plain = CoreRuntime::new("hooks-none-test");
        assert!(
            !plain.service().user_hooks_active(),
            "default config must not install user hooks"
        );
        plain.shutdown();

        // with_config 注入的宿主必须到达 AppService（run loop 权威位点可调用）。
        let default_eval = app_service::EvalProfile {
            provider_id: agent_domain::ProviderId::from("default"),
            model: agent_domain::ModelId::from("default"),
            system_prompt: None,
            reasoning_effort: None,
            budget: None,
            tool_rules: agent_domain::ProfileToolRules::default(),
            isolation: agent_domain::ProfileIsolation::None,
        };
        let host = app_service::UserHookHost::new(app_service::UserHookHostOptions::new(
            Vec::new(),
            Arc::new(NoProviders),
            default_eval.clone(),
            Arc::new(DefaultProfiles(default_eval)),
            Arc::new(MemSecret::default()),
        ))
        .expect("host must construct");
        let runtime = CoreRuntime::with_config(CoreRuntimeConfig {
            user_hooks: Some(Arc::new(host)),
            ..CoreRuntimeConfig::default()
        });
        assert!(
            runtime.service().user_hooks_active(),
            "CoreRuntimeConfig.user_hooks must reach the AppService"
        );
        runtime.shutdown();
    }

    #[tokio::test]
    async fn persistent_ledger_survives_runtime_reopen() {
        // P18-8 跨“进程”回归：同一 ledger 文件 open→record→drop→reopen，
        // 新进程 usage 可读且配额聚合正确；重放幂等不重复累计。
        use usage_ledger::UsageRecord;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("usage-ledger.sqlite3");
        let scope = quota_scope_for(
            agent_domain::TenantId::new(core_api::DEFAULT_QUOTA_TENANT),
            core_api::DEFAULT_QUOTA_ACCOUNT,
            "mock",
        );

        // 第一个“进程”：持久装配 + 记账。
        {
            let runtime = CoreRuntime::with_persistent_ledger("p18-8-persist-a", &path)
                .await
                .expect("open persistent ledger");
            let quota = runtime
                .service()
                .quota_runtime()
                .expect("persistent runtime carries QuotaRuntime");
            quota
                .ledger
                .record(UsageRecord {
                    record_id: "core-runtime-persist-1".into(),
                    tenant_id: agent_domain::TenantId::new(core_api::DEFAULT_QUOTA_TENANT),
                    principal_id: agent_domain::PrincipalId::default(),
                    account_id: core_api::DEFAULT_QUOTA_ACCOUNT.to_string(),
                    session_id: agent_domain::SessionId::default(),
                    agent_id: agent_domain::AgentId::default(),
                    run_id: Some(agent_domain::RunId::from("run-persist-1")),
                    provider_id: agent_domain::ProviderId::from("mock"),
                    model_id: agent_domain::ModelId::from("mock-model"),
                    input_tokens: 100,
                    output_tokens: 50,
                    cache_read_tokens: 0,
                    cache_write_tokens: 0,
                    cost_micros: 0,
                    currency: "USD".into(),
                    occurred_at_ms: 1,
                    ..UsageRecord::default()
                })
                .await
                .expect("record into persistent ledger");
            runtime.shutdown();
        }

        // 第二个“进程”：重开同一文件；replay 后配额聚合正确。
        let runtime = CoreRuntime::with_persistent_ledger("p18-8-persist-b", &path)
            .await
            .expect("reopen persistent ledger");
        let quota = runtime
            .service()
            .quota_runtime()
            .expect("persistent runtime carries QuotaRuntime");
        let records = quota
            .ledger
            .query(&usage_ledger::UsageQuery::default())
            .await
            .unwrap();
        assert_eq!(records.len(), 1, "run 进程写入的用量必须跨进程可见");

        let request = quota_service::QuotaRequest {
            scope,
            window: quota_service::QuotaWindow::Overall,
            unit: quota_service::QuotaUnit::Token,
        };
        let read = quota
            .quota
            .read(&request, &agent_domain::CancellationToken::new())
            .await
            .expect("quota read");
        assert_eq!(
            read.snapshot.values.used,
            quota_service::QuotaMeasure::Exact(150),
            "同一账本驱动 quota 聚合，禁止第二套累计源"
        );
        runtime.shutdown();
    }

    #[tokio::test]
    async fn persistent_constructors_preserve_instance_name() {
        let dir = tempfile::tempdir().unwrap();
        let ledger = dir.path().join("usage-ledger.sqlite3");
        let runtime = CoreRuntime::with_persistent_ledger("ledger-instance", &ledger)
            .await
            .expect("open persistent ledger");
        assert_eq!(runtime.service().status().instance, "ledger-instance");
        runtime.shutdown();

        let control_ledger = dir.path().join("control-ledger.sqlite3");
        let control_plane = dir.path().join("control-plane.sqlite3");
        let runtime = CoreRuntime::with_persistent_control_plane(
            "control-instance",
            &control_ledger,
            &control_plane,
        )
        .await
        .expect("open persistent control plane");
        assert_eq!(runtime.service().status().instance, "control-instance");
        assert!(runtime.credential_pool().is_some());
        runtime.shutdown();
    }

    /// P18-4 审查补救：生产持久池周期 reclaim 按间隔重复调用，随 shutdown
    /// 结束（shutdown 后不再有任何回收调用）。
    #[tokio::test]
    async fn persistent_pool_periodic_reclaim_runs_until_shutdown() {
        let pool = CountingReclaimPool::new();
        let service = Arc::new(AppService::with_credential_pool(
            "reclaim-periodic",
            None,
            app_service::QuotaRuntime::production_in_memory(),
            Arc::new(pool.clone()),
        ));
        let runtime = CoreRuntime::from_parts(
            service,
            CoreRuntimeConfig {
                instance: "reclaim-periodic".into(),
                reclaim_interval: Some(Duration::from_millis(10)),
                ..CoreRuntimeConfig::default()
            },
        );

        // 至少两个周期：证明是周期重复调用，不是单次启动回收。
        let deadline = Instant::now() + Duration::from_secs(10);
        while pool.calls() < 2 && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert!(pool.calls() >= 2, "periodic reclaim must run repeatedly");
        assert!(
            !runtime.reclaim_finished(),
            "reclaim task must be alive before shutdown"
        );

        runtime.shutdown();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !runtime.reclaim_finished() && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert!(
            runtime.reclaim_finished(),
            "reclaim task must stop on shutdown"
        );
        let stopped_at = pool.calls();
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(
            pool.calls(),
            stopped_at,
            "no reclaim call may happen after shutdown"
        );
    }

    /// P18-4 审查补救：周期 reclaim 失败只告警并继续下一周期（不回退内存池、
    /// 不停止任务、不复制状态机）；恢复后继续正常调用。
    #[tokio::test]
    async fn persistent_pool_periodic_reclaim_warns_and_keeps_running_on_error() {
        let pool = CountingReclaimPool::new();
        pool.set_fail(true);
        let service = Arc::new(AppService::with_credential_pool(
            "reclaim-error",
            None,
            app_service::QuotaRuntime::production_in_memory(),
            Arc::new(pool.clone()),
        ));
        let runtime = CoreRuntime::from_parts(
            service,
            CoreRuntimeConfig {
                instance: "reclaim-error".into(),
                reclaim_interval: Some(Duration::from_millis(10)),
                ..CoreRuntimeConfig::default()
            },
        );

        // 失败路径下任务仍按周期重试（每次 Err 仅 warn）。
        let deadline = Instant::now() + Duration::from_secs(10);
        while pool.calls() < 2 && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert!(
            pool.calls() >= 2,
            "reclaim must keep retrying after errors (no fallback / no stop)"
        );
        assert!(!runtime.reclaim_finished());

        // 错误消失后同一池继续被周期调用（任务没有因错误退出或降级）。
        pool.set_fail(false);
        let before = pool.calls();
        let deadline = Instant::now() + Duration::from_secs(5);
        while pool.calls() <= before && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert!(
            pool.calls() > before,
            "reclaim must resume after errors clear"
        );

        runtime.shutdown();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !runtime.reclaim_finished() && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert!(runtime.reclaim_finished());
        let stopped_at = pool.calls();
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(pool.calls(), stopped_at);
    }

    fn quota_scope_for(
        tenant_id: agent_domain::TenantId,
        account: &str,
        provider: &str,
    ) -> quota_service::QuotaScope {
        quota_service::QuotaScope::new(
            tenant_id,
            quota_service::AccountId::new(account),
            agent_domain::ProviderId::from(provider),
            None,
        )
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn persistent_pool_acquire_missing_account_is_no_candidate() {
        let dir = tempfile::tempdir().unwrap();
        let ledger = dir.path().join("usage-ledger.sqlite3");
        let control_plane = dir.path().join("control-plane.sqlite3");
        let runtime = CoreRuntime::with_persistent_control_plane(
            "picker-non-legacy",
            &ledger,
            &control_plane,
        )
        .await
        .expect("open persistent control plane");
        let pool = runtime.credential_pool().expect("persistent pool");
        let error = pool
            .acquire(provider_control::AcquireRequest {
                tenant_id: agent_domain::TenantId::new("local/default"),
                principal_id: agent_domain::PrincipalId::new("local/user"),
                session_id: agent_domain::SessionId::new("session-picker"),
                agent_id: agent_domain::AgentId::new("agent-picker"),
                provider_id: Some(agent_domain::ProviderId::new("default")),
                account_id: Some(provider_control::AccountId::new("missing")),
                trace_id: Some("trace-picker".into()),
            })
            .await
            .expect_err("unknown account must not fall back to LegacyCredentialPicker");
        assert!(
            matches!(error, PoolError::NoCandidate),
            "expected NoCandidate from repository picker, got {error:?}"
        );
        runtime.shutdown();
    }

    /// 控制面账号 / 凭据从 SQLite 严格解析加载：自定义（非 legacy）账号与凭据
    /// 写入控制面库后，重启运行时仍能被 repository picker + 池 acquire 读到，
    /// 且与 legacy 默认账号共存（验证不再 hardcode legacy 种子、按表加载）。
    #[tokio::test(flavor = "multi_thread")]
    async fn control_plane_repository_loads_custom_account_credential_across_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let ledger = dir.path().join("usage-ledger.sqlite3");
        let control_plane = dir.path().join("control-plane.sqlite3");

        // 第一个「进程」：建表 + 写入自定义账号 / 凭据（tenant local/default 之外
        // 的独立 account / credential，证明加载器按表读取而非依赖 legacy 种子）。
        {
            let database = app_database::DatabaseActor::open(&control_plane)
                .await
                .expect("open control plane for seeding");
            app_database::migrate_control_plane(&database, &control_plane, false)
                .await
                .expect("migrate control plane for seeding");
            database
                .call(|connection| -> Result<(), String> {
                    connection
                        .execute_batch(
                            "INSERT INTO provider_accounts
                                (account_id, tenant_id, provider_id, principal_id, display_name,
                                 routing_strategy, schema_version, created_at_ms, priority,
                                 weight, max_concurrency, state)
                             VALUES
                                ('local/custom-acct', 'local/default', 'custom-prov', 'local/user',
                                 'Custom account', 'single_candidate', 2, 0, 0, 1, 1, 'active');
                             INSERT INTO credentials
                                (credential_id, tenant_id, account_id, provider_id, credential_kind,
                                 synthetic, schema_version, created_at_ms, secret_ref_service,
                                 secret_ref_account, state, refresh_state, expires_at_ms)
                             VALUES
                                ('custom-cred', 'local/default', 'local/custom-acct', 'custom-prov',
                                 'api_key', 0, 2, 0, 'custom-service', 'custom-account', 'active',
                                 'not_refreshable', NULL);",
                        )
                        .map_err(|error| format!("seed custom rows: {error}"))?;
                    Ok(())
                })
                .await
                .map_err(|error| ControlPlaneError::RepositoryLoad(format!("seed: {error}")))
                .expect("seed custom account/credential")
                .expect("seed custom account/credential result");
            drop(database);
        }

        // 第二个「进程」：重开同一控制面库；加载器从表解析出自定义凭据。
        let runtime = CoreRuntime::with_persistent_control_plane(
            "control-plane-reopen",
            &ledger,
            &control_plane,
        )
        .await
        .expect("reopen persistent control plane");
        let pool = runtime.credential_pool().expect("persistent pool");

        // 自定义账号：picker 必须选中 custom-cred（Active），lease 携带该凭据。
        let custom_lease = pool
            .acquire(provider_control::AcquireRequest {
                tenant_id: agent_domain::TenantId::new("local/default"),
                principal_id: agent_domain::PrincipalId::new("local/user"),
                session_id: agent_domain::SessionId::new("session-custom"),
                agent_id: agent_domain::AgentId::new("agent-custom"),
                provider_id: Some(agent_domain::ProviderId::new("custom-prov")),
                account_id: Some(provider_control::AccountId::new("local/custom-acct")),
                trace_id: Some("trace-custom".into()),
            })
            .await
            .expect("custom active credential must be picked");
        assert_eq!(
            custom_lease.credential_id.as_str(),
            "custom-cred",
            "picker must read the custom credential from the loaded repository"
        );

        // legacy 默认账号仍由迁移种子写入并加载（共存，非互斥）。
        let legacy_lease = pool
            .acquire(provider_control::AcquireRequest {
                tenant_id: agent_domain::TenantId::new("local/default"),
                principal_id: agent_domain::PrincipalId::new("local/user"),
                session_id: agent_domain::SessionId::new("session-legacy"),
                agent_id: agent_domain::AgentId::new("agent-legacy"),
                provider_id: Some(agent_domain::ProviderId::new("default")),
                account_id: Some(provider_control::AccountId::new("local/default")),
                trace_id: Some("trace-legacy".into()),
            })
            .await
            .expect("legacy default credential must still be loaded");
        assert_eq!(
            legacy_lease.credential_id.as_str(),
            "default",
            "legacy default seed remains loadable alongside custom rows"
        );
        runtime.shutdown();
    }

    /// 严格解析 fail loud：provider_accounts 出现未知 state 枚举时，启动必须失败，
    /// 不得静默丢弃坏行或回退 legacy 种子。
    #[tokio::test(flavor = "multi_thread")]
    async fn control_plane_repository_load_rejects_unknown_account_state() {
        let dir = tempfile::tempdir().unwrap();
        let ledger = dir.path().join("usage-ledger.sqlite3");
        let control_plane = dir.path().join("control-plane.sqlite3");

        {
            let database = app_database::DatabaseActor::open(&control_plane)
                .await
                .expect("open control plane for bad seed");
            app_database::migrate_control_plane(&database, &control_plane, false)
                .await
                .expect("migrate control plane for bad seed");
            database
                .call(|connection| -> Result<(), String> {
                    // CHECK 约束允许任意 TEXT；写入一个加载器无法反解的未知 state。
                    connection
                        .execute_batch(
                            "INSERT INTO provider_accounts
                                (account_id, tenant_id, provider_id, principal_id, display_name,
                                 routing_strategy, schema_version, created_at_ms, priority,
                                 weight, max_concurrency, state)
                             VALUES
                                ('local/bad-acct', 'local/default', 'bad-prov', 'local/user',
                                 'Bad account', 'single_candidate', 2, 0, 0, 1, 1, 'frozen');",
                        )
                        .map_err(|error| format!("seed bad row: {error}"))?;
                    Ok(())
                })
                .await
                .map_err(|error| ControlPlaneError::RepositoryLoad(format!("seed: {error}")))
                .expect("seed bad account row")
                .expect("seed bad account row result");
            drop(database);
        }

        let error = CoreRuntime::with_persistent_control_plane(
            "control-plane-bad-state",
            &ledger,
            &control_plane,
        )
        .await;
        // 不用 expect_err：它要求 Ok 变体 CoreRuntime: Debug（未实现）。改用 match
        // 直接取 Err，保持 fail-loud 断言。
        let error = match error {
            Ok(_) => panic!("unknown account state must block startup"),
            Err(error) => error,
        };
        assert!(
            matches!(error, ControlPlaneError::RepositoryLoad(_)),
            "expected RepositoryLoad on unknown enum, got {error:?}"
        );
    }
}
