//! 应用门面：读配置 → 凭证链（auth 文件 → env）→ provider → 读写工具 +
//! run_command → 事件化 `run_session`（S6 波 C 起六通道正式装配）。
//!
//! 不按 Provider 名称分支；协议来自 `extra.provider_protocols` 与默认表。
//! 落库 persist-first，再推渲染 sink。

mod approval;
mod auth;
mod channels;
mod checkpoint;
mod data_dir;
mod diff;
mod extensions;
mod gui_host;
mod hub;
mod idempotency;
mod import_host;
mod loop_ctx;
mod persist;
mod protocol;
mod rate_limit;

use async_trait::async_trait;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use pawork_api::{
    CanonicalModelRequest, ModelDefinition, ModelProvider, ModelResponseSummary, ProviderError,
    ProviderErrorKind, ProviderEventSink, ResolvedCredential, ToolDefinition,
};
use pawork_config::{
    api_key_env_name, ConfigError, Loader, PaworkConfig, ProviderConfig,
};
use pawork_domain::{
    AgentEvent, AgentEventEnvelope, ApprovalDecision, CancellationToken, ContentPart, EventId,
    EventSequence, Message, MessageId, MessageRole, ModelId, ProviderId, RequestId, RunId,
    SessionId, TextContent, ToolDescriptor, ToolResultContent, WorkspaceId, TokenUsage, Cost,
};
use pawork_engine::{
    assemble_request, assemble_request_with_tools, run_manual_compaction, run_session,
    AgentEventSink, ContextBudget, ContextLimits, EngineError, HeuristicEstimator, SessionTurn,
    TokenEstimator as EngineTokenEstimator, TurnContext, DEFAULT_MAX_TOOL_ROUNDS,
};
use pawork_providers::{
    AnthropicConfig, AnthropicProvider, ApiKeyChannelConfig, ApiKeyChannelProvider,
    OpenAiCompatibleConfig, OpenAiCompatibleProvider,
};
use pawork_auth::{
    load_default_oauth_credential, load_default_oauth_meta,
    refresh_default_oauth_credential_if_needed, resolve_oauth_credential,
    resolve_provider_credential, ApiKeyCredential, AuthError, CredentialSource, FileBackend,
    MemoryBackend, OAuthRefreshConfig, SecretBackend,
};
use pawork_policy::PolicyEngine;
use pawork_provider_core::{CatalogEntry, ModelRegistry};
use pawork_session::{SessionStore, SessionStoreError, DEFAULT_BRANCH_ID};
use pawork_tools::{ToolRegistry, ToolRegistryError, ToolScheduler, ToolSchedulerConfig};
use pawork_workspace::{FileIndex, FileIndexError, WorkspaceError, WorkspaceService};
use thiserror::Error;

use crate::loop_ctx::SessionLoopCtx;
use crate::protocol::resolve_adapter_protocol;

pub use approval::{
    parse_approval_mode, ApprovalAsk, ApprovalPromptHost, DenyAllApprovals, GuiApprovalHost,
    PendingToolApproval,
};
pub use checkpoint::{CheckpointSummary, RollbackOutcome};
pub use data_dir::{
    artifact_store_path, artifact_store_path_for, default_data_dir, instance_dir,
    normalize_instance, session_db_path, session_db_path_for, DEFAULT_INSTANCE,
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
pub use rate_limit::{
    DeltaKind, RateLimiter, RateLimiterStats, DEFAULT_RATE_LIMIT_BUFFER, DEFAULT_RATE_LIMIT_WINDOW,
};
pub use protocol::{AdapterProtocol, ProtocolError};
pub use auth::{AuthChannelStatus, AuthSource, OAuthLogin};
pub use channels::{
    first_party_channel, is_first_party, ChannelKind, FirstPartyChannel, FIRST_PARTY_CHANNELS,
};
pub use extensions::{AtAttachment, McpServerStatus};
pub use import_host::{
    parse_session_source, CompatImportItemView, CompatImportPreview, CompatImportReport,
    CompatTool, SessionImportFormat, SessionImportOutcome,
};
pub use pawork_compat::ExternalSource as CompatExternalSource;
pub use pawork_policy::{ApprovalMode, RiskLevel};
pub use pawork_session::{SessionExport, SessionRecord, EXPORT_SCHEMA_VERSION};

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
    Checkpoint(#[from] pawork_blob_store::CheckpointError),
    #[error(transparent)]
    Artifact(#[from] pawork_blob_store::ArtifactStoreError),
    #[error(transparent)]
    Git(#[from] pawork_git::GitError),
    #[error("checkpoint store is not open")]
    CheckpointStoreNotOpen,
    #[error("checkpoint not found: {0}")]
    CheckpointNotFound(String),
    #[error("ambiguous checkpoint `{prefix}` matches: {matches}")]
    AmbiguousCheckpoint { prefix: String, matches: String },
    #[error(transparent)]
    Mcp(#[from] pawork_mcp::McpError),
    #[error(transparent)]
    Resources(#[from] pawork_resources::ResourceLoadError),
    #[error(transparent)]
    Compat(#[from] pawork_compat::error::CompatError),
    #[error(transparent)]
    FileIndex(#[from] FileIndexError),
    #[error("{0}")]
    Import(String),
}

/// S5 压缩在 engine 侧保留的最近消息条数；session 侧保留策略按
/// `RETAINED_MESSAGES / 2` 轮对齐同一折叠边界。
pub(crate) const RETAINED_MESSAGES: usize = 4;

/// 目录兜底 provider：默认 provider 缺凭证时的占位（list 空目录、stream
/// fail-closed）。只在 host 装配层使用，Engine 无感知。
struct CatalogOnlyProvider {
    id: ProviderId,
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

/// 目录命令（models/auth/sessions）装配时容忍的「凭证缺失」错误族；
/// 其余错误（配置、协议、provider 未知）仍然 fail-closed。
fn is_credential_pending(err: &AppError) -> bool {
    matches!(
        err,
        AppError::MissingCredential { .. }
            | AppError::OAuthLoginRequired(_)
            | AppError::OAuthLogin(_)
            | AppError::Auth(_)
    )
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
    session_estimator: Arc<dyn pawork_session::TokenEstimator>,
    adapter_protocol: AdapterProtocol,
    store: Option<SessionStore>,
    scheduler: Arc<ToolScheduler>,
    workspace_id: WorkspaceId,
    /// `attach_workspace` 登记的显示名；未挂载时为 `unbound`。
    workspace_name: String,
    /// 进程内 session → workspace 绑定。不改 S1 sessions DDL；
    /// Host 重启后无绑定的历史 session 进 Unassigned。
    session_workspaces: Mutex<HashMap<String, WorkspaceId>>,
    tool_defs: Vec<ToolDefinition>,
    descriptors: Vec<ToolDescriptor>,
    approval_mode: ApprovalMode,
    workspace_trusted: bool,
    approval_host: Arc<dyn ApprovalPromptHost>,
    pub(crate) workspace_roots: Vec<PathBuf>,
    pub(crate) workspaces: WorkspaceService,
    pub(crate) file_index: FileIndex,
    pub(crate) resource_loader: Option<pawork_resources::ResourceLoader>,
    pub(crate) mcp_servers: Vec<extensions::McpServerSlot>,
    pub(crate) checkpoints: Option<pawork_blob_store::CheckpointService>,
    pub(crate) artifacts: Option<pawork_blob_store::ArtifactStore>,
    next_request: AtomicU64,
    next_run: AtomicU64,
    next_session: AtomicU64,
    next_message: AtomicU64,
}

/// 把 engine 的完整估算器桥接到 session 侧窄口 trait（依赖倒置的宿主实现）。
struct SessionTokenEstimatorBridge(Arc<HeuristicEstimator>);

impl pawork_session::TokenEstimator for SessionTokenEstimatorBridge {
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
            .field("approval_mode", &self.approval_mode)
            .field("workspace_trusted", &self.workspace_trusted)
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
        let data_dir = options.data_dir.unwrap_or_else(default_data_dir);
        let instance = if options.instance.trim().is_empty() {
            crate::DEFAULT_INSTANCE
        } else {
            crate::normalize_instance(&options.instance).map_err(AppError::InvalidInstance)?
        };
        core.open_store(session_db_path_for(&data_dir, instance))
            .await?;
        core.open_checkpoints(artifact_store_path_for(&data_dir, instance))
            .await?;
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
        let assembled = futures::executor::block_on(assemble_provider(
            &config,
            &provider_ref,
            &backend,
            false,
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
            match assemble_provider(&config, &provider_ref, &backend, true).await {
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
        let session_estimator: Arc<dyn pawork_session::TokenEstimator> =
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
            workspace_id: WorkspaceId::from("ws-unbound"),
            workspace_name: "unbound".into(),
            session_workspaces: Mutex::new(HashMap::new()),
            tool_defs: Vec::new(),
            descriptors: Vec::new(),
            approval_mode: ApprovalMode::ReadOnly,
            workspace_trusted: false,
            approval_host: Arc::new(DenyAllApprovals),
            workspace_roots: Vec::new(),
            workspaces: WorkspaceService::new(),
            file_index: Self::new_file_index(),
            resource_loader: None,
            mcp_servers: Vec::new(),
            checkpoints: None,
            artifacts: None,
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
        match &config.proxy_url {
            Some(proxy) => {
                let proxy = pawork_net::http::loopback_aware_proxy(proxy)
                    .map_err(|err| AppError::InvalidProxy(err))?;
                reqwest::Client::builder()
                    .proxy(proxy)
                    .build()
                    .map_err(|err| AppError::InvalidProxy(err.to_string()))
            }
            None => Ok(reqwest::Client::new()),
        }
    }

    /// 设置审批模式、workspace 信任与决策宿主。须在 [`Self::attach_workspace`] 之前调用。
    pub fn configure_approval(
        &mut self,
        mode: ApprovalMode,
        workspace_trusted: bool,
        host: Arc<dyn ApprovalPromptHost>,
    ) {
        self.approval_mode = mode;
        self.workspace_trusted = workspace_trusted;
        self.approval_host = host;
    }

    /// 把启动目录登记为默认 workspace root，并注册只读四件 + 写三件 + run_command。
    pub fn attach_workspace(&mut self, root: &Path) -> Result<(), AppError> {
        let workspaces = WorkspaceService::new();
        let workspace_id = WorkspaceId::from("ws-default");
        workspaces.add(workspace_id.clone(), "default", [root.to_path_buf()])?;
        self.install_builtin_tools(&workspaces)?;
        self.resource_loader = Some(Self::resource_loader_for(workspaces.clone()));
        self.file_index = Self::new_file_index();
        self.workspaces = workspaces;
        self.workspace_id = workspace_id;
        self.workspace_name = "default".into();
        self.workspace_roots = vec![root.to_path_buf()];
        Ok(())
    }

    pub async fn open_checkpoints(&mut self, root: impl AsRef<Path>) -> Result<(), AppError> {
        let root = root.as_ref();
        std::fs::create_dir_all(root)?;
        let store = pawork_blob_store::ArtifactStore::open(root).await?;
        let checkpoints = pawork_blob_store::CheckpointService::open(store.clone()).await?;
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
        &self.workspace_id
    }

    pub fn workspace_name(&self) -> &str {
        &self.workspace_name
    }

    pub fn workspace_trusted(&self) -> bool {
        self.workspace_trusted
    }

    /// 记录 SessionCreate 带来的 canonical workspace（进程内）。
    pub fn bind_session_workspace(&self, session_id: &SessionId, workspace_id: WorkspaceId) {
        self.session_workspaces
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(session_id.as_str().to_string(), workspace_id);
    }

    pub fn session_workspace(&self, session_id: &SessionId) -> Option<WorkspaceId> {
        self.session_workspaces
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(session_id.as_str())
            .cloned()
    }

    pub fn session_workspace_for_record(&self, session_id: &str) -> Option<WorkspaceId> {
        self.session_workspace(&SessionId::from(session_id))
    }

    pub fn approval_mode(&self) -> ApprovalMode {
        self.approval_mode
    }

    pub fn approval_host(&self) -> Arc<dyn ApprovalPromptHost> {
        Arc::clone(&self.approval_host)
    }

    /// 当前 provider 在 registry 的静态目录（REPL /model 列表用）。
    pub fn provider_models(&self) -> Vec<CatalogEntry> {
        self.registry
            .list()
            .into_iter()
            .filter(|entry| entry.provider == self.provider_id)
            .cloned()
            .collect()
    }

    /// 会话中途切换模型：后续轮走新模型；有活动 session 时事件流记录变更。
    pub async fn switch_model(
        &mut self,
        session: Option<&SessionId>,
        model: &str,
    ) -> Result<(), AppError> {
        let entry = match self.registry.resolve(model).cloned() {
            Some(entry) => entry,
            // 静态目录未登记：向当前 provider 探测一次，命中则惰性合并
            //（与 pawork models 展示一致），未命中仍 fail-closed。
            None => {
                let definitions = self
                    .provider
                    .list_models(self.credential.as_ref())
                    .await
                    .unwrap_or_default();
                let entry = definitions
                    .iter()
                    .find(|definition| definition.id.as_str() == model)
                    .map(|definition| CatalogEntry {
                        id: definition.id.clone(),
                        provider: self.provider_id.clone(),
                        display_name: definition.display_name.clone(),
                        context_window_tokens: definition.context_window_tokens,
                        max_output_tokens: definition.max_output_tokens,
                        capabilities: definition.capabilities.clone(),
                        pricing: None,
                        aliases: Vec::new(),
                    })
                    .ok_or_else(|| AppError::UnknownModel {
                        model: model.to_string(),
                        provider: self.provider_id.as_str().to_string(),
                    })?;
                let registry = std::sync::Arc::make_mut(&mut self.registry);
                registry.extend_with(vec![entry.clone()]);
                entry
            }
        };
        if entry.provider != self.provider_id {
            return Err(AppError::ModelBelongsToProvider {
                model: model.to_string(),
                owner: entry.provider.as_str().to_string(),
                current: self.provider_id.as_str().to_string(),
            });
        }
        let from = (self.provider_id.clone(), self.model.clone());
        self.model = entry.id.clone();
        let to = (self.provider_id.clone(), self.model.clone());
        if let Some(session) = session {
            self.record_model_switch(session, from, to).await?;
        }
        Ok(())
    }

    /// 会话中途切换 provider（可选同时切模型）：重建 adapter，后续轮生效。
    pub async fn switch_provider(
        &mut self,
        session: Option<&SessionId>,
        provider: &str,
        model: Option<&str>,
    ) -> Result<(), AppError> {
        let known = channels::is_first_party(provider)
            || self.config.providers.iter().any(|p| p.id == provider);
        if !known {
            return Err(AppError::UnknownProvider {
                id: provider.to_string(),
            });
        }
        let target = ProviderId::new(provider);
        let assembled = assemble_provider(&self.config, &target, &self.backend, true).await?;

        // 目标模型：显式参数 → 当前模型（若属于目标 provider）→ 目标 provider
        // 的第一个 registry 条目；都无则要求显式 /model。
        let target_model = if let Some(model) = model {
            let entry = assembled
                .registry
                .resolve(model)
                .cloned()
                .ok_or_else(|| AppError::UnknownModel {
                    model: model.to_string(),
                    provider: provider.to_string(),
                })?;
            if entry.provider != target {
                return Err(AppError::ModelBelongsToProvider {
                    model: model.to_string(),
                    owner: entry.provider.as_str().to_string(),
                    current: provider.to_string(),
                });
            }
            entry.id
        } else if self
            .registry
            .resolve(self.model.as_str())
            .is_some_and(|entry| entry.provider == target)
        {
            self.model.clone()
        } else {
            assembled
                .registry
                .list()
                .into_iter()
                .find(|entry| entry.provider == target)
                .map(|entry| entry.id.clone())
                .ok_or_else(|| AppError::UnknownModel {
                    model: "<any>".to_string(),
                    provider: provider.to_string(),
                })?
        };

        let from = (self.provider_id.clone(), self.model.clone());
        self.provider = assembled.adapter;
        self.credential = assembled.credential;
        self.adapter_protocol = assembled.protocol;
        self.registry = Arc::new(assembled.registry);
        self.provider_id = target;
        self.model = target_model;
        let to = (self.provider_id.clone(), self.model.clone());
        if let Some(session) = session {
            self.record_model_switch(session, from, to).await?;
        }
        Ok(())
    }

    /// 追加 model.switched 诊断事件（冻结的 Diagnostic 变体，不新增枚举形状）。
    async fn record_model_switch(
        &self,
        session: &SessionId,
        from: (ProviderId, ModelId),
        to: (ProviderId, ModelId),
    ) -> Result<(), AppError> {
        let mut sequence = self.next_sequence(session).await?;
        let run_id = RunId::from(format!(
            "run-switch-{}",
            pawork_engine::now_timestamp().as_unix_millis()
        ));
        self.append_payload(
            session,
            &run_id,
            &mut sequence,
            AgentEvent::Diagnostic {
                code: "model.switched".into(),
                details: serde_json::json!({
                    "from": {
                        "provider": from.0.as_str(),
                        "model": from.1.as_str(),
                    },
                    "to": {
                        "provider": to.0.as_str(),
                        "model": to.1.as_str(),
                    },
                }),
            },
        )
        .await
    }

    pub fn tool_names(&self) -> Vec<&str> {
        self.tool_defs
            .iter()
            .map(|tool| tool.name.as_str())
            .collect()
    }

    pub async fn create_session(&self, title: impl Into<String>) -> Result<SessionId, AppError> {
        let n = self.next_session.fetch_add(1, Ordering::Relaxed);
        let ts = pawork_engine::now_timestamp();
        let id = SessionId::from(format!("ses-{}-{n}", ts.as_unix_millis()));
        self.store()?
            .create_session(&id, title, ts)
            .await?;
        Ok(id)
    }

    /// GUI SessionCreate：落盘会话并绑定 command 里的 workspace_id。
    pub async fn create_session_with_workspace(
        &self,
        title: impl Into<String>,
        workspace_id: WorkspaceId,
    ) -> Result<SessionId, AppError> {
        let id = self.create_session(title).await?;
        self.bind_session_workspace(&id, workspace_id);
        Ok(id)
    }

    pub async fn list_sessions(&self) -> Result<Vec<SessionRecord>, AppError> {
        Ok(self.store()?.list_sessions().await?)
    }

    pub async fn get_session(&self, session_id: &SessionId) -> Result<SessionRecord, AppError> {
        Ok(self.store()?.get_session(session_id).await?)
    }

    pub async fn resume_messages(&self, session_id: &SessionId) -> Result<Vec<Message>, AppError> {
        let _ = self.get_session(session_id).await?;
        self.seal_orphaned_approvals(session_id).await?;
        Ok(self.store()?.projection_snapshot(session_id).await?.messages)
    }

    /// 把中途被杀、仍停在 `waiting_for_approval` 的调用以 Denied 收口，避免 resume 后重跑。
    async fn seal_orphaned_approvals(&self, session_id: &SessionId) -> Result<(), AppError> {
        let pending: Vec<_> = self
            .store()?
            .projection_snapshot(session_id)
            .await?
            .tool_calls
            .into_iter()
            .filter(|call| call.state == "waiting_for_approval")
            .collect();
        if pending.is_empty() {
            return Ok(());
        }
        let mut sequence = self.next_sequence(session_id).await?;
        for call in pending {
            self.append_payload(
                session_id,
                &call.run_id,
                &mut sequence,
                AgentEvent::ToolApprovalResponded {
                    tool_call_id: call.tool_call_id.clone(),
                    decision: ApprovalDecision::Denied,
                    comment: Some("pending approval closed on resume".into()),
                },
            )
            .await?;
            if call.result.is_some() {
                continue;
            }
            let result = ToolResultContent {
                tool_call_id: call.tool_call_id.clone(),
                tool_name: Some(call.name.clone()),
                content: vec![ContentPart::Text(TextContent {
                    text: "pending approval closed on resume".into(),
                })],
                is_error: true,
                metadata: serde_json::Value::Null,
            };
            self.append_payload(
                session_id,
                &call.run_id,
                &mut sequence,
                AgentEvent::ToolExecutionCompleted {
                    tool_call_id: call.tool_call_id.clone(),
                    result: result.clone(),
                },
            )
            .await?;
            let n = self.next_message.fetch_add(1, Ordering::Relaxed);
            let message = Message {
                id: MessageId::from(format!(
                    "msg-{}-{n}",
                    pawork_engine::now_timestamp().as_unix_millis()
                )),
                role: MessageRole::Tool,
                content: vec![ContentPart::ToolResult(result)],
                metadata: Default::default(),
            };
            self.append_payload(
                session_id,
                &call.run_id,
                &mut sequence,
                AgentEvent::MessageCommitted { message },
            )
            .await?;
        }
        Ok(())
    }

    pub(crate) async fn append_payload(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
        sequence: &mut u64,
        payload: AgentEvent,
    ) -> Result<(), AppError> {
        let value = *sequence;
        *sequence = sequence
            .checked_add(1)
            .ok_or_else(|| AppError::Engine(EngineError::sink("sequence overflow")))?;
        let envelope = AgentEventEnvelope::new(
            EventId::from(format!("evt-resume-{}-{value}", run_id.as_str())),
            session_id.clone(),
            run_id.clone(),
            EventSequence::new(value),
            pawork_engine::now_timestamp(),
            payload,
        );
        self.store()?
            .append_event(DEFAULT_BRANCH_ID, envelope)
            .await?;
        Ok(())
    }

    pub async fn next_sequence(&self, session_id: &SessionId) -> Result<u64, AppError> {
        let tail = self.store()?.tail_events(session_id, 1).await?;
        Ok(match tail.last() {
            Some(event) => event
                .sequence
                .value()
                .checked_add(1)
                .ok_or_else(|| AppError::Engine(EngineError::sink("sequence overflow")))?,
            None => 1,
        })
    }

    /// `latest`、完整 id，或唯一前缀。多命中 fail-closed。
    pub async fn resolve_session(&self, spec: &str) -> Result<SessionId, AppError> {
        let spec = spec.trim();
        if spec.is_empty() {
            return Err(AppError::SessionNotFound(spec.into()));
        }
        if spec == "latest" {
            return self
                .list_sessions()
                .await?
                .into_iter()
                .next()
                .map(|record| SessionId::from(record.session_id))
                .ok_or_else(|| AppError::SessionNotFound("latest".into()));
        }
        let exact = SessionId::from(spec);
        if self.store()?.get_session(&exact).await.is_ok() {
            return Ok(exact);
        }
        let matches: Vec<String> = self
            .list_sessions()
            .await?
            .into_iter()
            .map(|record| record.session_id)
            .filter(|id| id.starts_with(spec))
            .collect();
        match matches.as_slice() {
            [only] => Ok(SessionId::from(only.as_str())),
            [] => Err(AppError::SessionNotFound(spec.into())),
            many => Err(AppError::AmbiguousSession {
                prefix: spec.into(),
                matches: many.join(", "),
            }),
        }
    }

    /// 事件化单轮：persist-first 双写。`messages` 最后一条必须是本轮 user。
    ///
    /// 调用方传入的 user `message_id` 会在落库前换成全局唯一 id：V1 schema 里
    /// `messages.message_id` 是跨 session 主键，CLI 进程内从 `msg-1` 起号会撞号。
    pub async fn chat_turn(
        &self,
        session_id: &SessionId,
        messages: Vec<Message>,
        render: &dyn AgentEventSink,
        cancel: CancellationToken,
    ) -> Result<ModelResponseSummary, AppError> {
        let run_n = self.next_run.fetch_add(1, Ordering::Relaxed);
        let run_id = RunId::from(format!(
            "run-{}-{run_n}",
            pawork_engine::now_timestamp().as_unix_millis()
        ));
        self.chat_turn_with_run_id(run_id, session_id, messages, render, cancel)
            .await
    }

    /// 以调用方提供的 run_id 执行一轮（GUI 需要在启动前登记取消令牌并
    /// 向客户端回报 run_id，因此 run id 的分配权上移到宿主）。
    pub async fn chat_turn_with_run_id(
        &self,
        run_id: RunId,
        session_id: &SessionId,
        mut messages: Vec<Message>,
        render: &dyn AgentEventSink,
        cancel: CancellationToken,
    ) -> Result<ModelResponseSummary, AppError> {
        let n = self.next_request.fetch_add(1, Ordering::Relaxed);
        let trigger = messages.last_mut().ok_or(AppError::EmptyTurn)?;
        if trigger.role != MessageRole::User {
            return Err(AppError::EmptyTurn);
        }
        // trigger 与 assistant/tool 消息共用 next_message 命名空间；
        // 若误用 next_request，两个计数器同从 1 起且同毫秒时会产生相同
        // message_id（messages.message_id 全局主键 → UNIQUE 冲突）。
        let message_n = self.next_message.fetch_add(1, Ordering::Relaxed);
        trigger.id = MessageId::from(format!(
            "msg-{}-{message_n}",
            pawork_engine::now_timestamp().as_unix_millis()
        ));
        let trigger = trigger.clone();
        let mut request_messages = messages;
        if let Some(note) = crate::diff::git_status_note(&self.workspace_roots).await {
            request_messages.insert(
                0,
                Message {
                    id: MessageId::from(format!(
                        "msg-git-{}",
                        pawork_engine::now_timestamp().as_unix_millis()
                    )),
                    role: MessageRole::System,
                    content: vec![ContentPart::Text(TextContent { text: note })],
                    metadata: Default::default(),
                },
            );
        }
        let request = assemble_request_with_tools(
            RequestId::from(format!("req-{n}")),
            self.model.clone(),
            request_messages,
            self.tool_defs.clone(),
        );
        let start_sequence = self.next_sequence(session_id).await?;
        let turn = SessionTurn::new(
            session_id.clone(),
            run_id.clone(),
            self.provider_id.clone(),
            self.model.clone(),
            start_sequence,
            trigger,
        );
        let sink = PersistThenRender {
            store: self.store()?,
            render,
        };
        let mut turn_context = self.turn_context();
        turn_context.injected_layers = self.load_injected_layers();
        let loop_ctx = SessionLoopCtx {
            scheduler: self.scheduler.clone(),
            workspace_id: self.workspace_id.clone(),
            run_id,
            next_message: &self.next_message,
            next_request: &self.next_request,
            policy: PolicyEngine::new(self.approval_mode),
            approval_mode: self.approval_mode,
            workspace_trusted: self.workspace_trusted,
            descriptors: self.descriptors.clone(),
            approval_host: self.approval_host.clone(),
            store: Some(self.store()?),
            session_id: Some(session_id.clone()),
            token_estimator: Some(self.session_estimator.clone()),
            checkpoints: self.checkpoints.clone(),
            workspace_roots: self.workspace_roots.clone(),
        };
        Ok(run_session(
            self.provider.as_ref(),
            request,
            turn,
            &sink,
            cancel,
            &loop_ctx,
            DEFAULT_MAX_TOOL_ROUNDS,
            turn_context,
        )
        .await?)
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

    /// 模型目录（builtin + config 覆盖 + 运行期 /models 探测合并，探测失败退回静态）。
    pub async fn model_catalog(&self) -> Vec<CatalogEntry> {
        let mut catalog = self.registry.as_ref().clone();
        match catalog
            .probe_provider(self.provider.as_ref(), self.credential.as_ref())
            .await
        {
            Err(error) => {
                tracing::warn!(
                    provider = %self.provider_id,
                    error = %error,
                    "runtime model probe failed; falling back to static catalog"
                );
            }
            Ok(probe) => {
            for definition in &probe.definitions {
                if catalog.resolve(definition.id.as_str()).is_none() {
                    catalog.extend_with(vec![CatalogEntry {
                        id: definition.id.clone(),
                        provider: self.provider_id.clone(),
                        display_name: definition.display_name.clone(),
                        context_window_tokens: definition.context_window_tokens,
                        max_output_tokens: definition.max_output_tokens,
                        capabilities: definition.capabilities.clone(),
                        pricing: None,
                        aliases: Vec::new(),
                    }]);
                }
            }
            }
        }
        catalog.list().into_iter().cloned().collect()
    }

    /// pawork models 聚合目录：六通道静态条目 + config providers（Messages
    /// 静态目录与 models 覆盖）+ 所有能装配成功的通道的运行期探测（探测失败
    /// 静默退回该通道静态，与单通道目录一致）。未登记协议或无凭证的通道跳过探测。
    pub async fn models_overview(&self) -> Vec<CatalogEntry> {
        let mut provider_ids: Vec<ProviderId> = channels::FIRST_PARTY_CHANNELS
            .iter()
            .map(|channel| ProviderId::new(channel.id))
            .collect();
        for provider in &self.config.providers {
            let id = ProviderId::new(provider.id.as_str());
            if !provider_ids.contains(&id) {
                provider_ids.push(id);
            }
        }

        let mut catalog = ModelRegistry::empty();
        for id in &provider_ids {
            let channel = channels::first_party_channel(id.as_str());
            let protocol = match channel_protocol(channel, &self.config, id.as_str()) {
                Ok(protocol) => protocol,
                Err(_) => continue,
            };
            let registry = assemble_registry(&self.config, id, protocol, channel);
            for entry in registry.list() {
                if catalog.resolve(entry.id.as_str()).is_none() {
                    catalog.extend_with(vec![entry.clone()]);
                }
            }
        }
        let mut probe_jobs = Vec::new();
        for id in provider_ids {
            let assembled = if id.as_str() == self.provider_id.as_str() && !self.provider_pending {
                Some((Arc::clone(&self.provider), self.credential.clone()))
            } else {
                match assemble_provider(&self.config, &id, &self.backend, false).await {
                    Ok(assembled) => Some((assembled.adapter, assembled.credential)),
                    Err(_) => None,
                }
            };
            if let Some((adapter, credential)) = assembled {
                probe_jobs.push((id, adapter, credential));
            }
        }
        let catalog_for_probe = catalog.clone();
        let probe_results = futures::future::join_all(probe_jobs.into_iter().map(
            |(id, adapter, credential)| {
                let catalog = catalog_for_probe.clone();
                async move {
                    let result = catalog
                        .probe_provider(adapter.as_ref(), credential.as_ref())
                        .await;
                    (id, result)
                }
            },
        ))
        .await;
        for (id, result) in probe_results {
            match result {
                Err(error) => {
                    tracing::warn!(
                        provider = %id,
                        error = %error,
                        "runtime model probe failed; falling back to static catalog"
                    );
                }
                Ok(probe) => {
                    for definition in &probe.definitions {
                        if catalog.resolve(definition.id.as_str()).is_none() {
                            catalog.extend_with(vec![CatalogEntry {
                                id: definition.id.clone(),
                                provider: id.clone(),
                                display_name: definition.display_name.clone(),
                                context_window_tokens: definition.context_window_tokens,
                                max_output_tokens: definition.max_output_tokens,
                                capabilities: definition.capabilities.clone(),
                                pricing: None,
                                aliases: Vec::new(),
                            }]);
                        }
                    }
                }
            }
        }
        catalog.list().into_iter().cloned().collect()
    }

    /// 会话累计用量：对已完成 run 的 RunCompleted.usage 求和（投影可重建）。
    pub async fn session_usage(&self, session_id: &SessionId) -> Result<TokenUsage, AppError> {
        Ok(self.session_usage_inner(session_id).await?.0)
    }

    /// 最近一次完成 run 的用量（CLI 每轮尾部行）。
    pub async fn last_run_usage(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<TokenUsage>, AppError> {
        Ok(self.session_usage_inner(session_id).await?.1)
    }

    async fn session_usage_inner(
        &self,
        session_id: &SessionId,
    ) -> Result<(TokenUsage, Option<TokenUsage>), AppError> {
        let runs = self
            .store()?
            .projection_snapshot(session_id)
            .await?
            .runs;
        let mut total = TokenUsage::default();
        let mut last = None;
        for run in runs
            .iter()
            .filter(|run| run.state == "completed")
            .rev()
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
        {
            if let Some(usage) = run
                .data
                // run_json 存的是 Adjacently-tagged AgentEvent：
                // {"type":"run_completed","data":{"stop_reason":...,"usage":...}}。
                .get("data")
                .and_then(|inner| inner.get("usage"))
                .and_then(|value| serde_json::from_value::<TokenUsage>(value.clone()).ok())
            {
                // 按时间正序遍历，持续覆盖：最终拿到的是最新 completed run
                // 的 usage（get_or_insert 会冻结在最早一轮，REPL 每轮用量行
                // 因此显示过期数据，S5 波 C 冒烟实测发现）。
                last = Some(usage.clone());
                total.input_tokens = total.input_tokens.saturating_add(usage.input_tokens);
                total.output_tokens = total.output_tokens.saturating_add(usage.output_tokens);
                total.cache_read_tokens = total
                    .cache_read_tokens
                    .saturating_add(usage.cache_read_tokens);
                total.cache_write_tokens = total
                    .cache_write_tokens
                    .saturating_add(usage.cache_write_tokens);
            }
        }
        Ok((total, last))
    }

    /// 按 registry 定价估算费用；无定价条目返回 None（不编造）。
    pub fn estimate_cost_for(&self, model: &ModelId, usage: &TokenUsage) -> Option<Cost> {
        let entry = self.registry.resolve(model.as_str())?;
        let pricing = entry.pricing.as_ref()?;
        Some(pawork_provider_core::estimate_cost(usage, pricing))
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
        let messages = self.resume_messages(session_id).await?;
        let trigger = messages
            .last()
            .cloned()
            .ok_or(AppError::EmptyTurn)?;
        let n = self.next_request.fetch_add(1, Ordering::Relaxed);
        let request = assemble_request(
            RequestId::from(format!("req-compact-{n}")),
            self.model.clone(),
            messages,
        );
        let run_n = self.next_run.fetch_add(1, Ordering::Relaxed);
        let run_id = RunId::from(format!(
            "compact-{}-{run_n}",
            pawork_engine::now_timestamp().as_unix_millis()
        ));
        let turn = SessionTurn::new(
            session_id.clone(),
            run_id.clone(),
            self.provider_id.clone(),
            self.model.clone(),
            self.next_sequence(session_id).await?,
            trigger,
        );
        let sink = PersistThenRender {
            store: self.store()?,
            render,
        };
        let loop_ctx = SessionLoopCtx {
            scheduler: self.scheduler.clone(),
            workspace_id: self.workspace_id.clone(),
            run_id,
            next_message: &self.next_message,
            next_request: &self.next_request,
            policy: PolicyEngine::new(self.approval_mode),
            approval_mode: self.approval_mode,
            workspace_trusted: self.workspace_trusted,
            descriptors: self.descriptors.clone(),
            approval_host: self.approval_host.clone(),
            store: Some(self.store()?),
            session_id: Some(session_id.clone()),
            token_estimator: Some(self.session_estimator.clone()),
            checkpoints: self.checkpoints.clone(),
            workspace_roots: self.workspace_roots.clone(),
        };
        Ok(run_manual_compaction(
            self.provider.as_ref(),
            request,
            turn,
            &sink,
            cancel,
            &loop_ctx,
            self.turn_context(),
        )
        .await?)
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

fn find_provider<'a>(
    providers: &'a [ProviderConfig],
    id: &str,
) -> Result<&'a ProviderConfig, AppError> {
    providers
        .iter()
        .find(|provider| provider.id == id)
        .ok_or_else(|| AppError::UnknownProvider { id: id.to_string() })
}

/// 通道协议解析（无凭证依赖）：首发通道固定，其余走 config provider_protocols。
fn channel_protocol(
    channel: Option<&channels::FirstPartyChannel>,
    config: &PaworkConfig,
    id: &str,
) -> Result<AdapterProtocol, AppError> {
    match channel.map(|channel| channel.kind.clone()) {
        Some(ChannelKind::ChatGptOAuth) | Some(ChannelKind::XaiOAuth) => {
            Ok(AdapterProtocol::Responses)
        }
        Some(ChannelKind::ApiKey) => Ok(AdapterProtocol::ChatCompletions),
        None => Ok(resolve_adapter_protocol(config, id)?),
    }
}

/// 目录装配（无凭证依赖）：builtin + 协议静态目录 + config 覆盖 + transport。
fn assemble_registry(
    config: &PaworkConfig,
    provider_id: &ProviderId,
    protocol: AdapterProtocol,
    channel: Option<&channels::FirstPartyChannel>,
) -> ModelRegistry {
    let mut registry = ModelRegistry::builtin();
    if protocol == AdapterProtocol::Messages {
        registry.merge_provider_models(provider_id, &pawork_providers::builtin_models());
    }
    if channel.is_some_and(|channel| channel.kind == ChannelKind::XaiOAuth) {
        registry.merge_provider_models(provider_id, &pawork_providers::xai_builtin_models());
    }
    apply_config_models(&mut registry, &config.models, provider_id);
    apply_transport_overrides(&mut registry, config);
    registry
}

/// 装配产物：adapter + 凭证 + 协议标记 + 全量 registry。
struct AssembledProvider {
    adapter: Arc<dyn ModelProvider>,
    credential: Option<ResolvedCredential>,
    protocol: AdapterProtocol,
    registry: ModelRegistry,
}

/// 统一装配入口（S6 波 C）：首发通道走通道表，其余走 config + 协议解析。
///
/// 这是 host 装配层唯一的 Provider 选择点；Engine 仍只看 trait 对象。
/// `refresh_oauth = true` 时 OAuth 凭证先走请求前刷新（网络）。
async fn assemble_provider(
    config: &PaworkConfig,
    provider_id: &ProviderId,
    backend: &Arc<dyn SecretBackend>,
    refresh_oauth: bool,
) -> Result<AssembledProvider, AppError> {
    let id = provider_id.as_str();
    let channel = channels::first_party_channel(id);
    let config_base = find_provider(&config.providers, id)
        .ok()
        .and_then(|provider| provider.base_url.clone());

    let (adapter, credential, protocol) = match channel.map(|channel| channel.kind.clone()) {
        Some(ChannelKind::ChatGptOAuth) => {
            let (credential, account_id) =
                oauth_credential(config, id, backend, refresh_oauth).await?;
            let account_id = account_id.ok_or_else(|| {
                AppError::OAuthLogin(
                    "ChatGPT account id missing; re-run pawork auth login chatgpt".into(),
                )
            })?;
            let base_url =
                config_base.unwrap_or_else(|| channel.expect("channel").default_base_url.into());
            let mut chatgpt_config =
                pawork_providers::ChatGptConfig::new(account_id).with_base_url(base_url);
            chatgpt_config.http.proxy = config.proxy_url.clone();
            let provider =
                pawork_providers::ChatGptProvider::new(chatgpt_config, Some(credential.clone()))?;
            (
                Arc::new(provider) as Arc<dyn ModelProvider>,
                Some(credential),
                AdapterProtocol::Responses,
            )
        }
        Some(ChannelKind::XaiOAuth) => {
            let (credential, _) = oauth_credential(config, id, backend, refresh_oauth).await?;
            let base_url =
                config_base.unwrap_or_else(|| channel.expect("channel").default_base_url.into());
            let mut xai_config = pawork_providers::XaiConfig::new(base_url);
            xai_config.http.proxy = config.proxy_url.clone();
            let provider =
                pawork_providers::XaiProvider::new(xai_config, Some(credential.clone()))?;
            (
                Arc::new(provider) as Arc<dyn ModelProvider>,
                Some(credential),
                AdapterProtocol::Responses,
            )
        }
        Some(ChannelKind::ApiKey) => {
            let channel_enum = channels::api_key_channel(id)
                .ok_or_else(|| AppError::UnknownProvider { id: id.to_string() })?;
            let (credential, _source) = resolve_api_key_credential(backend, id)?;
            let mut channel_config = ApiKeyChannelConfig::new(channel_enum);
            channel_config.http.proxy = config.proxy_url.clone();
            if let Some(base_url) = config_base {
                channel_config = channel_config.with_base_url(base_url);
            }
            for (model, transport) in model_transport_overrides(config) {
                channel_config = channel_config.with_model_transport(model, transport);
            }
            let provider =
                ApiKeyChannelProvider::new(channel_config, Some(credential.clone()))?;
            (
                Arc::new(provider) as Arc<dyn ModelProvider>,
                Some(credential),
                AdapterProtocol::ChatCompletions,
            )
        }
        // 非首发通道：config 必须提供 base_url，协议来自 provider_protocols。
        None => {
            // provider 必须已在 config 登记且提供 base_url（fail-closed）。
            let _provider = find_provider(&config.providers, id)?;
            let base_url = config_base.ok_or_else(|| AppError::MissingBaseUrl {
                id: id.to_string(),
            })?;
            let (credential, _source) = resolve_api_key_credential(backend, id)?;
            let protocol = resolve_adapter_protocol(config, id)?;
            let adapter: Arc<dyn ModelProvider> = match protocol {
                AdapterProtocol::ChatCompletions => Arc::new(OpenAiCompatibleProvider::new(
                    {
                        let mut c = OpenAiCompatibleConfig::new(base_url)
                            .with_provider_id(provider_id.as_str().to_string());
                        c.http.proxy = config.proxy_url.clone();
                        c
                    },
                    Some(credential.clone()),
                )?),
                AdapterProtocol::Messages => Arc::new(AnthropicProvider::new(
                    {
                        let mut c = AnthropicConfig::new(base_url)
                            .with_provider_id(provider_id.as_str().to_string());
                        c.http.proxy = config.proxy_url.clone();
                        c
                    },
                    Some(credential.clone()),
                )?),
                AdapterProtocol::Responses => {
                    return Err(AppError::Protocol(ProtocolError::Unknown {
                        provider: id.to_string(),
                        value: "responses".to_string(),
                    }))
                }
            };
            (adapter, Some(credential), protocol)
        }
    };

    // registry 装配与 CatalogOnly 路径共享（builtin + 静态目录 + config 覆盖）。
    let registry = assemble_registry(config, provider_id, protocol, channel);

    Ok(AssembledProvider {
        adapter,
        credential,
        protocol,
        registry,
    })
}

/// API key 凭证链：auth 文件 → env fallback → fail-closed。
fn resolve_api_key_credential(
    backend: &Arc<dyn SecretBackend>,
    id: &str,
) -> Result<(ResolvedCredential, AuthSource), AppError> {
    match resolve_provider_credential(backend.as_ref(), id) {
        CredentialSource::Keychain(stored) => {
            let credential = ApiKeyCredential::from_stored(stored)?
                .resolve(backend.as_ref())?;
            Ok((credential, AuthSource::File))
        }
        CredentialSource::EnvFallback(credential) => Ok((credential, AuthSource::Env)),
        CredentialSource::None => Err(AppError::MissingCredential {
            provider: id.to_string(),
            env_name: api_key_env_name(id),
        }),
    }
}

/// OAuth 凭证解析：default 条目（meta）→（可选）请求前刷新 → bearer。
async fn oauth_credential(
    config: &PaworkConfig,
    id: &str,
    backend: &Arc<dyn SecretBackend>,
    refresh: bool,
) -> Result<(ResolvedCredential, Option<String>), AppError> {
    let provider = ProviderId::new(id);
    let Some(mut stored) = load_default_oauth_credential(backend.as_ref(), &provider)? else {
        return Err(AppError::OAuthLoginRequired(id.to_string()));
    };
    if refresh {
        let preset = oauth_refresh_endpoint(config, id)?;
        let http = AppCore::http_from_config(config)?;
        let refresh_config = OAuthRefreshConfig {
            token_url: preset.token_url,
            client_id: preset.client_id,
            refresh_skew: Duration::from_secs(30),
        };
        match refresh_default_oauth_credential_if_needed(
            &mut stored,
            backend.as_ref(),
            &refresh_config,
            &http,
        )
        .await
        {
            Ok(_) => {}
            Err(AuthError::TokenEndpoint { error, .. }) if error == "invalid_grant" => {
                return Err(AppError::OAuthLogin(format!(
                    "provider {id} 的 OAuth refresh token 已失效；请运行 pawork auth login {id} 重新登录"
                )))
            }
            Err(error) => return Err(AppError::Auth(error)),
        }
    }
    let account_id =
        load_default_oauth_meta(backend.as_ref(), &provider)?.and_then(|meta| meta.account_id);
    let credential = resolve_oauth_credential(&stored, backend.as_ref())?;
    Ok((credential, account_id))
}

/// OAuth 刷新端点：config [oauth.<id>] 覆盖 → 通道预设（xAI 无预设则报错）。
fn oauth_refresh_endpoint(
    config: &PaworkConfig,
    id: &str,
) -> Result<channels::OAuthPreset, AppError> {
    if let Some(preset) = channels::oauth_override(config, id) {
        return Ok(preset);
    }
    channels::first_party_channel(id)
        .and_then(|channel| channel.oauth_preset())
        .ok_or_else(|| {
            AppError::OAuthLogin(format!(
                "provider {id} has no OAuth endpoint preset; configure [oauth.{id}] first"
            ))
        })
}

/// extra["model_transports"]：{"model-id": "responses"|"chat_completions"|"messages"}。
fn model_transport_overrides(
    config: &PaworkConfig,
) -> Vec<(String, pawork_api::ModelTransport)> {
    let Some(table) = config.extra.get("model_transports") else {
        return Vec::new();
    };
    let Some(map) = table.as_object() else {
        return Vec::new();
    };
    map.iter()
        .filter_map(|(model, value)| {
            let transport = match value.as_str()? {
                "responses" => pawork_api::ModelTransport::Responses,
                "chat_completions" | "openai-compatible" => {
                    pawork_api::ModelTransport::ChatCompletions
                }
                "messages" | "anthropic-messages" => pawork_api::ModelTransport::Messages,
                _ => return None,
            };
            Some((model.clone(), transport))
        })
        .collect()
}

/// 把 transport 覆盖应用到 registry 条目（混合协议模型显式声明，不按渠道猜）。
fn apply_transport_overrides(registry: &mut ModelRegistry, config: &PaworkConfig) {
    for (model, transport) in model_transport_overrides(config) {
        if let Some(mut entry) = registry.resolve(&model).cloned() {
            entry.capabilities.transport = transport;
            registry.extend_with(vec![entry]);
        }
    }
}

/// 把 config `[[models]]` 覆盖并入 registry：已有条目只改 window / max_output
/// （能力、定价、别名保持目录权威），未知条目追加（provider 归当前 provider，
/// 能力 fail-closed 全 false，定价 None——不编造）。
fn apply_config_models(
    registry: &mut ModelRegistry,
    models: &[pawork_config::ModelConfig],
    provider_id: &ProviderId,
) {
    for config in models {
        let mut entry = match registry.resolve(&config.id) {
            Some(existing) => existing.clone(),
            None => CatalogEntry {
                id: pawork_domain::ModelId::new(&config.id),
                provider: provider_id.clone(),
                display_name: config.id.clone(),
                context_window_tokens: 0,
                max_output_tokens: 0,
                capabilities: Default::default(),
                pricing: None,
                aliases: Vec::new(),
            },
        };
        if let Some(window) = config.context_window {
            entry.context_window_tokens = window;
        }
        if let Some(max_output) = config.max_output {
            entry.max_output_tokens = max_output;
        }
        registry.extend_with(vec![entry]);
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
    use std::sync::Mutex;

    use async_trait::async_trait;
    use pawork_api::{
        CanonicalModelRequest, ModelCapabilities, ProviderStreamEvent,
    };
    use pawork_domain::{
        AgentEvent, AgentEventEnvelope, ContentPart, MessageId, MessageRole, StopReason,
        TextContent, TokenUsage,
    };
    use pawork_engine::EngineError;
    use wiremock::matchers::{body_string_contains, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;

    #[derive(Default)]
    struct RecordingEvents(Mutex<Vec<AgentEventEnvelope>>);

    impl RecordingEvents {
        fn types(&self) -> Vec<&'static str> {
            self.0
                .lock()
                .expect("mutex")
                .iter()
                .map(|envelope| match &envelope.payload {
                    AgentEvent::MessageCommitted { message }
                        if message.role == MessageRole::User =>
                    {
                        "user"
                    }
                    AgentEvent::MessageCommitted { .. } => "assistant",
                    AgentEvent::RunStarted { .. } => "RunStarted",
                    AgentEvent::RunCompleted { .. } => "RunCompleted",
                    AgentEvent::AssistantTextDelta { .. } => "delta",
                    AgentEvent::ToolCallStarted { .. } => "ToolCallStarted",
                    AgentEvent::ToolApprovalRequested { .. } => "ToolApprovalRequested",
                    AgentEvent::ToolApprovalResponded { .. } => "ToolApprovalResponded",
                    AgentEvent::ToolExecutionStarted { .. } => "ToolExecutionStarted",
                    AgentEvent::ToolExecutionCompleted { .. } => "ToolExecutionCompleted",
                    AgentEvent::ToolOutputDelta { .. } => "ToolOutputDelta",
                    AgentEvent::CompactionStarted { .. } => "CompactionStarted",
                    AgentEvent::CompactionCompleted { .. } => "CompactionCompleted",
                    AgentEvent::CheckpointCreated { .. } => "CheckpointCreated",
                    AgentEvent::CheckpointRolledBack { .. } => "CheckpointRolledBack",
                    _ => "other",
                })
                .collect()
        }
    }

    #[async_trait]
    impl AgentEventSink for RecordingEvents {
        async fn emit(&self, envelope: AgentEventEnvelope) -> Result<(), EngineError> {
            self.0.lock().expect("mutex").push(envelope);
            Ok(())
        }
    }

    struct ScriptedProvider {
        events: Vec<ProviderStreamEvent>,
        summary: ModelResponseSummary,
        models: Vec<ModelDefinition>,
    }

    #[async_trait]
    impl ModelProvider for ScriptedProvider {
        fn id(&self) -> ProviderId {
            ProviderId::from("mock")
        }

        async fn list_models(
            &self,
            _credential: Option<&ResolvedCredential>,
        ) -> Result<Vec<ModelDefinition>, ProviderError> {
            Ok(self.models.clone())
        }

        async fn stream(
            &self,
            _request: CanonicalModelRequest,
            sink: &dyn pawork_api::ProviderEventSink,
            _cancel: CancellationToken,
        ) -> Result<ModelResponseSummary, ProviderError> {
            for event in &self.events {
                sink.emit(event.clone()).await?;
            }
            Ok(self.summary.clone())
        }
    }

    fn sample_config(id: &str) -> PaworkConfig {
        PaworkConfig {
            default_provider: Some(id.into()),
            default_model: Some("glm-5.2".into()),
            providers: vec![ProviderConfig {
                id: id.into(),
                base_url: Some("https://example.test/v1".into()),
                ..ProviderConfig::default()
            }],
            ..PaworkConfig::default()
        }
    }

    fn set_env(key: &str, value: &str) {
        #[allow(unused_unsafe)]
        unsafe {
            std::env::set_var(key, value);
        }
    }

    fn remove_env(key: &str) {
        #[allow(unused_unsafe)]
        unsafe {
            std::env::remove_var(key);
        }
    }

    fn user_hello() -> Message {
        Message {
            id: MessageId::from("message-1"),
            role: MessageRole::User,
            content: vec![ContentPart::Text(TextContent {
                text: "hello".into(),
            })],
            metadata: Default::default(),
        }
    }

    async fn mock_core(events: Vec<ProviderStreamEvent>) -> (AppCore, tempfile::TempDir) {
        mock_core_with_usage(events, TokenUsage::default()).await
    }

    async fn mock_core_with_usage(
        events: Vec<ProviderStreamEvent>,
        usage: TokenUsage,
    ) -> (AppCore, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("session.db");
        let (store, _) = SessionStore::open(&path).await.expect("store");
        let summary = ModelResponseSummary {
            stop_reason: StopReason::Completed,
            usage,
            response_id: Some("resp-1".into()),
            provider_metadata: Default::default(),
        };
        let core = AppCore::from_parts(
            Arc::new(ScriptedProvider {
                events,
                summary,
                models: vec![ModelDefinition {
                    id: ModelId::from("glm-5.2"),
                    display_name: "glm-5.2".into(),
                    context_window_tokens: 0,
                    max_output_tokens: 0,
                    capabilities: ModelCapabilities::default(),
                }],
            }),
            None,
            ModelId::from("glm-5.2"),
            ProviderId::from("mock"),
            Some(store),
        );
        (core, dir)
    }

    fn core_with_registry(registry: ModelRegistry, model: &str) -> AppCore {
        AppCore::from_parts_with_protocol(
            Arc::new(ScriptedProvider {
                events: Vec::new(),
                summary: ModelResponseSummary {
                    stop_reason: StopReason::Completed,
                    usage: TokenUsage::default(),
                    response_id: Some("resp-1".into()),
                    provider_metadata: Default::default(),
                },
                models: Vec::new(),
            }),
            None,
            ModelId::from(model),
            ProviderId::from("mock"),
            AdapterProtocol::ChatCompletions,
            None,
            registry,
        )
    }

    #[test]
    fn from_resolved_requires_provider_and_model() {
        let err = AppCore::from_resolved(PaworkConfig::default(), None, None)
            .expect_err("empty config");
        assert!(matches!(err, AppError::MissingDefaultProvider));

        let err = AppCore::from_resolved(
            PaworkConfig {
                default_provider: Some("missing".into()),
                default_model: Some("m".into()),
                ..PaworkConfig::default()
            },
            None,
            None,
        )
        .expect_err("unknown provider");
        assert!(matches!(err, AppError::UnknownProvider { id } if id == "missing"));
    }

    #[tokio::test]
    async fn catalog_load_allows_zero_config_for_auth_and_models() {
        let core = AppCore::from_config_inner(
            PaworkConfig::default(),
            None,
            None,
            Arc::new(pawork_auth::MemoryBackend::new()),
            true,
        )
        .await
        .expect("catalog load tolerates missing defaults");
        assert_eq!(core.provider_id.as_str(), "catalog");
        // auth list 在零配置下可列出六通道（全部 none 来源）。
        let rows = core.auth_status();
        assert!(rows.iter().any(|row| row.provider == "xai"));
    }

    #[tokio::test]
    async fn chat_load_still_fails_closed_without_defaults() {
        let err = AppCore::from_config_inner(
            PaworkConfig::default(),
            None,
            None,
            Arc::new(pawork_auth::MemoryBackend::new()),
            false,
        )
        .await
        .expect_err("chat load must fail");
        assert!(matches!(err, AppError::MissingDefaultProvider));
    }

    #[test]
    fn from_resolved_fail_closed_without_env_key() {
        let id = "app-core-missing-key";
        remove_env(&api_key_env_name(id));
        let err = AppCore::from_resolved(sample_config(id), None, None).expect_err("no key");
        let display = format!("{err}");
        match err {
            AppError::MissingCredential { provider, env_name } => {
                assert_eq!(provider, id);
                assert_eq!(env_name, "PAWORK_API_KEY_APP_CORE_MISSING_KEY");
                assert!(display.contains("PAWORK_API_KEY_APP_CORE_MISSING_KEY"));
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[tokio::test]
    async fn models_overview_aggregates_six_channels() {
        let (core, _dir) = mock_core(Vec::new()).await;
        let overview = core.models_overview().await;
        let providers: std::collections::BTreeSet<String> = overview
            .iter()
            .map(|entry| entry.provider.as_str().to_string())
            .collect();
        // chatgpt 无静态目录（Codex backend 模型只能登录后运行期探测）。
        for expected in ["xai", "glm-coding", "opencode-go", "qwen-token-plan", "deepseek"] {
            assert!(
                providers.contains(expected),
                "missing provider {expected} in overview: {providers:?}"
            );
        }
        assert!(overview.iter().any(|entry| entry.id.as_str() == "grok-4"),
            "xai static models missing");
    }

    #[tokio::test]
    async fn switch_model_records_diagnostic_event() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (store, _) = SessionStore::open(&dir.path().join("session.db"))
            .await
            .expect("store");
        let mut registry = ModelRegistry::empty();
        for id in ["m-a", "m-b"] {
            registry.extend_with(vec![CatalogEntry {
                id: ModelId::from(id),
                provider: ProviderId::from("mock"),
                display_name: id.into(),
                context_window_tokens: 8_000,
                max_output_tokens: 1_024,
                capabilities: Default::default(),
                pricing: None,
                aliases: Vec::new(),
            }]);
        }
        let mut core = AppCore::from_parts_with_protocol(
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
            ModelId::from("m-a"),
            ProviderId::from("mock"),
            AdapterProtocol::ChatCompletions,
            Some(store),
            registry,
        );
        let session = core.create_session("switch").await.expect("session");
        core.switch_model(Some(&session), "m-b")
            .await
            .expect("switch");
        let events = core
            .store()
            .expect("store")
            .replay_events(&session, 0, 100)
            .await
            .expect("replay");
        let switches: Vec<_> = events
            .iter()
            .filter(|envelope| matches!(
                &envelope.payload,
                AgentEvent::Diagnostic { code, .. } if code == "model.switched"
            ))
            .collect();
        assert_eq!(switches.len(), 1, "model.switched event missing");
        match &switches[0].payload {
            AgentEvent::Diagnostic { details, .. } => {
                assert_eq!(details["from"]["model"], "m-a");
                assert_eq!(details["to"]["model"], "m-b");
            }
            other => panic!("unexpected {other:?}"),
        }
    }

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

    #[tokio::test]
    async fn default_oauth_refresh_is_singleflight_and_persists_meta() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/token"))
            .and(body_string_contains("refresh_token=old-refresh"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_delay(Duration::from_millis(100))
                    .set_body_json(serde_json::json!({
                        "access_token": "singleflight-access",
                        "refresh_token": "singleflight-refresh",
                        "token_type": "Bearer",
                        "expires_in": 3600,
                        "scope": "openid profile"
                    })),
            )
            .expect(1)
            .mount(&server)
            .await;

        let backend: Arc<dyn SecretBackend> = Arc::new(pawork_auth::MemoryBackend::new());
        let provider = ProviderId::new("xai");
        pawork_auth::store_default_oauth_token(
            backend.as_ref(),
            provider.clone(),
            &pawork_auth::TokenSet {
                access_token: "old-access".into(),
                refresh_token: Some("old-refresh".into()),
                id_token: None,
                expires_in: Some(0),
                token_type: "Bearer".into(),
                scope: Some("openid".into()),
            },
        )
        .expect("store default oauth");

        let mut config = PaworkConfig::default();
        config.extra.insert(
            "oauth".into(),
            serde_json::json!({
                "xai": {
                    "client_id": "client-id",
                    "device_auth_url": "https://example.test/device/code",
                    "token_url": format!("{}/token", server.uri()),
                    "scopes": ["openid", "profile"]
                }
            }),
        );

        let (first, second) = tokio::join!(
            oauth_credential(&config, "xai", &backend, true),
            oauth_credential(&config, "xai", &backend, true)
        );
        for result in [first, second] {
            let (credential, account_id) = result.expect("oauth credential");
            assert_eq!(credential.expose_secret(), "singleflight-access");
            assert!(account_id.is_none());
        }

        let stored = load_default_oauth_credential(backend.as_ref(), &provider)
            .expect("load default oauth")
            .expect("default oauth present");
        assert_eq!(
            pawork_auth::read_refresh_token(&stored, backend.as_ref()).expect("rotated refresh"),
            "singleflight-refresh"
        );
        let meta = load_default_oauth_meta(backend.as_ref(), &provider)
            .expect("load meta")
            .expect("meta present");
        assert_eq!(meta.masked, stored.masked);
        assert_eq!(
            meta.expires_at_ms,
            stored.expires_at.map(|value| value.as_unix_millis())
        );
        assert_eq!(meta.scopes, vec!["openid", "profile"]);
        server.verify().await;
    }

    #[tokio::test]
    async fn permanent_oauth_refresh_failure_requires_relogin_without_secret_leak() {
        let server = MockServer::start().await;
        let endpoint_description = "rotated credential is permanently invalid: secret-sentinel";
        Mock::given(method("POST"))
            .and(path("/token"))
            .and(body_string_contains("refresh_token=invalid-old-refresh"))
            .respond_with(ResponseTemplate::new(400).set_body_json(serde_json::json!({
                "error": "invalid_grant",
                "error_description": endpoint_description
            })))
            .expect(1)
            .mount(&server)
            .await;

        let backend: Arc<dyn SecretBackend> = Arc::new(pawork_auth::MemoryBackend::new());
        let provider = ProviderId::new("xai");
        let stored = pawork_auth::store_default_oauth_token(
            backend.as_ref(),
            provider.clone(),
            &pawork_auth::TokenSet {
                access_token: "invalid-old-access".into(),
                refresh_token: Some("invalid-old-refresh".into()),
                id_token: None,
                expires_in: Some(0),
                token_type: "Bearer".into(),
                scope: Some("openid".into()),
            },
        )
        .expect("store default oauth");

        let mut config = PaworkConfig::default();
        config.extra.insert(
            "oauth".into(),
            serde_json::json!({
                "xai": {
                    "client_id": "client-id",
                    "device_auth_url": "https://example.test/device/code",
                    "token_url": format!("{}/token", server.uri()),
                    "scopes": ["openid"]
                }
            }),
        );

        let error = oauth_credential(&config, "xai", &backend, true)
            .await
            .expect_err("invalid_grant must fail closed");
        let message = error.to_string();
        assert!(message.contains("pawork auth login xai"));
        assert!(!message.contains(endpoint_description));
        assert!(!message.contains("secret-sentinel"));
        assert!(!message.contains("invalid-old-refresh"));
        assert_eq!(
            backend
                .get(&stored.keychain_service, &stored.keychain_account)
                .expect("access remains unchanged"),
            "invalid-old-access"
        );
        assert_eq!(
            pawork_auth::read_refresh_token(&stored, backend.as_ref())
                .expect("refresh remains unchanged"),
            "invalid-old-refresh"
        );
        server.verify().await;
    }

    #[tokio::test]
    async fn catalog_load_tolerates_missing_credential() {
        // 独立 provider id：避免与并行 env 测试共享同一环境变量。
        let id = "deepseek";
        remove_env(&api_key_env_name(id));
        let mut config = sample_config(id);
        config.default_model = Some("glm-5.2".into());
        let backend: Arc<dyn SecretBackend> = Arc::new(pawork_auth::MemoryBackend::new());
        let strict = AppCore::from_config(config.clone(), None, None, backend.clone()).await;
        assert!(matches!(
            strict,
            Err(AppError::MissingCredential { provider, .. }) if provider == id
        ));
        let core = AppCore::from_config_inner(config, None, None, backend, true)
            .await
            .expect("catalog load");
        assert!(core.provider_pending(), "core should be pending");
        let overview = core.models_overview().await;
        assert!(overview.iter().any(|entry| entry.id.as_str() == "glm-5.2"));
    }

    #[test]
    fn cli_overrides_win_and_secret_is_redacted() {
        let id = "app-core-redact";
        let env_name = api_key_env_name(id);
        let secret = "super-secret-key-value-not-for-logs";
        set_env(&env_name, secret);
        let core = AppCore::from_resolved(sample_config(id), Some(id), Some("deepseek-v4-pro"))
            .expect("load with key");
        remove_env(&env_name);

        assert_eq!(core.provider_id().as_str(), id);
        assert_eq!(core.model().as_str(), "deepseek-v4-pro");
        assert_eq!(core.adapter_protocol(), AdapterProtocol::ChatCompletions);
        let debug = format!("{core:?}");
        assert!(
            !debug.contains(secret),
            "secret leaked in Debug: {debug}"
        );
        assert!(debug.contains("[REDACTED]"), "{debug}");
    }

    #[tokio::test]
    async fn chat_turn_persists_and_projects_for_resume() {
        let (core, _dir) = mock_core(vec![
            ProviderStreamEvent::TextDelta("hi".into()),
            ProviderStreamEvent::ThinkingDelta("think".into()),
            ProviderStreamEvent::ResponseCompleted(StopReason::Completed),
        ])
        .await;
        let session = core.create_session("hello").await.expect("create");
        let sink = RecordingEvents::default();
        core.chat_turn(
            &session,
            vec![user_hello()],
            &sink,
            CancellationToken::new(),
        )
        .await
        .expect("turn");
        assert!(sink.types().contains(&"user"));
        assert!(sink.types().contains(&"assistant"));
        assert!(sink.types().contains(&"RunCompleted"));
        assert!(!sink.types().contains(&"RunFailed"));

        let messages = core.resume_messages(&session).await.expect("resume");
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, MessageRole::User);
        assert_eq!(messages[1].role, MessageRole::Assistant);

        let listed = core.list_sessions().await.expect("list");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].session_id, session.as_str());
        assert_eq!(
            core.resolve_session("latest").await.expect("latest").as_str(),
            session.as_str()
        );

        let models = core.list_models().await.expect("models");
        assert_eq!(models[0].id.as_str(), "glm-5.2");
        core.shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    async fn two_sessions_do_not_collide_on_caller_message_ids() {
        let (core, _dir) = mock_core(vec![
            ProviderStreamEvent::TextDelta("ok".into()),
            ProviderStreamEvent::ResponseCompleted(StopReason::Completed),
        ])
        .await;
        let first = core.create_session("one").await.expect("first");
        let second = core.create_session("two").await.expect("second");
        let sink = RecordingEvents::default();
        core.chat_turn(
            &first,
            vec![user_hello()],
            &sink,
            CancellationToken::new(),
        )
        .await
        .expect("first turn");
        core.chat_turn(
            &second,
            vec![user_hello()],
            &sink,
            CancellationToken::new(),
        )
        .await
        .expect("second turn");

        let first_messages = core.resume_messages(&first).await.expect("resume first");
        let second_messages = core.resume_messages(&second).await.expect("resume second");
        assert_eq!(first_messages.len(), 2);
        assert_eq!(second_messages.len(), 2);
        assert_ne!(
            first_messages[0].id, second_messages[0].id,
            "user message_id is a global primary key"
        );
        core.shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    async fn unknown_resume_is_fail_closed() {
        let (core, _dir) = mock_core(Vec::new()).await;
        let err = core
            .resolve_session("missing-session")
            .await
            .expect_err("missing");
        assert!(matches!(err, AppError::SessionNotFound(_)));
        core.shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    async fn secret_in_message_metadata_is_redacted_from_db() {
        let secret = "fake-api-key-that-must-not-reach-sqlite";
        let (core, dir) = mock_core(vec![
            ProviderStreamEvent::TextDelta("ok".into()),
            ProviderStreamEvent::ResponseCompleted(StopReason::Completed),
        ])
        .await;
        let session = core.create_session("secret-test").await.expect("create");
        let mut user = user_hello();
        user.metadata
            .provider_metadata
            .insert("api_key".into(), serde_json::json!(secret));
        core.chat_turn(&session, vec![user], &RecordingEvents::default(), CancellationToken::new())
            .await
            .expect("turn");

        let path = core.store().expect("store").path().to_path_buf();
        let bytes = std::fs::read(&path).expect("read db");
        let haystack = String::from_utf8_lossy(&bytes);
        assert!(
            !haystack.contains(secret),
            "secret leaked into session.db"
        );
        let replayed = core
            .store()
            .expect("store")
            .replay_events(&session, 1, 64)
            .await
            .expect("replay");
        let json = serde_json::to_string(&replayed).expect("json");
        assert!(!json.contains(secret), "secret leaked into replay json");
        assert!(json.contains("[REDACTED]"));
        core.shutdown().await.expect("shutdown");
        drop(dir);
    }

    #[test]
    fn from_resolved_selects_messages_adapter_from_default_table() {
        let id = "glm-coding-anthropic";
        let env_name = api_key_env_name(id);
        set_env(&env_name, "not-a-real-key");
        let core = AppCore::from_resolved(sample_config(id), None, None).expect("load");
        remove_env(&env_name);
        assert_eq!(core.adapter_protocol(), AdapterProtocol::Messages);
        assert_eq!(core.provider_id().as_str(), id);
    }

    #[test]
    fn extra_protocol_overrides_default_and_rejects_unknown() {
        let id = "app-core-protocol-extra";
        let env_name = api_key_env_name(id);
        set_env(&env_name, "not-a-real-key");
        let mut config = sample_config(id);
        config.extra.insert(
            "provider_protocols".into(),
            serde_json::json!({ id: "messages" }),
        );
        let core = AppCore::from_resolved(config.clone(), None, None).expect("override");
        assert_eq!(core.adapter_protocol(), AdapterProtocol::Messages);

        config.extra.insert(
            "provider_protocols".into(),
            serde_json::json!({ id: "not-a-protocol" }),
        );
        let err = AppCore::from_resolved(config, None, None).expect_err("bad protocol");
        remove_env(&env_name);
        assert!(matches!(err, AppError::Protocol(_)));
    }

    #[tokio::test]
    async fn attach_workspace_registers_eight_tools() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (mut core, _store_dir) = mock_core(Vec::new()).await;
        core.attach_workspace(dir.path()).expect("attach");
        let mut names = core.tool_names();
        names.sort();
        assert_eq!(
            names,
            vec![
                "apply_patch",
                "edit_file",
                "find_files",
                "list_directory",
                "read_file",
                "run_command",
                "search_text",
                "write_file",
            ]
        );
    }

    #[tokio::test]
    async fn chat_turn_executes_read_file_via_scheduler() {
        use pawork_testkit::{MockProvider, MockScript};

        let workspace = tempfile::tempdir().expect("workspace");
        std::fs::write(workspace.path().join("hello.txt"), "hello-from-workspace")
            .expect("write fixture");
        let dir = tempfile::tempdir().expect("store");
        let path = dir.path().join("session.db");
        let (store, _) = SessionStore::open(&path).await.expect("store");
        let provider = MockProvider::sequence(vec![
            MockScript::new()
                .tool_call("read_file", serde_json::json!({"path": "hello.txt"}))
                .complete_with(StopReason::ToolUse),
            MockScript::new()
                .text("the file says hello-from-workspace")
                .complete(),
        ]);
        let mut core = AppCore::from_parts(
            Arc::new(provider),
            None,
            ModelId::from("model-1"),
            ProviderId::from("mock"),
            Some(store),
        );
        core.attach_workspace(workspace.path()).expect("attach");
        let session = core.create_session("tools").await.expect("create");
        let sink = RecordingEvents::default();
        core.chat_turn(
            &session,
            vec![user_hello()],
            &sink,
            CancellationToken::new(),
        )
        .await
        .expect("tool loop");

        let types = sink.types();
        assert!(types.contains(&"ToolCallStarted"));
        assert!(types.contains(&"ToolExecutionStarted"));
        assert!(types.contains(&"ToolExecutionCompleted"));
        assert!(types.contains(&"RunCompleted"));
        let messages = core.resume_messages(&session).await.expect("resume");
        assert!(messages.iter().any(|message| message.role == MessageRole::Tool));
        let joined: String = messages
            .iter()
            .flat_map(|message| message.content.iter())
            .filter_map(|part| match part {
                ContentPart::Text(text) => Some(text.text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            joined.contains("hello-from-workspace"),
            "expected tool output or assistant recap, got {joined}"
        );
        core.shutdown().await.expect("shutdown");
    }

    #[test]
    fn turn_context_derives_from_registry_with_config_override() {
        let mut registry = ModelRegistry::builtin();
        apply_config_models(
            &mut registry,
            &[pawork_config::ModelConfig {
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
            &[pawork_config::ModelConfig {
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

    #[test]
    fn estimate_cost_uses_registry_pricing_and_hides_unpriced() {
        let core = core_with_registry(ModelRegistry::builtin(), "glm-5.2");
        let usage = TokenUsage {
            input_tokens: 1_000_000,
            output_tokens: 1_000_000,
            ..TokenUsage::default()
        };
        let cost = core
            .estimate_cost_for(&ModelId::from("deepseek-v4-pro"), &usage)
            .expect("deepseek-v4-pro is priced");
        assert_eq!(cost.currency, "USD");
        assert_eq!(cost.amount_micros, 435_000 + 870_000);
        // 订阅制无公开费率、未知条目：不编造费用。
        assert!(core
            .estimate_cost_for(&ModelId::from("glm-5.2"), &usage)
            .is_none());
        assert!(core
            .estimate_cost_for(&ModelId::from("mystery"), &usage)
            .is_none());
    }

    #[tokio::test]
    async fn session_usage_accumulates_completed_runs() {
        let usage = TokenUsage {
            input_tokens: 120,
            output_tokens: 45,
            cache_read_tokens: 10,
            cache_write_tokens: 5,
        };
        let (core, _dir) = mock_core_with_usage(
            vec![
                ProviderStreamEvent::TextDelta("ok".into()),
                ProviderStreamEvent::ResponseCompleted(StopReason::Completed),
            ],
            usage.clone(),
        )
        .await;
        let session = core.create_session("usage").await.expect("create");
        let sink = RecordingEvents::default();
        core.chat_turn(
            &session,
            vec![user_hello()],
            &sink,
            CancellationToken::new(),
        )
        .await
        .expect("turn 1");
        core.chat_turn(
            &session,
            vec![user_hello()],
            &sink,
            CancellationToken::new(),
        )
        .await
        .expect("turn 2");

        let total = core.session_usage(&session).await.expect("total");
        assert_eq!(total.input_tokens, 240);
        assert_eq!(total.output_tokens, 90);
        assert_eq!(total.cache_read_tokens, 20);
        assert_eq!(total.cache_write_tokens, 10);
        let last = core
            .last_run_usage(&session)
            .await
            .expect("last")
            .expect("at least one completed run");
        assert_eq!(last, usage);
        core.shutdown().await.expect("shutdown");
    }

    /// 回归（S5 波 C 冒烟发现）：每轮用量行必须取「最新 completed run」的
    /// usage，而不是最早一轮——按次递变 usage 验证 last_run_usage 跟随第 2 轮。
    #[tokio::test]
    async fn last_run_usage_returns_latest_completed_run() {
        struct SteppedUsageProvider {
            usages: Vec<TokenUsage>,
            calls: std::sync::atomic::AtomicUsize,
        }

        #[async_trait]
        impl ModelProvider for SteppedUsageProvider {
            fn id(&self) -> ProviderId {
                ProviderId::from("mock")
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
                sink: &dyn pawork_api::ProviderEventSink,
                _cancel: CancellationToken,
            ) -> Result<ModelResponseSummary, ProviderError> {
                let index = self
                    .calls
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let usage = self.usages[index.min(self.usages.len() - 1)].clone();
                sink.emit(ProviderStreamEvent::TextDelta("ok".into()))
                    .await?;
                Ok(ModelResponseSummary {
                    stop_reason: StopReason::Completed,
                    usage,
                    response_id: Some("resp-stepped".into()),
                    provider_metadata: Default::default(),
                })
            }
        }

        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("session.db");
        let (store, _) = SessionStore::open(&path).await.expect("store");
        let core = AppCore::from_parts(
            Arc::new(SteppedUsageProvider {
                usages: vec![
                    TokenUsage {
                        input_tokens: 100,
                        output_tokens: 10,
                        cache_read_tokens: 0,
                        cache_write_tokens: 0,
                    },
                    TokenUsage {
                        input_tokens: 222,
                        output_tokens: 22,
                        cache_read_tokens: 4,
                        cache_write_tokens: 0,
                    },
                ],
                calls: std::sync::atomic::AtomicUsize::new(0),
            }),
            None,
            ModelId::from("glm-5.2"),
            ProviderId::from("mock"),
            Some(store),
        );
        let session = core.create_session("stepped").await.expect("create");
        let sink = RecordingEvents::default();
        core.chat_turn(&session, vec![user_hello()], &sink, CancellationToken::new())
            .await
            .expect("turn 1");
        core.chat_turn(&session, vec![user_hello()], &sink, CancellationToken::new())
            .await
            .expect("turn 2");

        let last = core
            .last_run_usage(&session)
            .await
            .expect("last")
            .expect("at least one completed run");
        assert_eq!(last.input_tokens, 222);
        assert_eq!(last.output_tokens, 22);
        assert_eq!(last.cache_read_tokens, 4);
        let total = core.session_usage(&session).await.expect("total");
        assert_eq!(total.input_tokens, 322);
        core.shutdown().await.expect("shutdown");
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

    struct ScriptedHost {
        queue: Mutex<Vec<ApprovalDecision>>,
        asked: AtomicU64,
    }

    impl ScriptedHost {
        fn new(queue: Vec<ApprovalDecision>) -> Arc<Self> {
            Arc::new(Self {
                queue: Mutex::new(queue),
                asked: AtomicU64::new(0),
            })
        }
    }

    #[async_trait]
    impl ApprovalPromptHost for ScriptedHost {
        async fn decide(&self, _ask: &ApprovalAsk, _cancel: CancellationToken) -> ApprovalDecision {
            self.asked.fetch_add(1, Ordering::SeqCst);
            self.queue.lock().expect("queue").remove(0)
        }
    }

    struct PanicHost;

    #[async_trait]
    impl ApprovalPromptHost for PanicHost {
        async fn decide(&self, ask: &ApprovalAsk, _cancel: CancellationToken) -> ApprovalDecision {
            panic!("approval host should not be asked for {}", ask.tool_name);
        }
    }

    async fn write_ready_core(
        mode: ApprovalMode,
        trusted: bool,
        host: Arc<dyn ApprovalPromptHost>,
        workspace: &Path,
    ) -> (AppCore, tempfile::TempDir) {
        use pawork_testkit::{MockProvider, MockScript};

        let dir = tempfile::tempdir().expect("store");
        let path = dir.path().join("session.db");
        let (store, _) = SessionStore::open(&path).await.expect("store");
        let provider = MockProvider::sequence(vec![
            MockScript::new()
                .tool_call(
                    "write_file",
                    serde_json::json!({"path": "notes.txt", "content": "hello-write"}),
                )
                .complete_with(StopReason::ToolUse),
            MockScript::new().text("wrote notes").complete(),
        ]);
        let mut core = AppCore::from_parts(
            Arc::new(provider),
            None,
            ModelId::from("model-1"),
            ProviderId::from("mock"),
            Some(store),
        );
        core.open_checkpoints(dir.path().join("artifacts"))
            .await
            .expect("artifacts");
        core.configure_approval(mode, trusted, host);
        core.attach_workspace(workspace).expect("attach");
        (core, dir)
    }

    #[tokio::test]
    async fn ask_for_writes_approved_once_persists_file_and_event_pair() {
        let workspace = tempfile::tempdir().expect("workspace");
        let host = ScriptedHost::new(vec![ApprovalDecision::ApprovedOnce]);
        let (core, _dir) =
            write_ready_core(ApprovalMode::AskForWrites, true, host.clone(), workspace.path())
                .await;
        let session = core.create_session("write").await.expect("create");
        let sink = RecordingEvents::default();
        core.chat_turn(
            &session,
            vec![user_hello()],
            &sink,
            CancellationToken::new(),
        )
        .await
        .expect("turn");

        let types = sink.types();
        let requested = types
            .iter()
            .position(|name| *name == "ToolApprovalRequested")
            .expect("requested");
        let responded = types
            .iter()
            .position(|name| *name == "ToolApprovalResponded")
            .expect("responded");
        let started = types
            .iter()
            .position(|name| *name == "ToolExecutionStarted")
            .expect("started");
        assert!(requested < responded);
        assert!(responded < started);
        assert_eq!(host.asked.load(Ordering::SeqCst), 1);
        let written = std::fs::read_to_string(workspace.path().join("notes.txt")).expect("file");
        assert_eq!(written, "hello-write");
        assert!(types.contains(&"CheckpointCreated"));
        let created = types
            .iter()
            .position(|name| *name == "CheckpointCreated")
            .expect("created");
        assert!(responded < created);
        assert!(created < started);
        core.shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    async fn write_checkpoint_rollback_restores_and_appends() {
        let workspace = tempfile::tempdir().expect("workspace");
        let host = ScriptedHost::new(vec![ApprovalDecision::ApprovedOnce]);
        let (core, _dir) =
            write_ready_core(ApprovalMode::AskForWrites, true, host, workspace.path()).await;
        let session = core.create_session("rollback").await.expect("create");
        let sink = RecordingEvents::default();
        core.chat_turn(
            &session,
            vec![user_hello()],
            &sink,
            CancellationToken::new(),
        )
        .await
        .expect("turn");
        assert_eq!(
            std::fs::read_to_string(workspace.path().join("notes.txt")).expect("file"),
            "hello-write"
        );

        let listed = core.list_checkpoints(&session).await.expect("list");
        assert!(!listed.is_empty());
        let spec = listed
            .iter()
            .find(|item| item.tool_call_id.is_none())
            .or_else(|| listed.first())
            .expect("checkpoint")
            .checkpoint_id
            .clone();
        let before_rollback = core
            .store()
            .expect("store")
            .replay_events(&session, 1, usize::MAX)
            .await
            .expect("replay");
        let last_seq = before_rollback.last().expect("tail").sequence.value();

        let outcome = core.rollback(&session, &spec).await.expect("rollback");
        assert!(!workspace.path().join("notes.txt").exists());
        assert!(outcome.restored.iter().any(|path| path == "notes.txt"));

        let after = core
            .store()
            .expect("store")
            .replay_events(&session, 1, usize::MAX)
            .await
            .expect("replay after");
        assert_eq!(after.last().expect("last").sequence.value(), last_seq + 1);
        assert!(matches!(
            after.last().expect("last").payload,
            AgentEvent::CheckpointRolledBack { .. }
        ));
        let diff = core.session_diff(&session).await.expect("diff");
        assert!(diff.files.is_empty());
        core.shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    async fn deny_all_emits_approval_pair_and_does_not_write() {
        let workspace = tempfile::tempdir().expect("workspace");
        let (core, _dir) = write_ready_core(
            ApprovalMode::AskForWrites,
            true,
            Arc::new(DenyAllApprovals),
            workspace.path(),
        )
        .await;
        let session = core.create_session("deny").await.expect("create");
        let sink = RecordingEvents::default();
        core.chat_turn(
            &session,
            vec![user_hello()],
            &sink,
            CancellationToken::new(),
        )
        .await
        .expect("turn");

        let types = sink.types();
        assert!(types.contains(&"ToolApprovalRequested"));
        assert!(types.contains(&"ToolApprovalResponded"));
        assert!(!types.contains(&"ToolExecutionStarted"));
        assert!(!workspace.path().join("notes.txt").exists());
        core.shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    async fn read_only_trusted_write_is_denied_without_asking() {
        let workspace = tempfile::tempdir().expect("workspace");
        let (core, _dir) = write_ready_core(
            ApprovalMode::ReadOnly,
            true,
            Arc::new(PanicHost),
            workspace.path(),
        )
        .await;
        let session = core.create_session("readonly").await.expect("create");
        let sink = RecordingEvents::default();
        core.chat_turn(
            &session,
            vec![user_hello()],
            &sink,
            CancellationToken::new(),
        )
        .await
        .expect("turn");

        assert!(!sink.types().contains(&"ToolApprovalRequested"));
        assert!(!workspace.path().join("notes.txt").exists());
        core.shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    async fn untrusted_never_ask_denies_write_without_asking() {
        let workspace = tempfile::tempdir().expect("workspace");
        let (core, _dir) = write_ready_core(
            ApprovalMode::NeverAsk,
            false,
            Arc::new(PanicHost),
            workspace.path(),
        )
        .await;
        let session = core.create_session("untrusted").await.expect("create");
        let sink = RecordingEvents::default();
        core.chat_turn(
            &session,
            vec![user_hello()],
            &sink,
            CancellationToken::new(),
        )
        .await
        .expect("turn");

        assert!(!sink.types().contains(&"ToolApprovalRequested"));
        assert!(!workspace.path().join("notes.txt").exists());
        core.shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    async fn resume_seals_orphaned_approval_as_denied() {
        let (core, _dir) = mock_core(Vec::new()).await;
        let session = core.create_session("orphan").await.expect("create");
        let tool_call_id = pawork_domain::ToolCallId::from("call-orphan");
        let run_id = RunId::from("run-orphan");
        let ts = pawork_engine::now_timestamp();
        core.store()
            .expect("store")
            .append_event(
                DEFAULT_BRANCH_ID,
                AgentEventEnvelope::new(
                    EventId::from("evt-1"),
                    session.clone(),
                    run_id.clone(),
                    EventSequence::new(1),
                    ts,
                    AgentEvent::ToolCallStarted {
                        tool_call_id: tool_call_id.clone(),
                        name: "write_file".into(),
                    },
                ),
            )
            .await
            .expect("started");
        core.store()
            .expect("store")
            .append_event(
                DEFAULT_BRANCH_ID,
                AgentEventEnvelope::new(
                    EventId::from("evt-2"),
                    session.clone(),
                    run_id,
                    EventSequence::new(2),
                    ts,
                    AgentEvent::ToolApprovalRequested {
                        tool_call_id: tool_call_id.clone(),
                        reason: "needs approval".into(),
                    },
                ),
            )
            .await
            .expect("requested");

        let waiting = core
            .store()
            .expect("store")
            .projection_snapshot(&session)
            .await
            .expect("snap");
        assert_eq!(waiting.tool_calls[0].state, "waiting_for_approval");

        let messages = core.resume_messages(&session).await.expect("resume");
        assert!(messages.iter().any(|message| message.role == MessageRole::Tool));

        let sealed = core
            .store()
            .expect("store")
            .projection_snapshot(&session)
            .await
            .expect("sealed");
        assert_eq!(sealed.tool_calls[0].state, "completed");
        assert!(sealed.tool_calls[0].result.is_some());

        let replayed = core
            .store()
            .expect("store")
            .replay_events(&session, 1, 64)
            .await
            .expect("replay");
        let responded = replayed.iter().find_map(|envelope| match &envelope.payload {
            AgentEvent::ToolApprovalResponded {
                decision, comment, ..
            } => Some((decision.clone(), comment.clone())),
            _ => None,
        });
        assert_eq!(
            responded,
            Some((
                ApprovalDecision::Denied,
                Some("pending approval closed on resume".into())
            ))
        );

        let again = core.resume_messages(&session).await.expect("idempotent");
        assert_eq!(again.len(), messages.len());
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
    async fn resource_loader_injects_root_agents_md() {
        let workspace = tempfile::tempdir().expect("workspace");
        std::fs::write(
            workspace.path().join("AGENTS.md"),
            "所有回答以『收到』开头\n",
        )
        .expect("agents");
        let (mut core, _store) = mock_core(Vec::new()).await;
        core.attach_workspace(workspace.path()).expect("attach");
        let layers = core.load_injected_layers();
        assert!(
            layers.iter().any(|layer| {
                layer.kind == "root_agents_file" && layer.content.contains("收到")
            }),
            "{layers:?}"
        );
    }

    #[tokio::test]
    async fn expand_at_refs_adds_separate_content_part() {
        let workspace = tempfile::tempdir().expect("workspace");
        std::fs::write(workspace.path().join("ROADMAP.md"), "phase S9 wiring\n")
            .expect("roadmap");
        let (mut core, _store) = mock_core(Vec::new()).await;
        core.attach_workspace(workspace.path()).expect("attach");
        core.prime_extensions().await.expect("prime");
        let parts = core
            .expand_at_refs("请根据附件：@ROADMAP 回答")
            .expect("expand");
        assert_eq!(parts.len(), 2, "{parts:?}");
        match &parts[0] {
            ContentPart::Text(text) => assert_eq!(text.text, "请根据附件：@ROADMAP 回答"),
            other => panic!("expected user text, got {other:?}"),
        }
        match &parts[1] {
            ContentPart::Text(text) => {
                assert!(text.text.contains("ROADMAP.md"), "{text:?}");
                assert!(text.text.contains("phase S9 wiring"), "{text:?}");
            }
            other => panic!("expected attachment, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn compat_import_writes_pawork_files_and_keeps_source_mtime() {
        let workspace = tempfile::tempdir().expect("workspace");
        let home = tempfile::tempdir().expect("home");
        std::fs::write(workspace.path().join("CLAUDE.md"), "keep diffs small\n")
            .expect("claude md");
        let (mut core, _store) = mock_core(Vec::new()).await;
        core.attach_workspace(workspace.path()).expect("attach");
        let source = workspace.path().join("CLAUDE.md");
        let before = std::fs::metadata(&source).expect("meta").modified().ok();
        let before_bytes = std::fs::read(&source).expect("bytes");
        let report = core
            .apply_compat_import(pawork_compat::ExternalSource::Claude, Some(home.path()))
            .expect("import");
        assert!(report.sources_unchanged);
        assert_eq!(
            std::fs::metadata(&source).expect("meta").modified().ok(),
            before
        );
        assert_eq!(std::fs::read(&source).expect("bytes"), before_bytes);
        let imported = std::fs::read_to_string(workspace.path().join(".pawork/instructions.md"))
            .expect("instructions");
        assert!(imported.contains("keep diffs small"), "{imported}");
    }

    #[tokio::test]
    async fn session_export_v3_round_trip() {
        let (core, _store) = mock_core(Vec::new()).await;
        let session = core.create_session("export-me").await.expect("create");
        let (_, export) = core
            .export_session_doc(Some(session.as_str()))
            .await
            .expect("export");
        assert_eq!(export.schema_version, pawork_session::EXPORT_SCHEMA_VERSION);
        let dir = tempfile::tempdir().expect("import store");
        let path = dir.path().join("session.db");
        let (store, _) = SessionStore::open(&path).await.expect("store");
        store
            .import_session(&export, &export.tenant_id, &export.principal_id)
            .await
            .expect("import");
        let imported = store.get_session(&session).await.expect("imported session");
        assert_eq!(imported.title, "export-me");
        store.shutdown().await.expect("shutdown import store");
        core.shutdown().await.expect("shutdown");
    }
}
