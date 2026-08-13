//! CLI 与 GUI 共享的进程内应用服务门面（P13-1）。
//!
//! [`AppService`] 是唯一正式宿主（`pawork`）持有的门面：保留 legacy
//! [`AppService::dispatch`]（[`ServiceRequest`] → [`ServiceResponse`]）的同时，
//! 把真实命令/查询路由委托给统一 [`CommandRouter`]（`dispatch_envelope` /
//! `dispatch_query`），CLI 与 GUI 走同一入口、同一错误协议。

mod aggregate;
mod approval;
mod client_adapter;
mod error;
mod idempotency;
mod policy;
mod profile_resolver;
mod rate_limit;
mod router;
mod supervisor;
mod team;
mod user_hook;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;

use agent_domain::{ActorId, ArtifactId, CancellationToken, SessionId, WorkspaceId};
use core_api::{
    ActorIdentity, AppCommand, AppCommandEnvelope, AppQueryEnvelope, AppResponse,
    AppResponseEnvelope, CommandSource, API_VERSION,
};
use provider_api::ModelProvider;
use quota_service::QuotaAdapter;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub use aggregate::{
    AggregateError, AggregateState, ApprovalRecord, ApprovalStatus, ArtifactRecord, ProviderRecord,
    RunRecord, SessionRecord, Snapshot, TerminalRecord,
};
pub use approval::{ApprovalError, ApprovalRegistry, PendingApproval, Registration};
pub use client_adapter::ClientAdapterHost;
pub use error::AppServiceError;
pub use idempotency::{IdempotencyCheck, IdempotencyError, IdempotencyStats, IdempotencyStore};
pub use policy::{PolicyGateError, RoutingTenantPolicyAdapter, TenantPolicyGate};
pub use profile_resolver::{
    DenyAllModelOverridePolicy, IsolationCapability, ModelLanding, ModelOverrideDecision,
    ModelOverridePolicy, ModelOverrideRequest, ProductionModelOverridePolicy, ProfileResolveError,
    ResolvedRunProfile, RunProfileResolver, SandboxIsolationCapability,
};
pub use rate_limit::{DeltaKind, RateLimiter, RateLimiterStats};
pub use router::{source_name, CommandRouter, RouterConfig};
pub use supervisor::{
    CancelOutcome, RunRequest, RunSupervisor, RunSupervisorStats, SuperviseError,
};
pub use team::{SqliteTeamStore, TeamHost};
pub use teams::TeamError;
pub use tenant_service::{
    IdentityContext, IdentityError, IdentityResolver, InMemoryTenantPolicyEngine,
    LocalIdentityResolver, Permission, TenantPolicy,
};
pub use user_hook::{
    hook_config_from_resource, BackendSecretResolver, CanonicalJudge, EvalProfile,
    EvalProfileResolver, HookMcpApproval, HookPolicyGate, HookRunContext,
    HookWorkspaceTrustResolver, HttpHookExecutor, McpToolInvokerHost, ProviderResolver,
    SandboxCommandExecutor, SqliteHookAuditSink, StaticHookRunContext, TokioAsyncRunner,
    UserHookHost, UserHookHostOptions,
};

use crate::error::now_timestamp;

use artifact_store::{ArtifactStore, BlobId};

/// Quota 运行时（P14-8）：进程内共享的唯一 UsageLedger + QuotaService + Clock。
///
/// 这是 app-service 层唯一的用量计数源：成功 run 完成后向 [`Self::ledger`]
/// 追加幂等 [`usage_ledger::UsageRecord`]；[`Self::quota`] 读取该 ledger 产出
/// canonical 额度视图。不再引入第二个计数器或存储。查询路径只读缓存，
/// 不触发网络抓取。
#[derive(Clone)]
pub struct QuotaRuntime {
    /// 进程内唯一共享的用量账本（成功 run 追加 + 查询读取的同一实例）。
    pub ledger: Arc<dyn usage_ledger::UsageLedger>,
    /// Quota 服务（已注册适配器，读取共享 ledger）。
    pub quota: Arc<quota_service::service::QuotaService>,
    /// 私有持有的本地 Ledger 适配器：同一 Arc 同时 serve 查询注册表、
    /// ledger reconciler 与 [`Self::refresh_local_cache`] 的本地 fetch，
    /// 绝无第二套账本。
    adapter: Arc<quota_service::ledger::LedgerQuotaAdapter>,
    /// Quota 时钟（与适配器共享，测试可注入 MutableQuotaClock）。
    pub clock: Arc<dyn quota_service::service::QuotaClock>,
}

impl std::fmt::Debug for QuotaRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QuotaRuntime")
            .field("ledger", &"Arc<dyn UsageLedger>")
            .field("quota", &"Arc<QuotaService>")
            .field("adapter", &"Arc<LedgerQuotaAdapter>")
            .finish_non_exhaustive()
    }
}

/// 本地对账单键失败（可诊断）：scope + window + unit 精确定位失败的缓存键。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalReconcileFailure {
    pub scope: quota_service::QuotaScope,
    pub window: quota_service::QuotaWindow,
    pub unit: quota_service::QuotaUnit,
    pub error: quota_service::QuotaError,
}

impl QuotaRuntime {
    /// 组装共享运行时：以同一 ledger 注册 [`quota_service::LedgerQuotaAdapter`]，
    /// 适配任意 scope（适配器内部按 scope 过滤）。同一 Arc 同时挂为 ledger
    /// reconciler（[`quota_service::service::QuotaService::set_ledger_reconciler`]），
    /// 并私有持有供 [`Self::refresh_local_cache`] 复用；注册表、对账与本地
    /// 缓存共享同一适配器实例、同一账本，绝无第二套账本。
    pub fn new(
        ledger: Arc<dyn usage_ledger::UsageLedger>,
        clock: Arc<dyn quota_service::service::QuotaClock>,
    ) -> Arc<Self> {
        let quota = Arc::new(quota_service::service::QuotaService::new(Arc::clone(
            &clock,
        )));
        let adapter = Arc::new(quota_service::ledger::LedgerQuotaAdapter::new(
            Arc::clone(&ledger),
            Arc::clone(&clock),
        ));
        let registered: Arc<dyn quota_service::QuotaAdapter> = adapter.clone();
        quota.register(quota_service::service::ScopeMatch::any(), registered);
        quota.set_ledger_reconciler(Arc::clone(&adapter));
        Arc::new(Self {
            ledger,
            quota,
            adapter,
            clock,
        })
    }

    /// 测试/组合构造：注入外部 QuotaService（如 counting-adapter 集成测试
    /// 自建的 mock 注册表）。私有本地 adapter 仍基于传入的同一 ledger + clock
    /// 派生，`refresh_local_cache` 因此与 [`Self::new`] 行为一致。
    ///
    /// 与 [`Self::new`] 不同，本构造不触碰注册表与 reconciler——注册/对账
    /// 接线由调用方按需完成，保证测试只运行自己注册的 adapter。
    pub fn from_parts(
        ledger: Arc<dyn usage_ledger::UsageLedger>,
        quota: Arc<quota_service::service::QuotaService>,
        clock: Arc<dyn quota_service::service::QuotaClock>,
    ) -> Arc<Self> {
        let adapter = Arc::new(quota_service::ledger::LedgerQuotaAdapter::new(
            Arc::clone(&ledger),
            Arc::clone(&clock),
        ));
        Arc::new(Self {
            ledger,
            quota,
            adapter,
            clock,
        })
    }

    /// 进程内构造（P14-8）：新建共享
    /// [`usage_ledger::InMemoryUsageLedger`] 与
    /// [`quota_service::service::SystemQuotaClock`]，同一 ledger 同时服务
    /// 记账（成功 run 追加）与额度查询（适配器派生）。进程内存活期间唯一
    /// 累计源；跨进程持久化使用 [`Self::production_persistent`]。
    ///
    /// **仅供测试 / 嵌入式便捷使用，不是生产构造**：内存账本不跨进程，
    /// 生产 CLI 必须走 [`Self::production_persistent`]，禁止把本构造用作
    /// 生产累计源（P18-8 review：生产 run→ledger→usage/quota 单一事实源，
    /// 不得回退到进程内第二套累计）。
    ///
    /// 唯一注册的适配器是本地 ledger 派生（[`quota_service::AdapterKind::LocalLedger`]），
    /// 构造与空查询均不触发任何网络。
    pub fn production_in_memory() -> Arc<Self> {
        Self::production_with_ledger(Arc::new(usage_ledger::InMemoryUsageLedger::new()))
    }

    /// 生产持久化构造（P18-8）：打开（必要时创建）指定路径的 SQLite 账本，
    /// 同一 ledger 同时服务记账（成功 run 追加）与额度查询（适配器派生），
    /// run 进程写入后，新进程打开同一文件即可读取并正确聚合——run→ledger→
    /// usage/quota 单一事实源，禁止第二套累计计数器。
    ///
    /// 打开时校验 schema 版本，不兼容（如更高版本）返回
    /// [`usage_ledger::UsageLedgerError::Storage`]，不静默迁移、不丢历史。
    /// 启动时调用 [`Self::replay_local_cache`] 把历史记录回放进本地 Quota
    /// 缓存；异步 canonical quota 读取仍由注册的 Ledger adapter 直读同一
    /// 账本，同步 overview 则只读该缓存。回放遇存储 / 解码错误整体失败返回
    ///（fail-closed），不吞错启动。
    pub async fn production_persistent(
        path: impl AsRef<std::path::Path>,
    ) -> Result<Arc<Self>, usage_ledger::UsageLedgerError> {
        let ledger: Arc<dyn usage_ledger::UsageLedger> =
            Arc::new(usage_ledger::SqliteUsageLedger::open(path)?);
        let runtime = Self::production_with_ledger(ledger);
        runtime.replay_local_cache().await?;
        Ok(runtime)
    }

    /// 生产装配公共部分：[`Self::production`] 与
    /// [`Self::production_persistent`] 共用同一系统时钟与注册接线，保证
    /// 两种构造行为一致（同一账本实例同时服务记账与查询）。
    fn production_with_ledger(ledger: Arc<dyn usage_ledger::UsageLedger>) -> Arc<Self> {
        let clock: Arc<dyn quota_service::service::QuotaClock> =
            Arc::new(quota_service::service::SystemQuotaClock);
        Self::new(ledger, clock)
    }

    /// 启动回放（P18-8）：读取持久账本全部历史记录，按
    /// (tenant, account, credential, provider, model, currency) 去重，
    /// 逐条 [`Self::refresh_local_cache`] 补本地 Quota 缓存（同一账本聚合，
    /// 不引入第二套计数）。单条缓存刷新失败仅告警不中断；**账本查询 / 行
    /// 解码错误整体失败返回**（fail-closed，绝不吞错为空集）。currency 是
    /// Cost 缓存键的一部分；同 scope 多币种必须分别回放，不能互相覆盖。
    pub async fn replay_local_cache(&self) -> Result<(), usage_ledger::UsageLedgerError> {
        let records = self
            .ledger
            .query(&usage_ledger::UsageQuery::default())
            .await?;
        let mut seen: std::collections::BTreeSet<(
            String,
            String,
            Option<String>,
            String,
            String,
            String,
        )> = std::collections::BTreeSet::new();
        let mut failures = 0usize;
        for record in records {
            let key = (
                record.tenant_id.as_str().to_string(),
                record.account_id.clone(),
                record.credential_id.clone(),
                record.provider_id.as_str().to_string(),
                record.model_id.as_str().to_string(),
                record.currency.clone(),
            );
            if !seen.insert(key) {
                continue;
            }
            if let Err(fails) = self.refresh_local_cache(&record).await {
                failures += fails.len();
                for failure in fails {
                    tracing::warn!(
                        tenant = %failure.scope.tenant_id,
                        account = %failure.scope.account_id,
                        error = %failure.error,
                        "ledger replay cache refresh failed",
                    );
                }
            }
        }
        if failures > 0 {
            tracing::warn!(
                failures,
                "ledger replay completed with cache refresh failures"
            );
        }
        Ok(())
    }

    /// 本地 Ledger 对账/缓存（P14-8）：给定一条已成功写入账本的
    /// [`usage_ledger::UsageRecord`]，按其完整 tenant/account/credential/
    /// provider/model scope 与 tenant/account/provider 账户级聚合 scope
    /// （credential/model 均为 `None`），为每个去重 scope 的 Overall /
    /// Rolling5h / Weekly / Monthly 的 Token 与该记录 `currency` 的 Cost，
    /// 直接调用本地
    /// [`quota_service::ledger::LedgerQuotaAdapter::fetch`]（只读同一账本，
    /// 不触发任何远端适配器或网络），再把每个快照
    /// [`quota_service::service::QuotaService::publish_local_snapshot`]
    /// 进进程内缓存。键之间相互独立：单个键失败不中断其余键；全部成功返回
    /// `Ok(())`，任一失败返回完整可诊断列表（scope + window + unit + 错误）。
    ///
    /// 复用 [`Self::new`] 保留的同一 adapter Arc（不每次重建），与注册表/
    /// 对账路径读写同一个账本实例。
    pub async fn refresh_local_cache(
        &self,
        record: &usage_ledger::UsageRecord,
    ) -> Result<(), Vec<LocalReconcileFailure>> {
        let full_scope = quota_service::QuotaScope {
            tenant_id: record.tenant_id.clone(),
            account_id: quota_service::AccountId::new(record.account_id.clone()),
            credential_id: record.credential_id.clone(),
            provider_id: record.provider_id.clone(),
            model_id: Some(record.model_id.clone()),
        };
        let account_scope = quota_service::QuotaScope {
            tenant_id: record.tenant_id.clone(),
            account_id: quota_service::AccountId::new(record.account_id.clone()),
            credential_id: None,
            provider_id: record.provider_id.clone(),
            model_id: None,
        };
        let mut scopes = vec![full_scope, account_scope];
        scopes.sort();
        scopes.dedup();
        let cancel = CancellationToken::new();
        let mut failures = Vec::new();
        for scope in scopes {
            for window in [
                quota_service::QuotaWindow::Overall,
                quota_service::QuotaWindow::Rolling5h,
                quota_service::QuotaWindow::Weekly,
                quota_service::QuotaWindow::Monthly,
            ] {
                for unit in [
                    quota_service::QuotaUnit::Token,
                    quota_service::QuotaUnit::Cost {
                        currency: record.currency.clone(),
                    },
                ] {
                    let request = quota_service::QuotaRequest {
                        scope: scope.clone(),
                        window,
                        unit: unit.clone(),
                    };
                    match self.adapter.fetch(&request, None, &cancel).await {
                        Ok(snapshot) => {
                            // P14：publish_local_snapshot 直接返回 QuotaError，
                            // 不再有包装的 .error 字段。
                            if let Err(error) = self.quota.publish_local_snapshot(snapshot) {
                                failures.push(LocalReconcileFailure {
                                    scope: scope.clone(),
                                    window,
                                    unit: unit.clone(),
                                    error,
                                });
                            }
                        }
                        Err(error) => failures.push(LocalReconcileFailure {
                            scope: scope.clone(),
                            window,
                            unit: unit.clone(),
                            error,
                        }),
                    }
                }
            }
        }
        if failures.is_empty() {
            Ok(())
        } else {
            Err(failures)
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LifecycleState {
    Starting,
    Ready,
    ShuttingDown,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ServiceStatus {
    pub version: String,
    pub instance: String,
    pub process_id: u32,
    pub lifecycle: LifecycleState,
    pub uptime_millis: u64,
    pub commands_handled: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DoctorCheck {
    pub name: String,
    pub ok: bool,
    pub detail: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DoctorReport {
    pub ok: bool,
    pub checks: Vec<DoctorCheck>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ServiceOperation {
    Serve,
    Run {
        workspace: Option<String>,
        prompt: Option<String>,
        keep_serving: bool,
    },
    Shell,
    Watch,
    Status,
    Shutdown,
    Doctor,
    Placeholder {
        command: String,
        arguments: Vec<String>,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct ServiceRequest {
    pub source: CommandSource,
    pub operation: ServiceOperation,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ServiceResponse {
    pub ok: bool,
    pub kind: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub data: Value,
}

/// Artifact 流式读取结果（P13-8）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArtifactReadResult {
    /// 聚合记录的 artifact 总字节数。
    pub byte_length: u64,
    /// `[offset, offset + data.len())` 的 payload 分片（`limit == 0` 时读到文件尾）。
    pub data: Vec<u8>,
    /// 本片是否已覆盖文件尾（或 `offset` 已超尾）。
    pub eof: bool,
}

struct State {
    lifecycle: LifecycleState,
    commands_handled: u64,
    sources: BTreeMap<String, u64>,
}

/// 单进程内共享的应用服务门面。CLI 直接持有此对象，不通过 socket 回连自身。
pub struct AppService {
    instance: String,
    started_at: Instant,
    state: Mutex<State>,
    router: CommandRouter,
    artifact_store: Option<Arc<ArtifactStore>>,
    quota_runtime: Option<Arc<QuotaRuntime>>,
    /// P17-6 Team 协作宿主：durable store + 重启重放 + typed EventHub 桥。
    team_host: TeamHost,
    tenant_policy: Arc<TenantPolicyGate>,
}

impl AppService {
    pub fn new(instance: impl Into<String>) -> Self {
        Self::build(
            instance,
            None,
            None,
            None,
            Arc::new(LocalIdentityResolver),
            None,
        )
        .expect("in-memory Team store construction must succeed")
    }

    /// 携带内容寻址 Blob Store 构造（P13-8 接线）；`AppService::new` 等价于
    /// store 为 `None`（此时 `artifact_read` 返回 `Unavailable`）。
    pub fn with_artifact_store(instance: impl Into<String>, store: Arc<ArtifactStore>) -> Self {
        Self::build(
            instance,
            Some(store),
            None,
            None,
            Arc::new(LocalIdentityResolver),
            None,
        )
        .expect("in-memory Team store construction must succeed")
    }

    /// 携带共享 Quota 运行时构造（P14-8）：注入唯一 ledger + QuotaService。
    /// 既有 [`AppService::new`] / [`AppService::with_artifact_store`] 保持兼容
    /// （quota 为 `None`，不记账、不查额度）。
    pub fn with_quota_runtime(
        instance: impl Into<String>,
        store: Option<Arc<ArtifactStore>>,
        quota_runtime: Arc<QuotaRuntime>,
    ) -> Self {
        Self::build(
            instance,
            store,
            Some(quota_runtime),
            None,
            Arc::new(LocalIdentityResolver),
            None,
        )
        .expect("in-memory Team store construction must succeed")
    }

    /// 携带 durable Team 事件存储构造（P17-6）：Team 命令先落盘 SQLite，
    /// 重启时从同一路径全量重放重建状态。未指定路径的构造（[`Self::new`] 等）
    /// 使用内存 SQLite（完整 SQL 语义，无跨进程持久性）。
    pub fn with_team_db(
        instance: impl Into<String>,
        team_db_path: impl Into<PathBuf>,
    ) -> Result<Self, teams::TeamError> {
        Self::build(
            instance,
            None,
            None,
            Some(team_db_path.into()),
            Arc::new(LocalIdentityResolver),
            None,
        )
    }

    /// 生产组合入口：同时注入 artifact/quota 运行时与 durable Team DB。
    /// 显式路径打开或重放失败时返回错误，正式宿主必须终止启动，绝不降级为空状态。
    pub fn with_runtime_components(
        instance: impl Into<String>,
        store: Option<Arc<ArtifactStore>>,
        quota_runtime: Arc<QuotaRuntime>,
        team_db_path: impl Into<PathBuf>,
    ) -> Result<Self, teams::TeamError> {
        Self::build(
            instance,
            store,
            Some(quota_runtime),
            Some(team_db_path.into()),
            Arc::new(LocalIdentityResolver),
            None,
        )
    }

    /// 携带共享 CredentialPool 构造（P18-4）：注入后每个 run attempt 在
    /// provider 调用前异步 acquire 并持有 LeaseGuard 至终态，usage 归属的
    /// account/credential 来自真实 lease。既有 [`AppService::new`] /
    /// [`AppService::with_quota_runtime`] 保持兼容（pool 为 `None`，走 legacy
    /// 过渡路径）。
    pub fn with_credential_pool(
        instance: impl Into<String>,
        store: Option<Arc<ArtifactStore>>,
        quota_runtime: Arc<QuotaRuntime>,
        pool: Arc<dyn provider_control::CredentialPool>,
    ) -> Self {
        let service = Self::build(
            instance,
            store,
            Some(quota_runtime),
            None,
            Arc::new(LocalIdentityResolver),
            None,
        )
        .expect("in-memory Team store construction must succeed");
        service.router.set_credential_pool(pool);
        service
    }

    /// 携带自定义租户策略引擎构造（P18-9）：引擎注入 CommandRouter 唯一
    /// dispatch / dispatch_query 边界，AppService 与 router 共享同一 policy
    /// gate（不双记）。既有构造保持默认 `local/default` 兼容策略。
    pub fn with_tenant_policy(
        instance: impl Into<String>,
        engine: Arc<dyn tenant_service::TenantPolicyEngine>,
    ) -> Self {
        Self::build_with_policy_and_resolver(
            instance,
            None,
            None,
            None,
            Arc::new(LocalIdentityResolver),
            Some(engine),
        )
        .expect("in-memory Team store construction must succeed")
    }

    /// 生产接线（P18-9）：同一 identity resolver 与同一 tenant policy engine
    /// 注入 router 唯一公开边界；facade 的查询 / 管理接口复用 router 的同一
    /// policy gate，裁决只记录一次。
    pub fn with_identity_resolver_and_tenant_policy(
        instance: impl Into<String>,
        identity_resolver: Arc<dyn IdentityResolver>,
        engine: Arc<dyn tenant_service::TenantPolicyEngine>,
    ) -> Self {
        Self::build_with_policy_and_resolver(
            instance,
            None,
            None,
            None,
            identity_resolver,
            Some(engine),
        )
        .expect("in-memory Team store construction must succeed")
    }

    /// 最小生产组合（P17+P18）：同时注入 quota 运行时、durable Team DB 与
    /// CredentialPool——core-runtime 在同一入口同时装配 P17 Team 宿主与 P18
    /// credential lease/usage 链。内部复用 [`Self::build`] 装配 team_host /
    /// tenant_policy，再 `set_credential_pool` 注入 pool。显式 Team DB 路径
    /// 打开或重放失败时返回错误，正式宿主必须终止启动，绝不降级。
    pub fn with_runtime_components_and_credential_pool(
        instance: impl Into<String>,
        store: Option<Arc<ArtifactStore>>,
        quota_runtime: Arc<QuotaRuntime>,
        team_db_path: impl Into<PathBuf>,
        pool: Arc<dyn provider_control::CredentialPool>,
    ) -> Result<Self, teams::TeamError> {
        let service = Self::build(
            instance,
            store,
            Some(quota_runtime),
            Some(team_db_path.into()),
            Arc::new(LocalIdentityResolver),
            None,
        )?;
        service.router.set_credential_pool(pool);
        Ok(service)
    }

    fn build(
        instance: impl Into<String>,
        artifact_store: Option<Arc<ArtifactStore>>,
        quota_runtime: Option<Arc<QuotaRuntime>>,
        team_db_path: Option<PathBuf>,
        identity_resolver: Arc<dyn IdentityResolver>,
        tenant_policy_engine: Option<Arc<dyn tenant_service::TenantPolicyEngine>>,
    ) -> Result<Self, teams::TeamError> {
        Self::build_with_policy_and_resolver(
            instance,
            artifact_store,
            quota_runtime,
            team_db_path,
            identity_resolver,
            tenant_policy_engine,
        )
    }

    fn build_with_policy_and_resolver(
        instance: impl Into<String>,
        artifact_store: Option<Arc<ArtifactStore>>,
        quota_runtime: Option<Arc<QuotaRuntime>>,
        team_db_path: Option<PathBuf>,
        identity_resolver: Arc<dyn IdentityResolver>,
        tenant_policy_engine: Option<Arc<dyn tenant_service::TenantPolicyEngine>>,
    ) -> Result<Self, teams::TeamError> {
        let instance = instance.into();
        let router = CommandRouter::with_tenant_policy(
            RouterConfig {
                instance: instance.clone(),
                ..RouterConfig::default()
            },
            identity_resolver,
            tenant_policy_engine.unwrap_or_else(|| Arc::new(InMemoryTenantPolicyEngine::default())),
        );
        if let Some(runtime) = quota_runtime.as_ref() {
            router.set_quota_runtime(Arc::clone(runtime));
        }
        let team_host = team::open_durable(router.team_sink(), team_db_path)?;
        let tenant_policy = Arc::clone(router.tenant_policy());
        Ok(Self {
            instance: instance.clone(),
            started_at: Instant::now(),
            state: Mutex::new(State {
                lifecycle: LifecycleState::Starting,
                commands_handled: 0,
                sources: BTreeMap::new(),
            }),
            router,
            artifact_store,
            quota_runtime,
            team_host,
            tenant_policy,
        })
    }

    /// Team 协作命令面 / 查询面（P17-6）。
    pub fn teams(&self) -> &teams::TeamService {
        self.team_host.service()
    }

    /// Team 宿主（durable store + 重放状态；测试 / 自省）。
    pub fn team_host(&self) -> &TeamHost {
        &self.team_host
    }

    /// 注入共享 User Hooks 宿主（P17-1）：run 的 pre-prompt / pre-tool 权威
    /// 位点回灌 hooks 结果。幂等：同一实例重复注入为 no-op；未注入时行为
    /// 与既往完全一致。
    pub fn set_user_hooks(&self, host: Arc<UserHookHost>) {
        self.router.set_user_hooks(host);
    }

    /// 注入 run 的 workspace roots（P17-1）：run loop 的 pre-prompt / pre-tool
    /// 权威位点把它传给 UserHookHost。与 [`Self::set_user_hooks`] 同生命周期。
    pub fn set_workspace_roots(&self, roots: Vec<PathBuf>) {
        self.router.set_workspace_roots(roots);
    }

    /// 是否已注入共享 User Hooks 宿主（宿主装配 / 诊断用）。
    pub fn user_hooks_active(&self) -> bool {
        self.router.user_hooks_active()
    }

    /// 注入 P17-5 主 run profile 解析器（生产 ResourceLoader 装配）。未注入时
    /// RunStart 携带 profile 名一律 fail-closed。幂等。
    pub fn set_profile_resolver(
        &self,
        resolver: Arc<dyn crate::profile_resolver::RunProfileResolver>,
    ) {
        self.router.set_profile_resolver(resolver);
    }

    /// 注入 P17-5 模型覆盖授权策略（生产装配
    /// [`ProductionModelOverridePolicy`]：本机交互 + LocalUser 放行；未注入
    /// 时缺省 DenyAll fail-closed）。幂等。
    pub fn set_model_override_policy(
        &self,
        policy: Arc<dyn crate::profile_resolver::ModelOverridePolicy>,
    ) {
        self.router.set_model_override_policy(policy);
    }

    /// 注入 P17-5 后台任务管理器：background=true 的 run 经它注册 / 启动 /
    /// 完成 / 取消一个 TaskKind::Agent。幂等。
    pub fn set_task_manager(&self, manager: Arc<task_manager::TaskManager>) {
        self.router.set_task_manager(manager);
    }

    /// legacy 入口：`ServiceOperation` → `ServiceResponse`。
    pub fn dispatch(&self, request: ServiceRequest) -> ServiceResponse {
        let source = source_name(&request.source);
        {
            let mut state = self.state();
            state.commands_handled = state.commands_handled.saturating_add(1);
            *state.sources.entry(source.to_owned()).or_default() += 1;
        }

        match request.operation {
            ServiceOperation::Serve => {
                self.state().lifecycle = LifecycleState::Ready;
                ServiceResponse {
                    ok: true,
                    kind: "serve".into(),
                    message: format!("Pawork Core instance '{}' is ready", self.instance),
                    data: serde_json::to_value(self.status()).expect("status is serializable"),
                }
            }
            ServiceOperation::Run {
                workspace,
                prompt,
                keep_serving,
            } => self.run_operation(request.source, workspace, prompt, keep_serving),
            ServiceOperation::Shell => response("shell", "interactive shell is ready"),
            ServiceOperation::Watch => response("watch", "event watch route is ready"),
            ServiceOperation::Status => ServiceResponse {
                ok: true,
                kind: "status".into(),
                message: "Core status".into(),
                data: serde_json::to_value(self.status()).expect("status is serializable"),
            },
            ServiceOperation::Shutdown => {
                self.state().lifecycle = LifecycleState::ShuttingDown;
                response("shutdown", "Core shutdown requested")
            }
            ServiceOperation::Doctor => {
                let report = self.doctor();
                ServiceResponse {
                    ok: report.ok,
                    kind: "doctor".into(),
                    message: if report.ok {
                        "all available host checks passed".into()
                    } else {
                        "one or more checks failed".into()
                    },
                    data: serde_json::to_value(report).expect("doctor report is serializable"),
                }
            }
            ServiceOperation::Placeholder { command, arguments } => failed_response(
                &command,
                &format!("'{command}' is not implemented"),
                json!({
                    "arguments": arguments,
                    "error": "not_implemented",
                }),
            ),
        }
    }

    /// 统一命令入口（CLI 与 GUI 同协议）。
    pub fn dispatch_envelope(&self, envelope: AppCommandEnvelope) -> AppResponseEnvelope {
        self.router.dispatch(envelope)
    }

    /// 统一查询入口。
    pub fn dispatch_query(&self, envelope: AppQueryEnvelope) -> AppResponseEnvelope {
        // P18-9：策略闸口位于 CommandRouter 唯一 dispatch_query 边界，
        // 门面直接委托（同一闸口、同一 resolver，不预检、不双记）。
        self.router.dispatch_query(envelope)
    }

    pub fn router(&self) -> &CommandRouter {
        &self.router
    }

    /// 幂等 materialize（P17-7 跨 host/进程 resume）：以 registry 权威
    /// `core_session_id` 在本地 Core aggregate 重建会话记录（已存在即
    /// no-op，不生成新 id、不重绑映射）。Core 是会话事实的唯一来源，
    /// 同 id 多次调用可重放、不堆积。
    pub fn materialize_session(
        &self,
        session_id: &agent_domain::SessionId,
        workspace_id: &agent_domain::WorkspaceId,
        title: impl Into<String>,
    ) -> Result<crate::aggregate::SessionRecord, AppServiceError> {
        self.router.materialize_session(
            session_id,
            workspace_id,
            title.into(),
            crate::error::now_timestamp(),
        )
    }

    /// 租户策略闸口（P18-9）：查询 / 审计 / 管理接口共享的裁决入口。
    pub fn tenant_policy(&self) -> &Arc<TenantPolicyGate> {
        &self.tenant_policy
    }

    /// Audit 查询（P18-9）：要求请求者 `AuditRead` 权限，且只返回其租户
    /// 自己的 versioned、脱敏决策事件；跨租户观察一律拒绝。
    pub fn audit_decisions(
        &self,
        identity: &IdentityContext,
    ) -> Result<Vec<core_api::PolicyDecisionEventView>, AppServiceError> {
        self.tenant_policy
            .query_decision_events(identity)
            .map_err(|error| AppServiceError::Authorization(error.to_string()))
    }

    /// Canonical audit query (P18-13): RBAC-protected and tenant-scoped. Unlike the legacy
    /// policy-decision view this includes route, lease, Agent, approval and client lifecycle
    /// records, while the schema cannot represent prompts, tool output or plaintext secrets.
    pub fn canonical_audit_events(
        &self,
        identity: &IdentityContext,
    ) -> Result<Vec<audit_log::AuditEventV1>, AppServiceError> {
        self.tenant_policy
            .check_permission(identity, Permission::AuditRead)
            .map_err(|error| AppServiceError::Authorization(error.to_string()))?;
        self.tenant_policy
            .canonical_audit_events(&identity.tenant_id)
            .map_err(|error| AppServiceError::Unavailable(error.to_string()))
    }

    /// Canonical allowlist-only export. Both RBAC and the tenant destination policy are
    /// enforced before the exporter sees any records.
    pub fn export_canonical_audit(
        &self,
        identity: &IdentityContext,
        destination: &str,
        exporter: &dyn audit_log::AuditExporter,
    ) -> Result<usize, AppServiceError> {
        self.tenant_policy
            .check_audit_export(identity, destination)
            .map_err(|error| AppServiceError::Authorization(error.to_string()))?;
        self.tenant_policy
            .export_canonical_audit(&identity.tenant_id, exporter)
            .map_err(|error| AppServiceError::Unavailable(error.to_string()))
    }

    /// Attaches a durable canonical audit sink while preserving the built-in tenant
    /// projection. Intended for the production composition root.
    pub fn add_audit_sink(&self, sink: Arc<dyn audit_log::AuditSink>) {
        self.tenant_policy.add_audit_sink(sink);
    }

    /// 读取租户策略视图（P18-9 管理接口）：要求请求者 `PolicyManage` 权限
    /// 且目标为请求者自己的租户。
    pub fn tenant_policy_view(
        &self,
        requester: &IdentityContext,
        tenant: &agent_domain::TenantId,
    ) -> Result<core_api::TenantPolicyView, AppServiceError> {
        self.tenant_policy
            .check_permission(requester, Permission::PolicyManage)
            .map_err(|error| AppServiceError::Authorization(error.to_string()))?;
        self.tenant_policy
            .authorize_scope(requester, tenant)
            .map_err(|error| AppServiceError::Authorization(error.to_string()))?;
        Ok(self.tenant_policy.policy_view(tenant))
    }

    /// 更新租户策略（P18-9 管理接口）：要求请求者 `PolicyManage` 权限且
    /// 目标为请求者自己的租户；引擎每次更新递增策略版本。
    pub fn set_tenant_policy(
        &self,
        requester: &IdentityContext,
        tenant: agent_domain::TenantId,
        policy: TenantPolicy,
    ) -> Result<(), AppServiceError> {
        self.tenant_policy
            .check_permission(requester, Permission::PolicyManage)
            .map_err(|error| AppServiceError::Authorization(error.to_string()))?;
        self.tenant_policy
            .authorize_scope(requester, &tenant)
            .map_err(|error| AppServiceError::Authorization(error.to_string()))?;
        self.tenant_policy.engine().set_policy(tenant, policy);
        Ok(())
    }

    /// 进程内共享的 Quota 运行时（P14-8）；未注入时为 `None`。
    pub fn quota_runtime(&self) -> Option<&Arc<QuotaRuntime>> {
        self.quota_runtime.as_ref()
    }

    /// 注入的共享 CredentialPool（P18-4；未注入时为 `None`）。
    pub fn credential_pool(&self) -> Option<Arc<dyn provider_control::CredentialPool>> {
        self.router.credential_pool()
    }

    /// 注册 Provider 实现（测试注入 / 正式宿主后续由 provider-runtime 注入）。
    pub fn register_provider(&self, provider: Arc<dyn ModelProvider>) -> agent_domain::ProviderId {
        self.router.register_provider(provider)
    }

    /// 按 ProviderId 取共享 Provider（User Hook 判定执行器 / 宿主装配用）。
    pub fn provider(&self, id: &agent_domain::ProviderId) -> Option<Arc<dyn ModelProvider>> {
        self.router.provider(id)
    }

    /// 按 ProviderId 升序取第一个已注册 Provider（User Hook 默认判定 profile
    /// 的兜底落点；无注册时为 `None`，判定 fail-closed）。
    pub fn first_provider(&self) -> Option<Arc<dyn ModelProvider>> {
        self.router.first_provider()
    }

    /// 冲刷并取回已限流合并的应用事件。
    pub fn drain_events(&self) -> Vec<core_api::AppEventEnvelope> {
        self.router.drain_events()
    }

    pub fn status(&self) -> ServiceStatus {
        let state = self.state();
        ServiceStatus {
            version: env!("CARGO_PKG_VERSION").into(),
            instance: self.instance.clone(),
            process_id: std::process::id(),
            lifecycle: state.lifecycle.clone(),
            uptime_millis: u64::try_from(self.started_at.elapsed().as_millis()).unwrap_or(u64::MAX),
            commands_handled: state.commands_handled,
        }
    }

    pub fn doctor(&self) -> DoctorReport {
        let project_directories = directories::ProjectDirs::from("", "", "Pawork");
        let current_directory = std::env::current_dir();
        let checks = vec![
            DoctorCheck {
                name: "host".into(),
                ok: true,
                detail: "CLI and Core share the pawork process".into(),
            },
            DoctorCheck {
                name: "app_service".into(),
                ok: true,
                detail: "unified command router is available".into(),
            },
            DoctorCheck {
                name: "runtime".into(),
                ok: true,
                detail: format!("process {} is running", std::process::id()),
            },
            DoctorCheck {
                name: "project_directories".into(),
                ok: project_directories.is_some(),
                detail: project_directories.map_or_else(
                    || "platform config/data directories are unavailable".into(),
                    |paths| {
                        format!(
                            "config={} data={}",
                            paths.config_dir().display(),
                            paths.data_dir().display()
                        )
                    },
                ),
            },
            DoctorCheck {
                name: "current_directory".into(),
                ok: current_directory.is_ok(),
                detail: current_directory.map_or_else(
                    |error| format!("current directory unavailable: {error}"),
                    |path| path.display().to_string(),
                ),
            },
        ];
        DoctorReport {
            ok: checks.iter().all(|check| check.ok),
            checks,
        }
    }

    pub fn source_count(&self, source: &str) -> u64 {
        self.state()
            .sources
            .get(source)
            .copied()
            .unwrap_or_default()
    }

    /// 按 Artifact ID 流式读取 payload（P13-8）。
    ///
    /// 语义：
    /// - aggregate 无记录 → [`AppServiceError::NotFound`]；
    /// - 未配置 store → [`AppServiceError::Unavailable`]；
    /// - `artifact_id` 非 64-hex（[`BlobId::from_str`] 失败）→ [`AppServiceError::NotFound`]；
    /// - `limit == 0` → 读到文件尾；
    /// - `offset >= byte_length` → 空 `data` + `eof = true`；
    /// - 否则读 `[offset, offset + limit)`，`eof = offset + len >= byte_length`。
    pub async fn artifact_read(
        &self,
        artifact_id: &ArtifactId,
        offset: u64,
        limit: u64,
    ) -> Result<ArtifactReadResult, AppServiceError> {
        let record = self
            .router()
            .aggregate()
            .artifact(artifact_id)
            .ok_or_else(|| AppServiceError::NotFound(format!("artifact {artifact_id}")))?;
        let byte_length = record.byte_length;
        let store = self.artifact_store.as_ref().ok_or_else(|| {
            AppServiceError::Unavailable("artifact store is not configured".into())
        })?;
        let blob_id = BlobId::from_str(artifact_id.as_str())
            .map_err(|_| AppServiceError::NotFound(format!("artifact {artifact_id}")))?;
        if offset >= byte_length {
            return Ok(ArtifactReadResult {
                byte_length,
                data: Vec::new(),
                eof: true,
            });
        }
        // `offset < byte_length` 保证 read_limit >= 1，不会触发 EmptyRange。
        let read_limit = if limit == 0 {
            byte_length - offset
        } else {
            limit.min(byte_length - offset)
        };
        let data = store.read_range(&blob_id, offset, read_limit).await?;
        let eof = offset + data.len() as u64 >= byte_length;
        Ok(ArtifactReadResult {
            byte_length,
            data,
            eof,
        })
    }

    fn run_operation(
        &self,
        source: CommandSource,
        workspace: Option<String>,
        prompt: Option<String>,
        keep_serving: bool,
    ) -> ServiceResponse {
        let Some(prompt) = prompt else {
            return failed_response(
                "run",
                "run command requires a prompt",
                json!({ "implementation_phase": "P13-1" }),
            );
        };
        let prompt_present = !prompt.trim().is_empty();
        if !prompt_present {
            return failed_response(
                "run",
                "run command requires a non-empty prompt",
                json!({ "implementation_phase": "P13-1" }),
            );
        }

        // 1) 解析 workspace：指定路径时优先复用已有 workspace，否则新建。
        let workspace_id = match workspace {
            Some(path) => match self.find_workspace_by_root(&path) {
                Some(workspace) => workspace.id,
                None => match self.add_workspace(&source, &path) {
                    Ok(id) => id,
                    Err(response) => return response,
                },
            },
            None => {
                let default_id = WorkspaceId::from("default");
                if self.router.aggregate().workspace(&default_id).is_none() {
                    let cwd = std::env::current_dir()
                        .map(|path| path.to_string_lossy().into_owned())
                        .unwrap_or_else(|_| ".".into());
                    match self.add_workspace(&source, &cwd) {
                        Ok(id) => id,
                        Err(response) => return response,
                    }
                } else {
                    default_id
                }
            }
        };

        // 2) 创建会话。
        let session_id = match self.router.dispatch(self.envelope(
            &source,
            AppCommand::SessionCreate {
                workspace_id: workspace_id.clone(),
                title: Some("CLI run".into()),
            },
        )) {
            AppResponseEnvelope {
                response: AppResponse::Data(value),
                ..
            } => SessionId::from(
                value
                    .get("session_id")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
            ),
            AppResponseEnvelope {
                response: AppResponse::Error(context),
                ..
            } => {
                return failed_response(
                    "run",
                    &context.message,
                    json!({ "workspace_id": workspace_id }),
                );
            }
            other => {
                return failed_response(
                    "run",
                    &format!("unexpected session create response: {other:?}"),
                    json!({ "workspace_id": workspace_id }),
                );
            }
        };

        // 3) 启动 Run（无凭据/未注册 Provider 时返回结构化错误，不 panic）。
        match self.router.dispatch(self.envelope(
            &source,
            AppCommand::RunStart {
                session_id: session_id.clone(),
                user_message: prompt,
                model: None,
                profile: None,
            },
        )) {
            AppResponseEnvelope {
                response: AppResponse::Accepted { run_id, .. },
                ..
            } => ServiceResponse {
                ok: true,
                kind: "run".into(),
                message: "run command accepted by the in-process app-service".into(),
                data: json!({
                    "workspace_id": workspace_id,
                    "session_id": session_id,
                    "run_id": run_id,
                    "prompt_present": true,
                    "keep_serving": keep_serving,
                    "implementation_phase": "P13-1"
                }),
            },
            AppResponseEnvelope {
                response: AppResponse::Error(context),
                ..
            } => failed_response(
                "run",
                &context.message,
                json!({
                    "workspace_id": workspace_id,
                    "session_id": session_id,
                    "error": context,
                }),
            ),
            other => failed_response(
                "run",
                &format!("unexpected run start response: {other:?}"),
                json!({ "workspace_id": workspace_id, "session_id": session_id }),
            ),
        }
    }

    fn add_workspace(
        &self,
        source: &CommandSource,
        path: &str,
    ) -> Result<WorkspaceId, ServiceResponse> {
        match self.router.dispatch(self.envelope(
            source,
            AppCommand::WorkspaceAdd {
                root_path: path.to_string(),
            },
        )) {
            AppResponseEnvelope {
                response: AppResponse::Data(value),
                ..
            } => Ok(WorkspaceId::from(
                value.get("id").and_then(Value::as_str).unwrap_or_default(),
            )),
            AppResponseEnvelope {
                response: AppResponse::Error(context),
                ..
            } => Err(failed_response(
                "run",
                &context.message,
                json!({ "workspace_path": path }),
            )),
            other => Err(failed_response(
                "run",
                &format!("unexpected workspace add response: {other:?}"),
                json!({ "workspace_path": path }),
            )),
        }
    }

    fn find_workspace_by_root(&self, path: &str) -> Option<workspace_service::Workspace> {
        let canonical = std::fs::canonicalize(path).ok();
        self.router
            .aggregate()
            .workspace_list()
            .into_iter()
            .find(|workspace| {
                workspace.roots.iter().any(|root| {
                    root.path.to_string_lossy() == path
                        || canonical
                            .as_ref()
                            .is_some_and(|canonical| canonical == &root.path)
                })
            })
    }

    fn envelope(&self, source: &CommandSource, command: AppCommand) -> AppCommandEnvelope {
        AppCommandEnvelope {
            api_version: API_VERSION,
            command_id: agent_domain::CommandId::from(self.router.aggregate().next_id("cmd")),
            source: source.clone(),
            identity: ActorIdentity::LocalUser {
                actor_id: ActorId::from("local-cli"),
                display_name: None,
            },
            expected_revision: None,
            idempotency_key: None,
            issued_at: now_timestamp(),
            command,
        }
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

fn response(kind: &str, message: &str) -> ServiceResponse {
    ServiceResponse {
        ok: true,
        kind: kind.into(),
        message: message.into(),
        data: Value::Null,
    }
}

fn failed_response(kind: &str, message: &str, data: Value) -> ServiceResponse {
    ServiceResponse {
        ok: false,
        kind: kind.into(),
        message: message.into(),
        data,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_domain::{CancellationToken, Timestamp};
    use core_api::AppQuery;
    use core_api::{AppCommand, AppCommandEnvelope, AppResponse};

    #[test]
    fn routes_cli_requests_in_process_and_tracks_source() {
        let service = AppService::new("test");
        let response = service.dispatch(ServiceRequest {
            source: CommandSource::LocalCli {
                terminal_session_id: Some("terminal-1".into()),
            },
            operation: ServiceOperation::Status,
        });
        assert!(response.ok);
        assert_eq!(service.source_count("local_cli"), 1);
        assert_eq!(service.status().commands_handled, 1);
    }

    #[test]
    fn doctor_reports_same_process_host_and_router() {
        let service = AppService::new("test");
        let report = service.doctor();
        assert!(report.ok);
        assert_eq!(report.checks.len(), 5);
        assert!(report.checks.iter().all(|check| check.ok));
    }

    #[test]
    fn placeholder_commands_fail_closed_instead_of_reporting_success() {
        let service = AppService::new("p17-32-placeholder");
        for command in ["plugin", "mcp", "import-pi"] {
            let response = service.dispatch(ServiceRequest {
                source: CommandSource::LocalCli {
                    terminal_session_id: Some("terminal-1".into()),
                },
                operation: ServiceOperation::Placeholder {
                    command: command.into(),
                    arguments: vec!["list".into()],
                },
            });
            assert!(!response.ok, "{command} must not report success");
            assert_eq!(response.kind, command);
            assert!(
                response.message.contains("not implemented"),
                "unexpected placeholder message for {command}: {}",
                response.message
            );
            assert_eq!(response.data["error"], "not_implemented");
            assert_eq!(response.data["arguments"], json!(["list"]));
        }
    }

    #[test]
    fn unified_dispatch_handles_commands_and_queries() {
        let service = AppService::new("test");
        let source = CommandSource::Automation;
        let identity = ActorIdentity::Automation {
            name: "test".into(),
        };
        let query = service.dispatch_query(AppQueryEnvelope {
            api_version: API_VERSION,
            request_id: agent_domain::QueryId::from("q-1"),
            source: source.clone(),
            identity: identity.clone(),
            issued_at: Timestamp::from_unix_millis(1),
            query: AppQuery::WorkspaceList,
        });
        assert!(matches!(query.response, AppResponse::Data(_)));
        assert_eq!(service.router().source_stats().get("automation"), Some(&1));
    }

    #[tokio::test]
    async fn production_runtime_reads_local_ledger_without_network() {
        let runtime = QuotaRuntime::production_in_memory();
        let cancel = CancellationToken::new();
        let scope = quota_service::QuotaScope::new(
            agent_domain::TenantId::new(core_api::DEFAULT_QUOTA_TENANT),
            quota_service::AccountId::new(core_api::DEFAULT_QUOTA_ACCOUNT),
            agent_domain::ProviderId::from("mock"),
            None,
        );
        let request = quota_service::QuotaRequest {
            scope,
            window: quota_service::QuotaWindow::Overall,
            unit: quota_service::QuotaUnit::Token,
        };
        // 空查询：仅本地 ledger 派生，无任何网络适配器参与。
        let read = runtime
            .quota
            .read(&request, &cancel)
            .await
            .expect("local ledger read must succeed without network");
        assert!(
            read.failures.is_empty(),
            "no adapter may fail on an empty local read: {:?}",
            read.failures
        );
        assert_eq!(
            read.snapshot.provenance.adapter_kind,
            quota_service::AdapterKind::LocalLedger
        );
        assert_eq!(
            read.snapshot.values.used,
            quota_service::QuotaMeasure::Exact(0)
        );
    }

    #[tokio::test]
    async fn production_quota_runtimes_are_isolated() {
        let runtime_a = QuotaRuntime::production_in_memory();
        let runtime_b = QuotaRuntime::production_in_memory();
        let cancel = CancellationToken::new();
        let scope = quota_service::QuotaScope::new(
            agent_domain::TenantId::new(core_api::DEFAULT_QUOTA_TENANT),
            quota_service::AccountId::new(core_api::DEFAULT_QUOTA_ACCOUNT),
            agent_domain::ProviderId::from("mock"),
            None,
        );
        let request = quota_service::QuotaRequest {
            scope,
            window: quota_service::QuotaWindow::Overall,
            unit: quota_service::QuotaUnit::Token,
        };

        // 只向 A 的 ledger 记账（成功 run 的追加路径与查询读取同一实例）。
        runtime_a
            .ledger
            .record(usage_ledger::UsageRecord {
                record_id: "prod-isolation-1".into(),
                tenant_id: agent_domain::TenantId::new(core_api::DEFAULT_QUOTA_TENANT),
                principal_id: agent_domain::PrincipalId::default(),
                account_id: core_api::DEFAULT_QUOTA_ACCOUNT.to_string(),
                credential_id: None,
                session_id: agent_domain::SessionId::default(),
                agent_id: agent_domain::AgentId::default(),
                run_id: Some(agent_domain::RunId::from("run-a")),
                provider_id: agent_domain::ProviderId::from("mock"),
                model_id: agent_domain::ModelId::from("mock-model"),
                input_tokens: 100,
                output_tokens: 50,
                cache_read_tokens: 0,
                cache_write_tokens: 0,
                cost_micros: 0,
                currency: "USD".into(),
                occurred_at_ms: 1,
                ..usage_ledger::UsageRecord::default()
            })
            .await
            .expect("record into runtime A");

        let read_a = runtime_a
            .quota
            .read(&request, &cancel)
            .await
            .expect("runtime A read");
        let read_b = runtime_b
            .quota
            .read(&request, &cancel)
            .await
            .expect("runtime B read");
        assert_eq!(
            read_a.snapshot.values.used,
            quota_service::QuotaMeasure::Exact(150),
            "runtime A must see its own ledger"
        );
        assert_eq!(
            read_b.snapshot.values.used,
            quota_service::QuotaMeasure::Exact(0),
            "runtime B must stay isolated from runtime A"
        );
    }

    #[test]
    fn materialize_session_is_idempotent_and_does_not_create_ghosts() {
        let service = AppService::new("materialize-idempotent");
        let workspace = service.dispatch_envelope(AppCommandEnvelope {
            api_version: API_VERSION,
            command_id: agent_domain::CommandId::from("ws-add"),
            source: CommandSource::Automation,
            identity: ActorIdentity::Automation {
                name: "test".into(),
            },
            expected_revision: None,
            idempotency_key: None,
            issued_at: Timestamp::from_unix_millis(1),
            command: AppCommand::WorkspaceAdd {
                root_path: std::env::temp_dir().to_string_lossy().into_owned(),
            },
        });
        let AppResponse::Data(value) = workspace.response else {
            panic!("WorkspaceAdd 应成功");
        };
        let workspace_id = agent_domain::WorkspaceId::from(
            value
                .get("id")
                .and_then(|v| v.as_str())
                .expect("workspace id"),
        );
        let session_id = agent_domain::SessionId::from("session-7");
        let first = service
            .materialize_session(&session_id, &workspace_id, "resume")
            .expect("first materialize");
        let second = service
            .materialize_session(&session_id, &workspace_id, "other-title")
            .expect("second materialize");
        assert_eq!(first.session_id, session_id);
        assert_eq!(second.session_id, session_id);
        assert_eq!(first.title, "resume");
        assert_eq!(second.title, first.title, "已存在时不得改写字段");
        assert_eq!(service.router().aggregate().snapshot().sessions.len(), 1);
        let created = service.dispatch_envelope(AppCommandEnvelope {
            api_version: API_VERSION,
            command_id: agent_domain::CommandId::from("session-create"),
            source: CommandSource::Automation,
            identity: ActorIdentity::Automation {
                name: "test".into(),
            },
            expected_revision: None,
            idempotency_key: None,
            issued_at: Timestamp::from_unix_millis(2),
            command: AppCommand::SessionCreate {
                workspace_id,
                title: Some("fresh".into()),
            },
        });
        let AppResponse::Data(created) = created.response else {
            panic!("SessionCreate 应成功");
        };
        assert_ne!(
            created.get("session_id").and_then(|v| v.as_str()),
            Some("session-7"),
            "materialize 后 create_session 不得复用同一 id"
        );
        assert_eq!(service.router().aggregate().snapshot().sessions.len(), 2);
    }

    #[tokio::test]
    async fn production_persistent_reopen_reads_same_ledger() {
        // P18-8：同一 SQLite 账本文件跨“进程”重开（open→record→drop→
        // reopen），新进程 usage 可读且 quota 聚合正确；重放不重复累计。
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("usage-ledger.sqlite3");
        let cancel = CancellationToken::new();
        let scope = quota_service::QuotaScope::new(
            agent_domain::TenantId::new(core_api::DEFAULT_QUOTA_TENANT),
            quota_service::AccountId::new(core_api::DEFAULT_QUOTA_ACCOUNT),
            agent_domain::ProviderId::from("mock"),
            None,
        );
        let request = quota_service::QuotaRequest {
            scope,
            window: quota_service::QuotaWindow::Overall,
            unit: quota_service::QuotaUnit::Token,
        };

        // 第一个“进程”：持久装配 + 记账。
        {
            let runtime_a = QuotaRuntime::production_persistent(&path).await.unwrap();
            runtime_a
                .ledger
                .record(usage_ledger::UsageRecord {
                    record_id: "persistent-reopen-1".into(),
                    tenant_id: agent_domain::TenantId::new(core_api::DEFAULT_QUOTA_TENANT),
                    principal_id: agent_domain::PrincipalId::default(),
                    account_id: core_api::DEFAULT_QUOTA_ACCOUNT.to_string(),
                    credential_id: None,
                    session_id: agent_domain::SessionId::default(),
                    agent_id: agent_domain::AgentId::default(),
                    run_id: Some(agent_domain::RunId::from("run-persist")),
                    provider_id: agent_domain::ProviderId::from("mock"),
                    model_id: agent_domain::ModelId::from("mock-model"),
                    input_tokens: 100,
                    output_tokens: 50,
                    cache_read_tokens: 0,
                    cache_write_tokens: 0,
                    cost_micros: 0,
                    currency: "USD".into(),
                    occurred_at_ms: 1,
                    ..usage_ledger::UsageRecord::default()
                })
                .await
                .expect("record into persistent ledger");
        }

        // 第二个“进程”：重开同一文件，usage 可读、quota 聚合正确。
        let runtime_b = QuotaRuntime::production_persistent(&path).await.unwrap();
        let records = runtime_b
            .ledger
            .query(&usage_ledger::UsageQuery::default())
            .await
            .unwrap();
        assert_eq!(records.len(), 1, "run 进程写入的用量必须跨进程可见");

        let read = runtime_b.quota.read(&request, &cancel).await.unwrap();
        assert_eq!(
            read.snapshot.values.used,
            quota_service::QuotaMeasure::Exact(150),
            "同一账本驱动 quota 聚合，禁止第二套累计源"
        );
    }

    #[tokio::test]
    async fn persistent_replay_populates_each_currency_cache_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("usage-ledger-multi-currency.sqlite3");
        {
            let runtime = QuotaRuntime::production_persistent(&path).await.unwrap();
            for (index, currency, cost_micros) in [(1, "USD", 120), (2, "EUR", 340)] {
                runtime
                    .ledger
                    .record(usage_ledger::UsageRecord {
                        record_id: format!("currency-replay-{index}"),
                        tenant_id: agent_domain::TenantId::new(core_api::DEFAULT_QUOTA_TENANT),
                        principal_id: agent_domain::PrincipalId::default(),
                        account_id: core_api::DEFAULT_QUOTA_ACCOUNT.to_string(),
                        session_id: agent_domain::SessionId::default(),
                        agent_id: agent_domain::AgentId::default(),
                        provider_id: agent_domain::ProviderId::from("mock"),
                        model_id: agent_domain::ModelId::from("mock-model"),
                        input_tokens: 1,
                        output_tokens: 1,
                        cost_micros,
                        currency: currency.into(),
                        occurred_at_ms: index,
                        ..usage_ledger::UsageRecord::default()
                    })
                    .await
                    .expect("seed currency record");
            }
        }

        let runtime = QuotaRuntime::production_persistent(&path).await.unwrap();
        let scope = quota_service::QuotaScope::new(
            agent_domain::TenantId::new(core_api::DEFAULT_QUOTA_TENANT),
            quota_service::AccountId::new(core_api::DEFAULT_QUOTA_ACCOUNT),
            agent_domain::ProviderId::from("mock"),
            Some(agent_domain::ModelId::from("mock-model")),
        );
        for (currency, expected) in [("USD", 120), ("EUR", 340)] {
            let read = runtime
                .quota
                .read_cache_only(&quota_service::QuotaRequest {
                    scope: scope.clone(),
                    window: quota_service::QuotaWindow::Overall,
                    unit: quota_service::QuotaUnit::Cost {
                        currency: currency.into(),
                    },
                })
                .expect("each persisted currency must have a replayed cache key");
            let snapshot = match read {
                quota_service::CacheRead::Hit { snapshot, .. } => snapshot,
                other => panic!("expected cache hit for {currency}, got {other:?}"),
            };
            assert_eq!(
                snapshot.values.used,
                quota_service::QuotaMeasure::Exact(expected)
            );
        }
    }

    #[tokio::test]
    async fn refresh_local_cache_publishes_full_and_account_scopes() {
        let clock = Arc::new(quota_service::service::MutableQuotaClock::at(1_000_000));
        let ledger: Arc<dyn usage_ledger::UsageLedger> =
            Arc::new(usage_ledger::InMemoryUsageLedger::new());
        let runtime = QuotaRuntime::new(ledger, clock);
        let record = usage_ledger::UsageRecord {
            record_id: "local-reconcile-1".into(),
            tenant_id: agent_domain::TenantId::new(core_api::DEFAULT_QUOTA_TENANT),
            principal_id: agent_domain::PrincipalId::default(),
            account_id: core_api::DEFAULT_QUOTA_ACCOUNT.to_string(),
            credential_id: None,
            session_id: agent_domain::SessionId::default(),
            agent_id: agent_domain::AgentId::default(),
            run_id: None,
            provider_id: agent_domain::ProviderId::from("mock"),
            model_id: agent_domain::ModelId::from("mock-model"),
            input_tokens: 100,
            output_tokens: 50,
            cache_read_tokens: 20,
            cache_write_tokens: 30,
            cost_micros: 12_345,
            currency: "USD".into(),
            occurred_at_ms: 900_000,
            ..usage_ledger::UsageRecord::default()
        };
        runtime
            .ledger
            .record(record.clone())
            .await
            .expect("record into shared ledger");
        runtime
            .ledger
            .record(usage_ledger::UsageRecord {
                record_id: "local-reconcile-sibling".into(),
                tenant_id: record.tenant_id.clone(),
                principal_id: agent_domain::PrincipalId::default(),
                account_id: record.account_id.clone(),
                credential_id: Some("cred-other".into()),
                session_id: agent_domain::SessionId::default(),
                agent_id: agent_domain::AgentId::default(),
                run_id: None,
                provider_id: record.provider_id.clone(),
                model_id: agent_domain::ModelId::from("other-model"),
                input_tokens: 10,
                output_tokens: 5,
                cache_read_tokens: 2,
                cache_write_tokens: 3,
                cost_micros: 655,
                currency: "USD".into(),
                occurred_at_ms: 900_000,
                ..usage_ledger::UsageRecord::default()
            })
            .await
            .expect("record sibling usage into shared ledger");

        // 同一 adapter 实例已 register + set_ledger_reconciler；直接走本地
        // fetch + publish，全程无网络。
        runtime
            .refresh_local_cache(&record)
            .await
            .expect("all full-scope and account-scope keys must reconcile locally");

        // 无 credential 的完整 model scope 仍不同于无 model 的账户级 scope：
        // 2 scopes × 4 windows ×（Token + Cost<USD>）= 16 keys。
        assert_eq!(runtime.quota.cache_size(), 16);

        let full_scope = quota_service::QuotaScope {
            tenant_id: record.tenant_id.clone(),
            account_id: quota_service::AccountId::new(record.account_id.clone()),
            credential_id: record.credential_id.clone(),
            provider_id: record.provider_id.clone(),
            model_id: Some(record.model_id.clone()),
        };
        let account_scope = quota_service::QuotaScope {
            tenant_id: record.tenant_id.clone(),
            account_id: quota_service::AccountId::new(record.account_id.clone()),
            credential_id: None,
            provider_id: record.provider_id.clone(),
            model_id: None,
        };
        let assert_scope = |scope: &quota_service::QuotaScope, tokens, cost_micros| {
            for window in [
                quota_service::QuotaWindow::Overall,
                quota_service::QuotaWindow::Rolling5h,
                quota_service::QuotaWindow::Weekly,
                quota_service::QuotaWindow::Monthly,
            ] {
                for (unit, used) in [
                    (quota_service::QuotaUnit::Token, tokens),
                    (
                        quota_service::QuotaUnit::Cost {
                            currency: "USD".into(),
                        },
                        cost_micros,
                    ),
                ] {
                    let read = runtime
                        .quota
                        .read_cache_only(&quota_service::QuotaRequest {
                            scope: scope.clone(),
                            window,
                            unit: unit.clone(),
                        })
                        .expect("cache-only read must not touch adapters");
                    match read {
                        // 只依赖变体判别，不读取恒定的 from_cache 字段
                        // （P14 review §3.6 压平后该字段被删除）。
                        quota_service::CacheRead::Hit { snapshot, .. } => {
                            assert_eq!(snapshot.window, window);
                            assert_eq!(snapshot.unit, unit);
                            assert_eq!(
                                snapshot.values.used,
                                quota_service::QuotaMeasure::Exact(used)
                            );
                            assert_eq!(
                                snapshot.provenance.adapter_kind,
                                quota_service::AdapterKind::LocalLedger
                            );
                        }
                        other => {
                            panic!("expected fresh cache hit for {window:?} {unit:?}: {other:?}")
                        }
                    }
                }
            }
        };
        assert_scope(&full_scope, 200, 12_345);
        assert_scope(&account_scope, 220, 13_000);

        // 只发布目标完整 scope 与账户级 scope；其他具体 model/credential
        // 即使已有 ledger 记录，也不会获得具体 scope 的缓存键。
        let other_model = quota_service::QuotaScope {
            tenant_id: record.tenant_id.clone(),
            account_id: quota_service::AccountId::new(record.account_id.clone()),
            credential_id: None,
            provider_id: record.provider_id.clone(),
            model_id: Some(agent_domain::ModelId::from("other-model")),
        };
        assert!(
            runtime
                .quota
                .cached_snapshots_for_scope(&other_model)
                .is_empty(),
            "account-level entries must not leak to a concrete sibling model"
        );
        let other_credential = quota_service::QuotaScope {
            tenant_id: record.tenant_id.clone(),
            account_id: quota_service::AccountId::new(record.account_id.clone()),
            credential_id: Some("cred-other".into()),
            provider_id: record.provider_id.clone(),
            model_id: Some(record.model_id.clone()),
        };
        assert!(
            runtime
                .quota
                .cached_snapshots_for_scope(&other_credential)
                .is_empty(),
            "account-level entries must not leak to a concrete sibling credential"
        );
    }
}
