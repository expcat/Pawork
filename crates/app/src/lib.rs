//! 应用门面：读配置 → 凭证链（auth 文件 → env）→ provider → 读写工具 +
//! run_command → 事件化 `run_session`（S6 波 C 起六通道正式装配）。
//!
//! 不按 Provider 名称分支；协议来自 `extra.provider_protocols` 与默认表。
//! 落库 persist-first，再推渲染 sink。

mod approval;
mod auth;
mod channels;
mod checkpoint;
mod control;
mod data_dir;
mod diff;
mod extensions;
mod gui_host;
pub mod gui_server;
mod hub;
mod idempotency;
mod import_host;
mod loop_ctx;
mod orchestration_host;
mod persist;
mod plan_host;
mod protected;
mod provider_assembly;
mod protocol;
mod services;
mod tasks_host;
#[cfg(test)]
mod testsupport;

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicU64;
use std::sync::Arc;

use async_trait::async_trait;

use pawork_domain::{
    CanonicalModelRequest, ModelDefinition, ModelProvider, ModelResponseSummary, ProviderError,
    ProviderErrorKind, ProviderEventSink, ResolvedCredential, ToolDefinition,
};
use pawork_workspace::config::{
    ConfigError, Loader, PaworkConfig,
};
use pawork_domain::{
    AgentEvent, ApprovalDecision, CancellationToken, Message, ModelId, ProviderId, RequestId, RunId, SessionId,
    ToolDescriptor, WorkspaceId, TokenUsage, Cost,
};
use pawork_engine::{
    AgentEventSink, ContextBudget, ContextLimits, EngineError, HeuristicEstimator,
    TokenEstimator as EngineTokenEstimator, TurnContext,
};
use pawork_auth::{AuthError, FileBackend, MemoryBackend, SecretBackend};
use pawork_providers::ModelRegistry;
use pawork_storage::session::{SessionStore, SessionStoreError};
use pawork_tools::{ToolRegistry, ToolRegistryError, ToolScheduler, ToolSchedulerConfig};
use pawork_workspace::{FileIndexError, WorkspaceError, WorkspaceService};
use thiserror::Error;

use crate::provider_assembly::{
    assemble_provider, assemble_registry, channel_protocol, is_credential_pending,
};

pub use approval::{
    parse_approval_mode, ApprovalAsk, ApprovalPromptHost, ApprovalResolve, DenyAllApprovals,
    GuiApprovalHost, PendingToolApproval,
};
pub use checkpoint::{CheckpointSummary, RollbackOutcome};
pub use data_dir::{
    artifact_store_path, artifact_store_path_for, audit_log_path_for, consume_data_dir_outcome,
    default_data_dir, default_data_dir_outcome, instance_dir, normalize_instance, session_db_path,
    session_db_path_for, protected_store_path_for, tasks_snapshot_path_for, usage_ledger_path_for,
    DataDirOutcome,
    DEFAULT_INSTANCE,
};
pub use diff::{paginate_diff, render_diff_file, render_session_diff, GitDiffHeader, SessionDiff};
pub use pawork_git::{DiffFile, DiffPage};
pub use gui_host::{
    project_timeline_item, GuiBroadcastSink, GuiEventBus, GuiHostAdapter, GuiRunRegistry,
};
pub use hub::{EventHub, HubError, HubSubscription, DEFAULT_HUB_CAPACITY};
pub use idempotency::{
    should_cache, IdempotencyCheck, IdempotencyError, IdempotencyStats, IdempotencyStore,
    DEFAULT_IDEMPOTENCY_CAPACITY,
};
pub use persist::PersistThenRender;
pub use protocol::{AdapterProtocol, ProtocolError};
pub use auth::{AuthChannelStatus, AuthSource, OAuthLogin};
pub use channels::{
    first_party_channel, is_first_party, ChannelKind, FirstPartyChannel, FIRST_PARTY_CHANNELS,
};
pub use extensions::{AtAttachment, McpServerStatus};
pub use control::{
    LedgerTotals, QuotaWindowLine, SessionUsageLine, UsageOverview,
};
pub use import_host::{
    parse_session_source, CompatImportItemView, CompatImportPreview, CompatImportReport,
    CompatTool, SessionImportFormat, SessionImportOutcome,
};
pub use orchestration_host::{MultiAgentDemoOptions, MultiAgentDemoReport};
pub use pawork_workflow::plan::PlanSnapshot;
pub use pawork_workflow::task::TaskSnapshot;
pub use plan_host::review_status_label;
pub use tasks_host::parse_task_kind;
pub use pawork_workspace::import::ExternalSource as CompatExternalSource;
pub use pawork_policy::{ApprovalMode, RiskLevel};
pub use pawork_storage::session::{SessionExport, SessionRecord, EXPORT_SCHEMA_VERSION};

/// 从配置文件与 CLI 覆盖构造 [`AppCore`] 的选项。
#[derive(Clone, Default)]
pub struct AppLoadOptions {
    pub workspace_root: Option<PathBuf>,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub data_dir: Option<PathBuf>,
    pub approval_mode: Option<ApprovalMode>,
    pub approval_host: Option<Arc<dyn ApprovalPromptHost>>,
    /// 凭证后端覆盖（自动测试注入 MemoryBackend；默认 auth 文件）。
    pub auth_backend: Option<Arc<dyn SecretBackend>>,
    /// 隔离实例名（数据目录子路径；默认 `default`）。
    pub instance: String,
}

impl std::fmt::Debug for AppLoadOptions {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AppLoadOptions")
            .field("workspace_root", &self.workspace_root)
            .field("provider", &self.provider)
            .field("model", &self.model)
            .field("data_dir", &self.data_dir)
            .field("approval_mode", &self.approval_mode)
            .field("has_approval_host", &self.approval_host.is_some())
            .field("has_auth_backend", &self.auth_backend.is_some())
            .field("instance", &self.instance)
            .finish()
    }
}

impl AppLoadOptions {
    pub fn from_cli(provider: Option<String>, model: Option<String>) -> Self {
        Self {
            workspace_root: std::env::current_dir().ok(),
            provider,
            model,
            data_dir: None,
            approval_mode: None,
            approval_host: None,
            auth_backend: None,
            instance: crate::DEFAULT_INSTANCE.to_string(),
        }
    }
}

/// 装配期错误。明文 key 不得进入任何变体。
#[derive(Debug, Error)]
pub enum AppError {
    #[error(transparent)]
    Config(#[from] ConfigError),
    #[error("未配置 default_provider，请在 config.toml 中设置或使用 --provider")]
    MissingDefaultProvider,
    #[error("未配置 default_model，请在 config.toml 中设置或使用 --model")]
    MissingDefaultModel,
    #[error("配置中找不到 provider `{id}`")]
    UnknownProvider { id: String },
    #[error("provider `{id}` 未配置 base_url")]
    MissingBaseUrl { id: String },
    #[error("provider {provider} 缺少凭证：pawork auth set-key {provider}（auth 文件）或环境变量 {env_name}")]
    MissingCredential {
        provider: String,
        env_name: String,
    },
    #[error("provider {0} 缺少 OAuth 凭证：pawork auth login {0}")]
    OAuthLoginRequired(String),
    #[error("{0}")]
    OAuthLogin(String),
    #[error(transparent)]
    Auth(#[from] AuthError),
    #[error("未知模型 {model}（provider {provider}）")]
    UnknownModel { model: String, provider: String },
    #[error("模型 {model} 属于 provider {owner}，当前是 {current}；先 /provider {owner}")]
    ModelBelongsToProvider {
        model: String,
        owner: String,
        current: String,
    },
    #[error(transparent)]
    Provider(#[from] ProviderError),
    #[error(transparent)]
    Engine(#[from] EngineError),
    #[error(transparent)]
    Session(#[from] SessionStoreError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("session store is not open")]
    StoreNotOpen,
    #[error("session not found: {0}")]
    SessionNotFound(String),
    #[error("ambiguous session prefix `{prefix}` matches: {matches}")]
    AmbiguousSession { prefix: String, matches: String },
    #[error("chat turn requires at least one message")]
    EmptyTurn,
    #[error(transparent)]
    Workspace(#[from] WorkspaceError),
    #[error(transparent)]
    Tools(#[from] ToolRegistryError),
    #[error(transparent)]
    Protocol(#[from] ProtocolError),
    #[error("{0}")]
    ApprovalMode(String),
    #[error("invalid proxy_url: {0}")]
    InvalidProxy(String),
    #[error("{0}")]
    InvalidInstance(String),
    #[error(transparent)]
    Checkpoint(#[from] pawork_storage::blob::CheckpointError),
    #[error(transparent)]
    Artifact(#[from] pawork_storage::blob::ArtifactStoreError),
    #[error(transparent)]
    Git(#[from] pawork_git::GitError),
    #[error("checkpoint store is not open")]
    CheckpointStoreNotOpen,
    #[error("checkpoint not found: {0}")]
    CheckpointNotFound(String),
    #[error("ambiguous checkpoint `{prefix}` matches: {matches}")]
    AmbiguousCheckpoint { prefix: String, matches: String },
    #[error(transparent)]
    ProtectedBlob(#[from] pawork_storage::blob::ProtectedBlobError),
    #[error("{0}")]
    Protected(String),
    #[error(transparent)]
    Mcp(#[from] pawork_tools::mcp::McpError),
    #[error(transparent)]
    Resources(#[from] pawork_workspace::resources::ResourceLoadError),
    #[error(transparent)]
    Compat(#[from] pawork_workspace::import::error::CompatError),
    #[error(transparent)]
    FileIndex(#[from] FileIndexError),
    #[error("{0}")]
    Import(String),
    #[error("计划 {plan_id}@{version} 尚未批准（{status}），先 pawork plan approve")]
    PlanNotApproved {
        plan_id: String,
        version: String,
        status: String,
    },
    #[error("{0}")]
    Plan(String),
    #[error("{0}")]
    ControlPlane(String),
    #[error("{0}")]
    Task(String),
    #[error("{0}")]
    Orchestration(String),
}

/// S5 压缩在 engine 侧保留的最近消息条数；session 侧保留策略按
/// `RETAINED_MESSAGES / 2` 轮对齐同一折叠边界。
pub(crate) const RETAINED_MESSAGES: usize = 4;

/// 目录兜底 provider：默认 provider 缺凭证时的占位（list 空目录、stream
/// fail-closed）。只在 host 装配层使用，Engine 无感知。
struct CatalogOnlyProvider {
    id: ProviderId,
}

fn missing_credential_degrade(provider_id: &ProviderId) -> pawork_domain::DegradeEvent {
    pawork_domain::DegradeEvent::new(
        pawork_domain::DegradeKind::MissingCredential,
        pawork_domain::DegradeSeverity::Warning,
        format!("provider {} has no credential; using catalog-only fallback", provider_id.as_str()),
        serde_json::json!({ "provider_id": provider_id.as_str() }),
    )
}

fn emit_missing_credential_degrade(provider_id: &ProviderId) -> pawork_domain::DegradeEvent {
    let degrade = missing_credential_degrade(provider_id);
    tracing::warn!(
        code = %degrade.code(),
        provider_id = provider_id.as_str(),
        "{}",
        degrade.message
    );
    degrade
}

#[async_trait]
impl ModelProvider for CatalogOnlyProvider {
    fn id(&self) -> ProviderId {
        self.id.clone()
    }

    async fn list_models(
        &self,
        _credential: Option<&ResolvedCredential>,
    ) -> Result<Vec<ModelDefinition>, ProviderError> {
        Ok(Vec::new())
    }

    async fn stream(
        &self,
        _request: CanonicalModelRequest,
        _sink: &dyn ProviderEventSink,
        _cancel: CancellationToken,
    ) -> Result<ModelResponseSummary, ProviderError> {
        Err(ProviderError::new(
            ProviderErrorKind::Authentication,
            format!(
                "provider {} 未装配凭证：先 pawork auth set-key {} 或 pawork auth login {}",
                self.id.as_str(),
                self.id.as_str(),
                self.id.as_str()
            ),
        ))
    }
}

/// 已装配的 Core：协议中立 provider、读写工具、默认 model、可选 session store。
pub struct AppCore {
    provider: Arc<dyn ModelProvider>,
    /// true 表示默认 provider 因缺凭证未装配（目录/凭证命令仍可用；
    /// chat 走 CatalogOnlyProvider 的 Authentication 错误 fail-closed）。
    provider_pending: bool,
    credential: Option<ResolvedCredential>,
    model: ModelId,
    provider_id: ProviderId,
    /// 装配后的完整配置（provider 切换 / OAuth 覆盖读取）。
    config: PaworkConfig,
    /// 凭证后端（auth 文件或测试注入的内存后端）。
    backend: Arc<dyn SecretBackend>,
    /// OAuth 刷新 / token 交换用的共享 HTTP 客户端。
    http: reqwest::Client,
    /// 模型目录（builtin + provider 静态目录 + config 覆盖 + 运行期探测）。
    registry: Arc<ModelRegistry>,
    /// engine 侧启发式 token 估算器（预算 / 截断 / 压缩判定共用）。
    heuristic: Arc<HeuristicEstimator>,
    /// session 侧窄口 TokenEstimator（压缩快照统计），由 heuristic 桥接。
    session_estimator: Arc<dyn pawork_storage::session::TokenEstimator>,
    adapter_protocol: AdapterProtocol,
    store: Option<SessionStore>,
    scheduler: Arc<ToolScheduler>,
    tool_defs: Vec<ToolDefinition>,
    descriptors: Vec<ToolDescriptor>,
    pub(crate) approval: services::approval::ApprovalService,
    pub(crate) session: services::session::SessionService,
    pub(crate) run: services::run::RunService,
    pub(crate) extensions: services::extension::ExtensionService,
    pub(crate) checkpoints: Option<pawork_storage::blob::CheckpointService>,
    pub(crate) artifacts: Option<pawork_storage::blob::ArtifactStore>,
    pub(crate) protected_store: Option<std::sync::Arc<pawork_storage::blob::ProtectedBlobStore>>,
    pub(crate) reasoning_protector: std::sync::Arc<crate::protected::SwappableReasoningProtector>,
    pub(crate) usage: services::usage::UsageService,
    pub(crate) tasks: services::tasks::TaskService,
    pub(crate) imports: services::import::ImportService,
    next_request: AtomicU64,
    next_run: AtomicU64,
    next_session: AtomicU64,
    next_message: AtomicU64,
}

/// 把 engine 的完整估算器桥接到 session 侧窄口 trait（依赖倒置的宿主实现）。
struct SessionTokenEstimatorBridge(Arc<HeuristicEstimator>);

impl pawork_storage::session::TokenEstimator for SessionTokenEstimatorBridge {
    fn count_text(&self, text: &str) -> u64 {
        self.0.count_text(text)
    }

    fn count_message(&self, message: &Message) -> u64 {
        self.0.count_message(message)
    }
}

impl std::fmt::Debug for AppCore {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AppCore")
            .field("provider_id", &self.provider_id)
            .field("model", &self.model)
            .field("adapter_protocol", &self.adapter_protocol)
            .field("credential", &self.credential)
            .field("has_store", &self.store.is_some())
            .field("tool_count", &self.tool_defs.len())
            .field("approval_mode", &self.approval.mode())
            .field("workspace_trusted", &self.approval.workspace_trusted())
            .finish()
    }
}

impl AppCore {
    /// 发现 Builtin + Global + Workspace，再套用 CLI 覆盖，并打开 session.db。
    pub async fn load(options: AppLoadOptions) -> Result<Self, AppError> {
        Self::load_with(options, false).await
    }

    /// 目录 / 凭证命令用装配：默认 provider 缺凭证时不失败，退回
    /// CatalogOnlyProvider（chat 在请求时 fail-closed 报缺凭证）。
    pub async fn load_for_catalog(options: AppLoadOptions) -> Result<Self, AppError> {
        Self::load_with(options, true).await
    }

    async fn load_with(options: AppLoadOptions, allow_pending: bool) -> Result<Self, AppError> {
        let resolved = Loader::discover(options.workspace_root.as_deref()).resolve()?;
        let backend = options
            .auth_backend
            .clone()
            .unwrap_or_else(|| Arc::new(FileBackend::new()));
        let workspace_root = options
            .workspace_root
            .clone()
            .or_else(|| std::env::current_dir().ok());
        let trusted = resolved.config.trust_workspaces.unwrap_or(false);
        let mut core = Self::from_config_inner(
            resolved.config,
            options.provider.as_deref(),
            options.model.as_deref(),
            backend,
            allow_pending,
        )
        .await?;
        core.configure_approval(
            options.approval_mode.unwrap_or_default(),
            trusted,
            options
                .approval_host
                .unwrap_or_else(|| Arc::new(DenyAllApprovals)),
        );
        if let Some(root) = workspace_root.as_deref() {
            core.attach_workspace(root)?;
        }
        core.prime_extensions().await?;
        let data_dir = if let Some(data_dir) = options.data_dir {
            data_dir
        } else {
            consume_data_dir_outcome(default_data_dir_outcome())
        };
        let instance = if options.instance.trim().is_empty() {
            crate::DEFAULT_INSTANCE
        } else {
            crate::normalize_instance(&options.instance).map_err(AppError::InvalidInstance)?
        };
        core.open_store(session_db_path_for(&data_dir, instance))
            .await?;
        core.open_checkpoints(artifact_store_path_for(&data_dir, instance))
            .await?;
        core.open_protected(protected_store_path_for(&data_dir, instance))
            .await?;
        core.open_control_plane(crate::instance_dir(&data_dir, instance))?;
        Ok(core)
    }

    pub async fn load_from(
        global_file: Option<&Path>,
        workspace_file: Option<&Path>,
        provider: Option<&str>,
        model: Option<&str>,
        store_path: impl AsRef<Path>,
    ) -> Result<Self, AppError> {
        let resolved = Loader::discover_from(global_file, workspace_file).resolve()?;
        let backend: Arc<dyn SecretBackend> = Arc::new(FileBackend::new());
        let trusted = resolved.config.trust_workspaces.unwrap_or(false);
        let mut core = Self::from_config(resolved.config, provider, model, backend).await?;
        core.configure_approval(ApprovalMode::ReadOnly, trusted, Arc::new(DenyAllApprovals));
        if let Some(root) = workspace_root_from_config_file(workspace_file) {
            core.attach_workspace(&root)?;
        } else if let Ok(cwd) = std::env::current_dir() {
            core.attach_workspace(&cwd)?;
        }
        core.prime_extensions().await?;
        let store_path = store_path.as_ref();
        core.open_store(store_path).await?;
        if let Some(parent) = store_path.parent() {
            core.open_checkpoints(parent.join("artifacts")).await?;
            core.open_protected(parent.join("protected")).await?;
            core.open_control_plane(parent.to_path_buf())?;
        }
        Ok(core)
    }

    pub fn from_resolved(
        mut config: PaworkConfig,
        provider: Option<&str>,
        model: Option<&str>,
    ) -> Result<Self, AppError> {
        if let Some(provider) = provider {
            config.default_provider = Some(provider.to_string());
        }
        if let Some(model) = model {
            config.default_model = Some(model.to_string());
        }

        let provider_id = config
            .default_provider
            .clone()
            .ok_or(AppError::MissingDefaultProvider)?;
        let model_id = config
            .default_model
            .clone()
            .ok_or(AppError::MissingDefaultModel)?;
        let provider_ref = ProviderId::from(provider_id.as_str());
        let backend: Arc<dyn SecretBackend> = Arc::new(FileBackend::new());
        let reasoning_protector =
            std::sync::Arc::new(crate::protected::SwappableReasoningProtector::in_memory());
        let assembled = futures::executor::block_on(assemble_provider(
            &config,
            &provider_ref,
            &backend,
            false,
            std::sync::Arc::clone(&reasoning_protector)
                as Arc<dyn pawork_providers::ReasoningProtector>,
        ))?;
        let mut core = Self::from_parts_with_protocol(
            assembled.adapter,
            assembled.credential,
            ModelId::from(model_id.as_str()),
            provider_ref,
            assembled.protocol,
            None,
            assembled.registry,
        )
        .with_state(config, backend);
        core.reasoning_protector = reasoning_protector;
        core.http = Self::http_from_config(&core.config)?;
        Ok(core)
    }

    /// 完整装配（async）：OAuth 通道在此执行请求前刷新。
    pub async fn from_config(
        config: PaworkConfig,
        provider: Option<&str>,
        model: Option<&str>,
        backend: Arc<dyn SecretBackend>,
    ) -> Result<Self, AppError> {
        Self::from_config_inner(config, provider, model, backend, false).await
    }

    async fn from_config_inner(
        mut config: PaworkConfig,
        provider: Option<&str>,
        model: Option<&str>,
        backend: Arc<dyn SecretBackend>,
        allow_pending: bool,
    ) -> Result<Self, AppError> {
        if let Some(provider) = provider {
            config.default_provider = Some(provider.to_string());
        }
        if let Some(model) = model {
            config.default_model = Some(model.to_string());
        }
        // 目录/凭证命令允许 default provider/model 缺失：登录前用户可能尚未
        // 写任何配置，此时退化为 CatalogOnly 装配而不是拒绝启动。
        let provider_missing = config.default_provider.is_none();
        if provider_missing {
            if !allow_pending {
                return Err(AppError::MissingDefaultProvider);
            }
        } else if config.default_model.is_none() && !allow_pending {
            return Err(AppError::MissingDefaultModel);
        }
        let provider_id = config
            .default_provider
            .clone()
            .unwrap_or_else(|| "catalog".into());
        let model_id = config.default_model.clone().unwrap_or_else(|| "unset".into());
        let provider_ref = ProviderId::from(provider_id.as_str());
        let channel = channels::first_party_channel(provider_id.as_str());
        let protocol = channel_protocol(channel, &config, provider_id.as_str())?;
        let registry = assemble_registry(&config, &provider_ref, protocol, channel);
        let reasoning_protector =
            std::sync::Arc::new(crate::protected::SwappableReasoningProtector::in_memory());
        let mut pending = false;
        let core = if provider_missing {
            Self::from_parts_with_protocol(
                Arc::new(CatalogOnlyProvider {
                    id: provider_ref.clone(),
                }),
                None,
                ModelId::from(model_id.as_str()),
                provider_ref,
                protocol,
                None,
                registry,
            )
        } else {
            match assemble_provider(
                &config,
                &provider_ref,
                &backend,
                true,
                std::sync::Arc::clone(&reasoning_protector)
                    as Arc<dyn pawork_providers::ReasoningProtector>,
            )
            .await {
                Ok(assembled) => Self::from_parts_with_protocol(
                    assembled.adapter,
                    assembled.credential,
                    ModelId::from(model_id.as_str()),
                    provider_ref,
                    assembled.protocol,
                    None,
                    assembled.registry,
                ),
                Err(err) if allow_pending && is_credential_pending(&err) => {
                    pending = true;
                    emit_missing_credential_degrade(&provider_ref);
                    Self::from_parts_with_protocol(
                        Arc::new(CatalogOnlyProvider {
                            id: provider_ref.clone(),
                        }),
                        None,
                        ModelId::from(model_id.as_str()),
                        provider_ref,
                        protocol,
                        None,
                        registry,
                    )
                }
                Err(err) => return Err(err),
            }
        };
        let mut core = core.with_state(config, backend);
        core.reasoning_protector = reasoning_protector;
        core.http = Self::http_from_config(&core.config)?;
        core.provider_pending = pending;
        Ok(core)
    }

    pub fn from_parts(
        provider: Arc<dyn ModelProvider>,
        credential: Option<ResolvedCredential>,
        model: ModelId,
        provider_id: ProviderId,
        store: Option<SessionStore>,
    ) -> Self {
        Self::from_parts_with_protocol(
            provider,
            credential,
            model,
            provider_id,
            AdapterProtocol::ChatCompletions,
            store,
            ModelRegistry::builtin(),
        )
    }

    fn from_parts_with_protocol(
        provider: Arc<dyn ModelProvider>,
        credential: Option<ResolvedCredential>,
        model: ModelId,
        provider_id: ProviderId,
        adapter_protocol: AdapterProtocol,
        store: Option<SessionStore>,
        registry: ModelRegistry,
    ) -> Self {
        let heuristic = Arc::new(HeuristicEstimator::default());
        let session_estimator: Arc<dyn pawork_storage::session::TokenEstimator> =
            Arc::new(SessionTokenEstimatorBridge(heuristic.clone()));
        Self {
            provider,
            provider_pending: false,
            credential,
            model,
            provider_id,
            config: PaworkConfig::default(),
            backend: Arc::new(MemoryBackend::new()),
            http: reqwest::Client::new(),
            registry: Arc::new(registry),
            heuristic,
            session_estimator,
            adapter_protocol,
            store,
            scheduler: Arc::new(ToolScheduler::new(
                ToolRegistry::new(),
                ToolSchedulerConfig::default(),
            )),
            tool_defs: Vec::new(),
            descriptors: Vec::new(),
            approval: services::approval::ApprovalService::new(),
            session: services::session::SessionService::new(),
            run: services::run::RunService,
            extensions: services::extension::ExtensionService::new(),
            checkpoints: None,
            artifacts: None,
            protected_store: None,
            reasoning_protector: std::sync::Arc::new(
                crate::protected::SwappableReasoningProtector::in_memory(),
            ),
            usage: services::usage::UsageService::in_memory(),
            tasks: services::tasks::TaskService::new(),
            imports: services::import::ImportService,
            next_request: AtomicU64::new(1),
            next_run: AtomicU64::new(1),
            next_session: AtomicU64::new(1),
            next_message: AtomicU64::new(1),
        }
    }

    /// 装配后补全 host 状态（config + 凭证后端）。
    fn with_state(
        mut self,
        config: PaworkConfig,
        backend: Arc<dyn SecretBackend>,
    ) -> Self {
        self.config = config;
        self.backend = backend;
        self
    }

    /// 按全局 `proxy_url` 构造 OAuth/模型探测用 HTTP 客户端。
    ///
    /// 未配置时保持 reqwest 默认（读 `HTTPS_PROXY` 等环境变量）；配置后
    /// 显式代理优先生效，回环/`.local` 目标直连（`loopback_aware_proxy`）。
    fn http_from_config(config: &PaworkConfig) -> Result<reqwest::Client, AppError> {
        // F06: OAuth/探测客户端与 HttpClient 一样禁止跟随跨 origin 跳转
        // （默认政策会带出 x-api-key）。workspace 层 proxy_url 已在 loader 剥离。
        let redirect = reqwest::redirect::Policy::none();
        match &config.proxy_url {
            Some(proxy) => {
                let proxy = pawork_providers::net::http::loopback_aware_proxy(proxy)
                    .map_err(|err| AppError::InvalidProxy(err))?;
                reqwest::Client::builder()
                    .proxy(proxy)
                    .redirect(redirect)
                    .build()
                    .map_err(|err| AppError::InvalidProxy(err.to_string()))
            }
            None => reqwest::Client::builder()
                .redirect(redirect)
                .build()
                .map_err(|err| AppError::InvalidProxy(err.to_string())),
        }
    }

    /// 设置审批模式、workspace 信任与决策宿主。须在 [`Self::attach_workspace`] 之前调用。
    pub fn configure_approval(
        &mut self,
        mode: ApprovalMode,
        workspace_trusted: bool,
        host: Arc<dyn ApprovalPromptHost>,
    ) {
        self.approval.configure(mode, workspace_trusted, host);
    }

    /// 把启动目录登记为默认 workspace root，并注册只读四件 + 写三件 + run_command。
    pub fn attach_workspace(&mut self, root: &Path) -> Result<(), AppError> {
        let workspaces = WorkspaceService::new();
        let workspace_id = WorkspaceId::from("ws-default");
        workspaces.add(workspace_id.clone(), "default", [root.to_path_buf()])?;
        self.install_builtin_tools(&workspaces)?;
        self.extensions.resource_loader =
            Some(services::extension::ExtensionService::resource_loader_for(
                workspaces.clone(),
            ));
        self.extensions.file_index = services::extension::ExtensionService::new_file_index();
        self.extensions.workspaces = workspaces;
        self.extensions.workspace_id = workspace_id;
        self.extensions.workspace_name = "default".into();
        self.extensions.workspace_roots = vec![root.to_path_buf()];
        Ok(())
    }

    pub async fn open_checkpoints(&mut self, root: impl AsRef<Path>) -> Result<(), AppError> {
        let root = root.as_ref();
        std::fs::create_dir_all(root)?;
        let store = pawork_storage::blob::ArtifactStore::open(root).await?;
        let checkpoints = pawork_storage::blob::CheckpointService::open(store.clone()).await?;
        self.artifacts = Some(store);
        self.checkpoints = Some(checkpoints);
        Ok(())
    }

    pub async fn open_store(&mut self, path: impl AsRef<Path>) -> Result<(), AppError> {
        if let Some(parent) = path.as_ref().parent() {
            std::fs::create_dir_all(parent)?;
        }
        let (store, _) = SessionStore::open(path.as_ref()).await?;
        self.store = Some(store);
        Ok(())
    }

    pub fn open_control_plane(&mut self, dir: impl AsRef<Path>) -> Result<(), AppError> {
        let dir = dir.as_ref();
        self.usage.control = control::ControlPlaneRuntime::persistent(dir)?;
        self.open_tasks(dir.join("tasks.json"))?;
        Ok(())
    }

    pub fn store(&self) -> Result<&SessionStore, AppError> {
        self.store.as_ref().ok_or(AppError::StoreNotOpen)
    }

    pub fn provider_id(&self) -> &ProviderId {
        &self.provider_id
    }

    pub fn model(&self) -> &ModelId {
        &self.model
    }

    pub fn adapter_protocol(&self) -> AdapterProtocol {
        self.adapter_protocol
    }

    pub fn config(&self) -> &PaworkConfig {
        &self.config
    }

    pub fn auth_backend(&self) -> &Arc<dyn SecretBackend> {
        &self.backend
    }

    /// 默认 provider 是否因缺凭证未装配（目录兜底模式）。
    pub fn provider_pending(&self) -> bool {
        self.provider_pending
    }

    pub fn workspace_id(&self) -> &WorkspaceId {
        &self.extensions.workspace_id
    }

    pub fn workspace_name(&self) -> &str {
        &self.extensions.workspace_name
    }

    pub fn workspace_trusted(&self) -> bool {
        self.approval.workspace_trusted()
    }

    /// 记录 SessionCreate 带来的 canonical workspace（进程内）。
    pub fn bind_session_workspace(&self, session_id: &SessionId, workspace_id: WorkspaceId) {
        self.session.bind_workspace(session_id, workspace_id);
    }

    pub fn session_workspace(&self, session_id: &SessionId) -> Option<WorkspaceId> {
        self.session.workspace(session_id)
    }

    pub fn session_workspace_for_record(&self, session_id: &str) -> Option<WorkspaceId> {
        self.session.workspace_for_record(session_id)
    }

    pub fn approval_mode(&self) -> ApprovalMode {
        self.approval.mode()
    }

    pub fn approval_host(&self) -> Arc<dyn ApprovalPromptHost> {
        self.approval.host()
    }

    pub fn tool_names(&self) -> Vec<&str> {
        self.tool_defs
            .iter()
            .map(|tool| tool.name.as_str())
            .collect()
    }

    pub async fn create_session(&self, title: impl Into<String>) -> Result<SessionId, AppError> {
        self.session.create_session(self, title).await
    }

    /// GUI SessionCreate：落盘会话并绑定 command 里的 workspace_id。
    pub async fn create_session_with_workspace(
        &self,
        title: impl Into<String>,
        workspace_id: WorkspaceId,
    ) -> Result<SessionId, AppError> {
        self.session
            .create_session_with_workspace(self, title, workspace_id)
            .await
    }

    pub async fn list_sessions(&self) -> Result<Vec<SessionRecord>, AppError> {
        self.session.list_sessions(self).await
    }

    pub async fn get_session(&self, session_id: &SessionId) -> Result<SessionRecord, AppError> {
        self.session.get_session(self, session_id).await
    }

    pub async fn resume_messages(&self, session_id: &SessionId) -> Result<Vec<Message>, AppError> {
        self.session.resume_messages(self, session_id).await
    }

    pub async fn resume_messages_keep_pending(
        &self,
        session_id: &SessionId,
    ) -> Result<Vec<Message>, AppError> {
        self.session
            .resume_messages_keep_pending(self, session_id)
            .await
    }

    pub(crate) async fn resolve_waiting_tool_call(
        &self,
        session_id: &SessionId,
        call: &pawork_storage::session::ProjectedToolCall,
        decision: ApprovalDecision,
        comment: &str,
        sequence: &mut u64,
    ) -> Result<(), AppError> {
        self.session
            .resolve_waiting_tool_call(self, session_id, call, decision, comment, sequence)
            .await
    }

    async fn session_active_branch(&self, session_id: &SessionId) -> Result<String, AppError> {
        self.session.session_active_branch(self, session_id).await
    }

    pub async fn next_sequence(&self, session_id: &SessionId) -> Result<u64, AppError> {
        self.session.next_sequence(self, session_id).await
    }

    /// `latest`、完整 id，或唯一前缀。多命中 fail-closed。
    pub async fn resolve_session(&self, spec: &str) -> Result<SessionId, AppError> {
        self.session.resolve_session(self, spec).await
    }

    /// resume / 计划 / 审批收口共用的追补事件入口（persist-first）。
    pub(crate) async fn append_payload(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
        sequence: &mut u64,
        payload: AgentEvent,
    ) -> Result<(), AppError> {
        self.run
            .append_payload(self, session_id, run_id, sequence, payload)
            .await
    }

    pub async fn chat_turn(
        &self,
        session_id: &SessionId,
        messages: Vec<Message>,
        render: &dyn AgentEventSink,
        cancel: CancellationToken,
    ) -> Result<ModelResponseSummary, AppError> {
        self.run
            .chat_turn(self, session_id, messages, render, cancel)
            .await
    }

    /// 以调用方提供的 run_id 执行一轮（GUI 需要在启动前登记取消令牌并
    /// 向客户端回报 run_id，因此 run id 的分配权上移到宿主）。
    pub async fn chat_turn_with_run_id(
        &self,
        run_id: RunId,
        session_id: &SessionId,
        messages: Vec<Message>,
        render: &dyn AgentEventSink,
        cancel: CancellationToken,
    ) -> Result<ModelResponseSummary, AppError> {
        self.run
            .chat_turn_with_run_id(self, run_id, session_id, messages, render, cancel)
            .await
    }

    pub(crate) async fn projected_run_usage(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
    ) -> Option<TokenUsage> {
        self.usage
            .projected_run_usage(self, session_id, run_id)
            .await
    }

    pub(crate) async fn record_completed_usage(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
        request_id: &RequestId,
        usage: &TokenUsage,
    ) -> Result<(), AppError> {
        self.usage
            .record_completed_usage(self, session_id, run_id, request_id, usage)
            .await
    }

    pub async fn usage_overview(
        &self,
        provider_id: Option<&str>,
        session: Option<&SessionId>,
    ) -> Result<UsageOverview, AppError> {
        self.usage.usage_overview(self, provider_id, session).await
    }

    pub async fn list_models(&self) -> Result<Vec<ModelDefinition>, ProviderError> {
        self.provider.list_models(self.credential.as_ref()).await
    }

    /// 本会话模型的上下文配置：registry 解析 window / max_output 推导预算；
    /// 目录无条目或 window 为 0 时退回禁用（与 S5 前行为一致，不编造窗口）。
    pub fn turn_context(&self) -> TurnContext {
        let some_estimator = || TurnContext {
            estimator: Some(self.heuristic.clone()),
            ..TurnContext::default()
        };
        let Some(entry) = self.registry.resolve(self.model.as_str()) else {
            return some_estimator();
        };
        if entry.context_window_tokens == 0 {
            return some_estimator();
        }
        let output_reserve = if entry.max_output_tokens > 0 {
            entry.max_output_tokens
        } else {
            4_096
        };
        let budget = ContextBudget::from_context_window(
            entry.context_window_tokens,
            output_reserve,
            0,
        );
        // 软限 = 硬限的 80%：提前压缩，避免贴着上限触发 provider 4xx。
        let soft_limit = budget.max_input_tokens / 5 * 4;
        TurnContext {
            limits: Some(ContextLimits {
                budget,
                history_soft_limit_tokens: Some(soft_limit),
            }),
            estimator: Some(self.heuristic.clone()),
            retained_messages: RETAINED_MESSAGES,
            injected_layers: Vec::new(),
        }
    }

    /// 会话累计用量：对已完成 run 的 RunCompleted.usage 求和（投影可重建）。
    pub async fn session_usage(&self, session_id: &SessionId) -> Result<TokenUsage, AppError> {
        self.usage.session_usage(self, session_id).await
    }

    /// 最近一次完成 run 的用量（CLI 每轮尾部行）。
    pub async fn last_run_usage(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<TokenUsage>, AppError> {
        self.usage.last_run_usage(self, session_id).await
    }

    /// 按 registry 定价估算费用；无定价条目返回 None（不编造）。
    pub fn estimate_cost_for(&self, model: &ModelId, usage: &TokenUsage) -> Option<Cost> {
        self.usage.estimate_cost_for(self, model, usage)
    }

    /// 手动压缩（REPL /compact）：与自动链同一 engine 函数与事件序，
    /// persist-first 落 CompactionStarted / MessageCommitted(summary) /
    /// CompactionCompleted；返回重建后的消息列表。
    pub async fn compact_session(
        &self,
        session_id: &SessionId,
        render: &dyn AgentEventSink,
        cancel: CancellationToken,
    ) -> Result<Vec<Message>, AppError> {
        self.run
            .compact_session(self, session_id, render, cancel)
            .await
    }

    pub async fn session_diff(&self, session_id: &SessionId) -> Result<SessionDiff, AppError> {
        crate::diff::session_diff(self, session_id).await
    }

    pub async fn list_checkpoints(
        &self,
        session_id: &SessionId,
    ) -> Result<Vec<CheckpointSummary>, AppError> {
        let Some(service) = self.checkpoints.as_ref() else {
            return Ok(Vec::new());
        };
        let run_ids = crate::checkpoint::session_run_ids(self, session_id).await?;
        let runs = crate::checkpoint::run_checkpoints(service, &run_ids);
        Ok(crate::checkpoint::summaries_from_runs(&runs))
    }

    pub async fn rollback(
        &self,
        session_id: &SessionId,
        spec: &str,
    ) -> Result<RollbackOutcome, AppError> {
        let service = self
            .checkpoints
            .as_ref()
            .ok_or(AppError::CheckpointStoreNotOpen)?;
        let listed = self.list_checkpoints(session_id).await?;
        let resolved = crate::checkpoint::resolve_spec(&listed, spec)?;
        let restored = crate::checkpoint::perform_rollback(service, &resolved).await?;
        crate::checkpoint::persist_rolled_back(self, session_id, &resolved).await?;
        Ok(RollbackOutcome {
            checkpoint_id: resolved.checkpoint_id.as_str().to_string(),
            restored: restored
                .into_iter()
                .map(|file| file.relative_path)
                .collect(),
        })
    }

    pub async fn shutdown(self) -> Result<(), AppError> {
        self.shutdown_mcp().await;
        if let Some(store) = self.store {
            store.shutdown().await?;
        }
        if let Some(artifacts) = self.artifacts {
            artifacts.shutdown().await?;
        }
        Ok(())
    }
}

pub fn session_title_from_text(text: &str) -> String {
    let collapsed: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    const MAX: usize = 72;
    if collapsed.chars().count() <= MAX {
        if collapsed.is_empty() {
            "New session".into()
        } else {
            collapsed
        }
    } else {
        let mut title: String = collapsed.chars().take(MAX.saturating_sub(1)).collect();
        title.push('…');
        title
    }
}

fn workspace_root_from_config_file(workspace_file: Option<&Path>) -> Option<PathBuf> {
    let path = workspace_file?;
    let parent = path.parent()?;
    if parent.file_name().and_then(|name| name.to_str()) == Some(".pawork") {
        parent.parent().map(Path::to_path_buf)
    } else {
        Some(parent.to_path_buf())
    }
}

#[cfg(test)]
mod tests {
    use pawork_domain::{
        AgentEvent, ContentPart, MessageRole, ProviderStreamEvent, StopReason, TokenUsage,
    };

    use super::*;
    use crate::testsupport::*;
    use crate::provider_assembly::apply_config_models;

    // RecordingEvents / ScriptedProvider / mock_core 等共享测试装配
    // 已随服务抽取迁至 crate::testsupport。

    #[tokio::test]
    async fn auth_status_masks_and_prefers_file_over_env() {
        let env_name = "PAWORK_API_KEY_GLM_CODING";
        let secret = "sk-app-auth-mask-1234567890abcdef";
        set_env(env_name, secret);
        let core = AppCore::from_parts(
            Arc::new(ScriptedProvider {
                events: Vec::new(),
                summary: ModelResponseSummary {
                    stop_reason: StopReason::Completed,
                    usage: TokenUsage::default(),
                    response_id: None,
                    provider_metadata: Default::default(),
                },
                models: Vec::new(),
            }),
            None,
            ModelId::from("glm-5.2"),
            ProviderId::from("glm-coding"),
            None,
        )
        .with_state(
            PaworkConfig::default(),
            Arc::new(pawork_auth::MemoryBackend::new()),
        );
        let glm = |core: &AppCore| {
            core.auth_status()
                .into_iter()
                .find(|row| row.provider == "glm-coding")
                .expect("glm-coding status")
        };
        let env_row = glm(&core);
        assert_eq!(env_row.source.as_str(), "env");
        assert!(env_row.masked.is_none(), "env source must not display value");

        core.auth_set_key("glm-coding", secret).expect("set key");
        let file_row = glm(&core);
        assert_eq!(file_row.source.as_str(), "file");
        let masked = file_row.masked.as_deref().expect("masked");
        assert!(!masked.contains(secret), "masked leaks secret: {masked}");

        core.auth_logout("glm-coding").expect("logout");
        remove_env(env_name);
        let logged_out = glm(&core);
        assert_eq!(logged_out.source.as_str(), "none");
    }

    #[test]
    fn turn_context_derives_from_registry_with_config_override() {
        let mut registry = ModelRegistry::builtin();
        apply_config_models(
            &mut registry,
            &[pawork_workspace::config::ModelConfig {
                id: "glm-5.2".into(),
                context_window: Some(200_000),
                max_output: Some(16_384),
            }],
            &ProviderId::from("mock"),
        );
        let core = core_with_registry(registry, "glm-5.2");
        let context = core.turn_context();
        let limits = context.limits.expect("registry entry enables limits");
        assert_eq!(limits.budget.context_window_tokens, 200_000);
        assert_eq!(limits.budget.max_input_tokens, 183_616);
        // 软限 = 硬限 80%（整数运算：183_616 / 5 * 4）。
        assert_eq!(limits.history_soft_limit_tokens, Some(146_892));
        assert!(context.estimator.is_some());

        // 目录无条目：不编造窗口（limits 仍关），但保留估算器以便注入计入 ContextPrepared。
        let core = core_with_registry(ModelRegistry::builtin(), "no-such-model");
        let context = core.turn_context();
        assert!(context.limits.is_none());
        assert!(context.estimator.is_some());

        // config 覆盖 window=0：显式未知同样不启用压缩，但仍估算。
        let mut zero_window = ModelRegistry::builtin();
        apply_config_models(
            &mut zero_window,
            &[pawork_workspace::config::ModelConfig {
                id: "glm-5.2".into(),
                context_window: Some(0),
                max_output: None,
            }],
            &ProviderId::from("mock"),
        );
        let core = core_with_registry(zero_window, "glm-5.2");
        let context = core.turn_context();
        assert!(context.limits.is_none());
        assert!(context.estimator.is_some());
    }


    #[tokio::test]
    async fn compact_session_commits_summary_and_replaces_projection() {
        let (core, _dir) = mock_core(vec![
            ProviderStreamEvent::TextDelta("folded-history".into()),
            ProviderStreamEvent::ResponseCompleted(StopReason::Completed),
        ])
        .await;
        let session = core.create_session("compact").await.expect("create");
        let sink = RecordingEvents::default();
        for _ in 0..3 {
            core.chat_turn(
                &session,
                vec![user_hello()],
                &sink,
                CancellationToken::new(),
            )
            .await
            .expect("turn");
        }
        let before = core.resume_messages(&session).await.expect("before");
        assert_eq!(before.len(), 6);

        let rebuilt = core
            .compact_session(&session, &sink, CancellationToken::new())
            .await
            .expect("compact");
        // 重建结果 = [summary] + retained tail(4)。
        assert_eq!(rebuilt.len(), 5);
        // V1 语义：摘要以 User 角色作为上下文消息提交。
        assert_eq!(rebuilt[0].role, MessageRole::User);
        let summary_text: String = rebuilt[0]
            .content
            .iter()
            .filter_map(|part| match part {
                ContentPart::Text(text) => Some(text.text.as_str()),
                _ => None,
            })
            .collect();
        assert!(summary_text.contains("folded-history"), "{summary_text}");

        let envelopes = sink.0.lock().expect("mutex").clone();
        let started = envelopes
            .iter()
            .position(|envelope| {
                matches!(&envelope.payload, AgentEvent::CompactionStarted { .. })
            })
            .expect("CompactionStarted");
        let completed = envelopes
            .iter()
            .position(|envelope| {
                matches!(&envelope.payload, AgentEvent::CompactionCompleted { .. })
            })
            .expect("CompactionCompleted");
        assert!(started < completed);
        assert!(envelopes[started..completed].iter().any(|envelope| {
            matches!(
                &envelope.payload,
                AgentEvent::MessageCommitted { message } if message.role == MessageRole::User,
            )
        }));

        // 投影替换：早期消息被折叠，事件流保留可重放。
        let after = core.resume_messages(&session).await.expect("after");
        assert_eq!(after.len(), 5);
        // 投影按 sequence 排序：保留尾部（u2..a3）在前，摘要最后追加。
        assert_eq!(after[0].role, MessageRole::User);
        let summary = after.last().expect("summary");
        assert_eq!(summary.role, MessageRole::User);
        let summary_text: String = summary
            .content
            .iter()
            .filter_map(|part| match part {
                ContentPart::Text(text) => Some(text.text.as_str()),
                _ => None,
            })
            .collect();
        assert!(summary_text.contains("folded-history"), "{summary_text}");
        // 3 = retained 的 2 条用户消息 + summary(User)。
        assert_eq!(
            after
                .iter()
                .filter(|message| message.role == MessageRole::User)
                .count(),
            3
        );
        let replayed = core
            .store()
            .expect("store")
            .replay_events(&session, 1, 64)
            .await
            .expect("replay");
        assert!(replayed.iter().any(|envelope| {
            matches!(&envelope.payload, AgentEvent::CompactionCompleted { .. })
        }));
        assert!(replayed.len() > 6, "event stream keeps original messages");
        core.shutdown().await.expect("shutdown");
    }

    #[test]
    fn session_title_truncates() {
        assert_eq!(session_title_from_text("  hello   world  "), "hello world");
        assert_eq!(session_title_from_text(""), "New session");
        let long = "x".repeat(80);
        let title = session_title_from_text(&long);
        assert!(title.ends_with('…'));
        assert_eq!(title.chars().count(), 72);
    }

    #[tokio::test]
    async fn plan_gate_blocks_unapproved_turn_and_resumes_after_approve() {
        let (core, _dir) = mock_core(vec![
            ProviderStreamEvent::TextDelta("ok".into()),
            ProviderStreamEvent::ResponseCompleted(StopReason::Completed),
        ])
        .await;
        let session = core.create_session("plan-gate").await.expect("create");
        core.plan_create(
            &session,
            "safe edit",
            vec!["read file".into(), "write file".into()],
        )
        .await
        .expect("create plan");
        let blocked = core
            .chat_turn(
                &session,
                vec![user_hello()],
                &RecordingEvents::default(),
                CancellationToken::new(),
            )
            .await
            .expect_err("unapproved plan must block");
        assert!(
            matches!(blocked, AppError::PlanNotApproved { .. }),
            "{blocked}"
        );

        core.plan_approve(&session).await.expect("approve");
        core.chat_turn(
            &session,
            vec![user_hello()],
            &RecordingEvents::default(),
            CancellationToken::new(),
        )
        .await
        .expect("approved plan allows turn");

        core.plan_replace(
            &session,
            "changed",
            vec!["new step".into()],
        )
        .await
        .expect("replace");
        let blocked_again = core
            .chat_turn(
                &session,
                vec![user_hello()],
                &RecordingEvents::default(),
                CancellationToken::new(),
            )
            .await
            .expect_err("replaced plan must block");
        assert!(matches!(blocked_again, AppError::PlanNotApproved { .. }));
        core.shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    async fn multi_agent_demo_cancel_tree_and_budget_gate() {
        let (core, _dir) = mock_core(Vec::new()).await;
        let cancelled = core
            .run_multi_agent_demo(crate::MultiAgentDemoOptions {
                cancel: true,
                budget_input_tokens: None,
            })
            .await
            .expect("cancel demo");
        assert_eq!(cancelled.workers.len(), 2);
        assert!(!cancelled.cancelled.is_empty());
        assert!(cancelled
            .event_kinds
            .iter()
            .any(|kind| kind == "WorkerCancelled"));

        let budget = core
            .run_multi_agent_demo(crate::MultiAgentDemoOptions {
                cancel: false,
                budget_input_tokens: Some(1),
            })
            .await
            .expect("budget demo");
        assert!(budget.budget_exceeded, "{budget:?}");
        core.shutdown().await.expect("shutdown");
    }

    #[test]
    fn load_with_home_fallback_consumes_degrade_and_warns_once() {
        let expected = PathBuf::from("/tmp/process-temp/pawork");
        let outcome = crate::data_dir::data_dir_outcome_for_test(
            None,
            None,
            None,
            expected.parent().expect("parent").to_path_buf(),
        );
        assert!(outcome.degrade.is_some(), "HOME fallback must produce DegradeEvent");
        let subscriber = crate::testsupport::RecordingSubscriber::new();
        let path = tracing::subscriber::with_default(subscriber.clone(), || {
            crate::data_dir::consume_data_dir_outcome(outcome.clone())
        });
        assert_eq!(path, expected);
        let events = subscriber.events();
        let emitted: Vec<_> = events
            .iter()
            .filter(|event| {
                event.fields.get("code").map(String::as_str) == Some("degrade.home_dir_fallback")
            })
            .collect();
        assert_eq!(emitted.len(), 1, "load_with consumer must warn once: {events:?}");
        let emitted = emitted[0];
        assert_eq!(emitted.level, "WARN");
        assert!(emitted.message.contains("HOME is unset"), "{emitted:?}");
        assert_eq!(
            emitted.fields.get("severity").map(String::as_str),
            Some("warning"),
            "{emitted:?}"
        );
        assert_eq!(
            emitted.fields.get("path").map(String::as_str),
            Some(expected.display().to_string().as_str()),
            "{emitted:?}"
        );

        let subscriber = crate::testsupport::RecordingSubscriber::new();
        tracing::subscriber::with_default(subscriber.clone(), || {
            let _ = crate::data_dir::consume_data_dir_outcome(crate::DataDirOutcome {
                path: expected.clone(),
                degrade: None,
            });
        });
        assert!(
            subscriber.events().iter().all(|event| {
                event.fields.get("code").map(String::as_str) != Some("degrade.home_dir_fallback")
            }),
            "consume_data_dir_outcome must stay silent when degrade is absent: {:?}",
            subscriber.events()
        );
    }
}
