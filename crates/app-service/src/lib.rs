//! CLI 与 GUI 共享的进程内应用服务门面（P13-1）。
//!
//! [`AppService`] 是唯一正式宿主（`pawork`）持有的门面：保留 legacy
//! [`AppService::dispatch`]（[`ServiceRequest`] → [`ServiceResponse`]）的同时，
//! 把真实命令/查询路由委托给统一 [`CommandRouter`]（`dispatch_envelope` /
//! `dispatch_query`），CLI 与 GUI 走同一入口、同一错误协议。

mod aggregate;
mod approval;
mod error;
mod idempotency;
mod rate_limit;
mod router;
mod supervisor;

use std::collections::BTreeMap;
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
pub use error::AppServiceError;
pub use idempotency::{IdempotencyCheck, IdempotencyError, IdempotencyStats, IdempotencyStore};
pub use rate_limit::{DeltaKind, RateLimiter, RateLimiterStats};
pub use router::{source_name, CommandRouter, RouterConfig};
pub use supervisor::{
    CancelOutcome, RunRequest, RunSupervisor, RunSupervisorStats, SuperviseError,
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

    /// 生产构造（P14-8 正式接线）：进程内新建共享
    /// [`usage_ledger::InMemoryUsageLedger`] 与
    /// [`quota_service::service::SystemQuotaClock`]，同一 ledger 同时服务
    /// 记账（成功 run 追加）与额度查询（适配器派生）。
    ///
    /// 唯一注册的适配器是本地 ledger 派生（[`quota_service::AdapterKind::LocalLedger`]），
    /// 构造与空查询均不触发任何网络。
    pub fn production() -> Arc<Self> {
        let ledger: Arc<dyn usage_ledger::UsageLedger> =
            Arc::new(usage_ledger::InMemoryUsageLedger::new());
        let clock: Arc<dyn quota_service::service::QuotaClock> =
            Arc::new(quota_service::service::SystemQuotaClock);
        Self::new(ledger, clock)
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
}

impl AppService {
    pub fn new(instance: impl Into<String>) -> Self {
        Self::build(instance, None, None)
    }

    /// 携带内容寻址 Blob Store 构造（P13-8 接线）；`AppService::new` 等价于
    /// store 为 `None`（此时 `artifact_read` 返回 `Unavailable`）。
    pub fn with_artifact_store(instance: impl Into<String>, store: Arc<ArtifactStore>) -> Self {
        Self::build(instance, Some(store), None)
    }

    /// 携带共享 Quota 运行时构造（P14-8）：注入唯一 ledger + QuotaService。
    /// 既有 [`AppService::new`] / [`AppService::with_artifact_store`] 保持兼容
    /// （quota 为 `None`，不记账、不查额度）。
    pub fn with_quota_runtime(
        instance: impl Into<String>,
        store: Option<Arc<ArtifactStore>>,
        quota_runtime: Arc<QuotaRuntime>,
    ) -> Self {
        Self::build(instance, store, Some(quota_runtime))
    }

    fn build(
        instance: impl Into<String>,
        artifact_store: Option<Arc<ArtifactStore>>,
        quota_runtime: Option<Arc<QuotaRuntime>>,
    ) -> Self {
        let instance = instance.into();
        let router = CommandRouter::new(RouterConfig {
            instance: instance.clone(),
            ..RouterConfig::default()
        });
        if let Some(runtime) = quota_runtime.as_ref() {
            router.set_quota_runtime(Arc::clone(runtime));
        }
        Self {
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
        }
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
            ServiceOperation::Placeholder { command, arguments } => ServiceResponse {
                ok: true,
                kind: command.clone(),
                message: format!("'{command}' command route is available"),
                data: json!({ "arguments": arguments, "implementation_phase": "later" }),
            },
        }
    }

    /// 统一命令入口（CLI 与 GUI 同协议）。
    pub fn dispatch_envelope(&self, envelope: AppCommandEnvelope) -> AppResponseEnvelope {
        self.router.dispatch(envelope)
    }

    /// 统一查询入口。
    pub fn dispatch_query(&self, envelope: AppQueryEnvelope) -> AppResponseEnvelope {
        self.router.dispatch_query(envelope)
    }

    pub fn router(&self) -> &CommandRouter {
        &self.router
    }

    /// 进程内共享的 Quota 运行时（P14-8）；未注入时为 `None`。
    pub fn quota_runtime(&self) -> Option<&Arc<QuotaRuntime>> {
        self.quota_runtime.as_ref()
    }

    /// 注册 Provider 实现（测试注入 / 正式宿主后续由 provider-runtime 注入）。
    pub fn register_provider(&self, provider: Arc<dyn ModelProvider>) -> agent_domain::ProviderId {
        self.router.register_provider(provider)
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
            },
        )) {
            AppResponseEnvelope {
                response: AppResponse::Accepted { .. },
                ..
            } => ServiceResponse {
                ok: true,
                kind: "run".into(),
                message: "run command accepted by the in-process app-service".into(),
                data: json!({
                    "workspace_id": workspace_id,
                    "session_id": session_id,
                    "run_id": self.router.last_started_run(),
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
        let runtime = QuotaRuntime::production();
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
        let runtime_a = QuotaRuntime::production();
        let runtime_b = QuotaRuntime::production();
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
