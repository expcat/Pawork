//! 应用门面：读配置 → env key → provider → 读写工具 + run_command → 事件化 `run_session`。
//!
//! 不按 Provider 名称分支；协议来自 `extra.provider_protocols` 与默认表。
//! 落库 persist-first，再推渲染 sink。

mod approval;
mod data_dir;
mod loop_ctx;
mod persist;
mod protocol;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use pawork_api::{
    CredentialKind, ModelDefinition, ModelProvider, ModelResponseSummary, ProviderError,
    ResolvedCredential, ToolDefinition,
};
use pawork_config::{
    api_key_env_name, read_api_key_from_env, ConfigError, Loader, PaworkConfig, ProviderConfig,
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
    AnthropicConfig, AnthropicProvider, OpenAiCompatibleConfig, OpenAiCompatibleProvider,
};
use pawork_policy::PolicyEngine;
use pawork_provider_core::{CatalogEntry, ModelRegistry};
use pawork_session::{SessionStore, SessionStoreError, DEFAULT_BRANCH_ID};
use pawork_tools::{
    ApplyPatchTool, EditFileTool, FindFilesTool, ListDirectoryTool, ReadFileTool, RunCommandTool,
    SearchTextTool, ToolRegistry, ToolRegistryError, ToolScheduler, ToolSchedulerConfig,
    WriteFileTool,
};
use pawork_workspace::{WorkspaceError, WorkspaceService};
use thiserror::Error;

use crate::loop_ctx::SessionLoopCtx;
use crate::protocol::resolve_adapter_protocol;

pub use approval::{
    parse_approval_mode, ApprovalAsk, ApprovalPromptHost, DenyAllApprovals,
};
pub use data_dir::{default_data_dir, session_db_path};
pub use persist::PersistThenRender;
pub use protocol::{AdapterProtocol, ProtocolError};
pub use pawork_policy::{ApprovalMode, RiskLevel};
pub use pawork_session::SessionRecord;

/// 从配置文件与 CLI 覆盖构造 [`AppCore`] 的选项。
#[derive(Clone, Default)]
pub struct AppLoadOptions {
    pub workspace_root: Option<PathBuf>,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub data_dir: Option<PathBuf>,
    pub approval_mode: Option<ApprovalMode>,
    pub approval_host: Option<Arc<dyn ApprovalPromptHost>>,
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
    #[error("缺少 API key：请设置环境变量 {env_name}")]
    MissingCredential { env_name: String },
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
}

/// S5 压缩在 engine 侧保留的最近消息条数；session 侧保留策略按
/// `RETAINED_MESSAGES / 2` 轮对齐同一折叠边界。
pub(crate) const RETAINED_MESSAGES: usize = 4;

/// 已装配的 Core：协议中立 provider、读写工具、默认 model、可选 session store。
pub struct AppCore {
    provider: Arc<dyn ModelProvider>,
    credential: Option<ResolvedCredential>,
    model: ModelId,
    provider_id: ProviderId,
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
    tool_defs: Vec<ToolDefinition>,
    descriptors: Vec<ToolDescriptor>,
    approval_mode: ApprovalMode,
    workspace_trusted: bool,
    approval_host: Arc<dyn ApprovalPromptHost>,
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
        let resolved = Loader::discover(options.workspace_root.as_deref()).resolve()?;
        let workspace_root = options
            .workspace_root
            .clone()
            .or_else(|| std::env::current_dir().ok());
        let trusted = resolved.config.trust_workspaces.unwrap_or(false);
        let mut core = Self::from_resolved(
            resolved.config,
            options.provider.as_deref(),
            options.model.as_deref(),
        )?;
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
        let data_dir = options.data_dir.unwrap_or_else(default_data_dir);
        core.open_store(session_db_path(data_dir)).await?;
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
        let trusted = resolved.config.trust_workspaces.unwrap_or(false);
        let mut core = Self::from_resolved(resolved.config, provider, model)?;
        core.configure_approval(ApprovalMode::ReadOnly, trusted, Arc::new(DenyAllApprovals));
        if let Some(root) = workspace_root_from_config_file(workspace_file) {
            core.attach_workspace(&root)?;
        } else if let Ok(cwd) = std::env::current_dir() {
            core.attach_workspace(&cwd)?;
        }
        core.open_store(store_path).await?;
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
        let provider_cfg = find_provider(&config.providers, &provider_id)?;
        let base_url = provider_cfg
            .base_url
            .clone()
            .ok_or_else(|| AppError::MissingBaseUrl {
                id: provider_id.clone(),
            })?;

        let env_name = api_key_env_name(&provider_id);
        let secret = read_api_key_from_env(&provider_id).ok_or(AppError::MissingCredential {
            env_name,
        })?;
        let credential = ResolvedCredential::new(CredentialKind::ApiKey, secret);

        let protocol = resolve_adapter_protocol(&config, &provider_id)?;
        let adapter: Arc<dyn ModelProvider> = match protocol {
            AdapterProtocol::ChatCompletions => Arc::new(OpenAiCompatibleProvider::new(
                OpenAiCompatibleConfig::new(base_url).with_provider_id(provider_id.clone()),
                Some(credential.clone()),
            )?),
            AdapterProtocol::Messages => Arc::new(AnthropicProvider::new(
                AnthropicConfig::new(base_url).with_provider_id(provider_id.clone()),
                Some(credential.clone()),
            )?),
        };
        // registry 装配：builtin 目录 + adapter 静态目录（按协议选择，不做名称分支）
        // + config models 覆盖。运行期探测合并在 model_catalog() 内按需进行。
        let mut registry = ModelRegistry::builtin();
        if protocol == AdapterProtocol::Messages {
            let provider = ProviderId::from(provider_id.as_str());
            registry.merge_provider_models(&provider, &pawork_providers::builtin_models());
            apply_config_models(&mut registry, &config.models, &provider);
        } else {
            let provider = ProviderId::from(provider_id.as_str());
            apply_config_models(&mut registry, &config.models, &provider);
        }

        Ok(Self::from_parts_with_protocol(
            adapter,
            Some(credential),
            ModelId::from(model_id.as_str()),
            ProviderId::from(provider_id.as_str()),
            protocol,
            None,
            registry,
        ))
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
            credential,
            model,
            provider_id,
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
            tool_defs: Vec::new(),
            descriptors: Vec::new(),
            approval_mode: ApprovalMode::ReadOnly,
            workspace_trusted: false,
            approval_host: Arc::new(DenyAllApprovals),
            next_request: AtomicU64::new(1),
            next_run: AtomicU64::new(1),
            next_session: AtomicU64::new(1),
            next_message: AtomicU64::new(1),
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

        let mut registry = ToolRegistry::new();
        registry.extend([
            Arc::new(ReadFileTool::new(workspaces.clone())) as Arc<dyn pawork_api::AgentTool>,
            Arc::new(ListDirectoryTool::new(workspaces.clone())) as Arc<dyn pawork_api::AgentTool>,
            Arc::new(SearchTextTool::new(workspaces.clone())) as Arc<dyn pawork_api::AgentTool>,
            Arc::new(FindFilesTool::new(workspaces.clone())) as Arc<dyn pawork_api::AgentTool>,
            Arc::new(WriteFileTool::new(workspaces.clone())) as Arc<dyn pawork_api::AgentTool>,
            Arc::new(EditFileTool::new(workspaces.clone())) as Arc<dyn pawork_api::AgentTool>,
            Arc::new(ApplyPatchTool::new(workspaces.clone())) as Arc<dyn pawork_api::AgentTool>,
            Arc::new(RunCommandTool::new(workspaces)) as Arc<dyn pawork_api::AgentTool>,
        ])?;
        self.descriptors = registry.descriptors();
        self.tool_defs = self
            .descriptors
            .iter()
            .map(|descriptor| ToolDefinition {
                name: descriptor.name.clone(),
                description: descriptor.description.clone(),
                input_schema: descriptor.input_schema.clone(),
            })
            .collect();
        self.scheduler = Arc::new(ToolScheduler::new(
            registry,
            ToolSchedulerConfig {
                max_concurrent: 8,
                approval_mode: self.approval_mode,
                workspace_trusted: self.workspace_trusted,
            },
        ));
        self.workspace_id = workspace_id;
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

    async fn append_payload(
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
        let request = assemble_request_with_tools(
            RequestId::from(format!("req-{n}")),
            self.model.clone(),
            messages,
            self.tool_defs.clone(),
        );
        let run_n = self.next_run.fetch_add(1, Ordering::Relaxed);
        let start_sequence = self.next_sequence(session_id).await?;
        let run_id = RunId::from(format!(
            "run-{}-{run_n}",
            pawork_engine::now_timestamp().as_unix_millis()
        ));
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
        };
        Ok(run_session(
            self.provider.as_ref(),
            request,
            turn,
            &sink,
            cancel,
            &loop_ctx,
            DEFAULT_MAX_TOOL_ROUNDS,
            self.turn_context(),
        )
        .await?)
    }

    pub async fn list_models(&self) -> Result<Vec<ModelDefinition>, ProviderError> {
        self.provider.list_models(self.credential.as_ref()).await
    }

    /// 本会话模型的上下文配置：registry 解析 window / max_output 推导预算；
    /// 目录无条目或 window 为 0 时退回禁用（与 S5 前行为一致，不编造窗口）。
    pub fn turn_context(&self) -> TurnContext {
        let Some(entry) = self.registry.resolve(self.model.as_str()) else {
            return TurnContext::default();
        };
        if entry.context_window_tokens == 0 {
            return TurnContext::default();
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
        }
    }

    /// 模型目录（builtin + config 覆盖 + 运行期 /models 探测合并，探测失败退回静态）。
    pub async fn model_catalog(&self) -> Vec<CatalogEntry> {
        let mut catalog = self.registry.as_ref().clone();
        if let Ok(probe) = catalog
            .probe_provider(self.provider.as_ref(), self.credential.as_ref())
            .await
        {
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
                last.get_or_insert(usage.clone());
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

    pub async fn shutdown(self) -> Result<(), AppError> {
        if let Some(store) = self.store {
            store.shutdown().await?;
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

    #[test]
    fn from_resolved_fail_closed_without_env_key() {
        let id = "app-core-missing-key";
        remove_env(&api_key_env_name(id));
        let err = AppCore::from_resolved(sample_config(id), None, None).expect_err("no key");
        let display = format!("{err}");
        match err {
            AppError::MissingCredential { env_name } => {
                assert_eq!(env_name, "PAWORK_API_KEY_APP_CORE_MISSING_KEY");
                assert!(display.contains("PAWORK_API_KEY_APP_CORE_MISSING_KEY"));
            }
            other => panic!("unexpected {other:?}"),
        }
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

        // 目录无条目：禁用而非编造窗口，与 S5 前行为一致。
        let core = core_with_registry(ModelRegistry::builtin(), "no-such-model");
        let context = core.turn_context();
        assert!(context.limits.is_none());
        assert!(context.estimator.is_none());

        // config 覆盖 window=0：显式未知同样禁用。
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
        assert!(core.turn_context().limits.is_none());
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
}
