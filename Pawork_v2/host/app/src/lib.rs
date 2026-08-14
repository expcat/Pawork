//! 应用门面：读配置 → env key → provider → 只读工具 → 事件化 `run_session`。
//!
//! 不按 Provider 名称分支；协议来自 `extra.provider_protocols` 与默认表。
//! 落库 persist-first，再推渲染 sink。

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
    CancellationToken, Message, MessageId, MessageRole, ModelId, ProviderId, RequestId, RunId,
    SessionId, WorkspaceId,
};
use pawork_engine::{
    assemble_request_with_tools, run_session, AgentEventSink, EngineError, SessionTurn,
    DEFAULT_MAX_TOOL_ROUNDS,
};
use pawork_providers::{
    AnthropicConfig, AnthropicProvider, OpenAiCompatibleConfig, OpenAiCompatibleProvider,
};
use pawork_session::{SessionStore, SessionStoreError};
use pawork_tools::{
    FindFilesTool, ListDirectoryTool, ReadFileTool, SearchTextTool, ToolRegistry, ToolRegistryError,
    ToolScheduler, ToolSchedulerConfig,
};
use pawork_workspace::{WorkspaceError, WorkspaceService};
use thiserror::Error;

use crate::loop_ctx::SessionLoopCtx;
use crate::protocol::resolve_adapter_protocol;

pub use data_dir::{default_data_dir, session_db_path};
pub use persist::PersistThenRender;
pub use protocol::{AdapterProtocol, ProtocolError};
pub use pawork_session::SessionRecord;

/// 从配置文件与 CLI 覆盖构造 [`AppCore`] 的选项。
#[derive(Clone, Debug, Default)]
pub struct AppLoadOptions {
    pub workspace_root: Option<PathBuf>,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub data_dir: Option<PathBuf>,
}

impl AppLoadOptions {
    pub fn from_cli(provider: Option<String>, model: Option<String>) -> Self {
        Self {
            workspace_root: std::env::current_dir().ok(),
            provider,
            model,
            data_dir: None,
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
    #[error("S2 只注册只读工具，但 `{name}` 的 read_only 为 false")]
    NonReadOnlyTool { name: String },
}

/// 已装配的 Core：协议中立 provider、只读工具、默认 model、可选 session store。
pub struct AppCore {
    provider: Arc<dyn ModelProvider>,
    credential: Option<ResolvedCredential>,
    model: ModelId,
    provider_id: ProviderId,
    adapter_protocol: AdapterProtocol,
    store: Option<SessionStore>,
    scheduler: Arc<ToolScheduler>,
    workspace_id: WorkspaceId,
    tool_defs: Vec<ToolDefinition>,
    next_request: AtomicU64,
    next_run: AtomicU64,
    next_session: AtomicU64,
    next_message: AtomicU64,
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
        let mut core = Self::from_resolved(
            resolved.config,
            options.provider.as_deref(),
            options.model.as_deref(),
        )?;
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
        let mut core = Self::from_resolved(resolved.config, provider, model)?;
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

        Ok(Self::from_parts_with_protocol(
            adapter,
            Some(credential),
            ModelId::from(model_id.as_str()),
            ProviderId::from(provider_id.as_str()),
            protocol,
            None,
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
        )
    }

    fn from_parts_with_protocol(
        provider: Arc<dyn ModelProvider>,
        credential: Option<ResolvedCredential>,
        model: ModelId,
        provider_id: ProviderId,
        adapter_protocol: AdapterProtocol,
        store: Option<SessionStore>,
    ) -> Self {
        Self {
            provider,
            credential,
            model,
            provider_id,
            adapter_protocol,
            store,
            scheduler: Arc::new(ToolScheduler::new(
                ToolRegistry::new(),
                ToolSchedulerConfig::default(),
            )),
            workspace_id: WorkspaceId::from("ws-unbound"),
            tool_defs: Vec::new(),
            next_request: AtomicU64::new(1),
            next_run: AtomicU64::new(1),
            next_session: AtomicU64::new(1),
            next_message: AtomicU64::new(1),
        }
    }

    /// 把启动目录登记为默认 workspace root，并注册四个只读工具。
    pub fn attach_workspace(&mut self, root: &Path) -> Result<(), AppError> {
        let workspaces = WorkspaceService::new();
        let workspace_id = WorkspaceId::from("ws-default");
        workspaces.add(workspace_id.clone(), "default", [root.to_path_buf()])?;

        let mut registry = ToolRegistry::new();
        registry.extend([
            Arc::new(ReadFileTool::new(workspaces.clone())) as Arc<dyn pawork_api::AgentTool>,
            Arc::new(ListDirectoryTool::new(workspaces.clone())) as Arc<dyn pawork_api::AgentTool>,
            Arc::new(SearchTextTool::new(workspaces.clone())) as Arc<dyn pawork_api::AgentTool>,
            Arc::new(FindFilesTool::new(workspaces)) as Arc<dyn pawork_api::AgentTool>,
        ])?;
        for descriptor in registry.descriptors() {
            if !descriptor.read_only {
                return Err(AppError::NonReadOnlyTool {
                    name: descriptor.name,
                });
            }
        }
        self.tool_defs = registry
            .descriptors()
            .into_iter()
            .map(|descriptor| ToolDefinition {
                name: descriptor.name,
                description: descriptor.description,
                input_schema: descriptor.input_schema,
            })
            .collect();
        self.scheduler = Arc::new(ToolScheduler::new(registry, ToolSchedulerConfig::default()));
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
        Ok(self.store()?.projection_snapshot(session_id).await?.messages)
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
        trigger.id = MessageId::from(format!(
            "msg-{}-{n}",
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
        };
        Ok(run_session(
            self.provider.as_ref(),
            request,
            turn,
            &sink,
            cancel,
            &loop_ctx,
            DEFAULT_MAX_TOOL_ROUNDS,
        )
        .await?)
    }

    pub async fn list_models(&self) -> Result<Vec<ModelDefinition>, ProviderError> {
        self.provider.list_models(self.credential.as_ref()).await
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
                    AgentEvent::ToolExecutionStarted { .. } => "ToolExecutionStarted",
                    AgentEvent::ToolExecutionCompleted { .. } => "ToolExecutionCompleted",
                    AgentEvent::ToolOutputDelta { .. } => "ToolOutputDelta",
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
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("session.db");
        let (store, _) = SessionStore::open(&path).await.expect("store");
        let summary = ModelResponseSummary {
            stop_reason: StopReason::Completed,
            usage: TokenUsage::default(),
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
    async fn attach_workspace_registers_four_readonly_tools() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (mut core, _store_dir) = mock_core(Vec::new()).await;
        core.attach_workspace(dir.path()).expect("attach");
        let mut names = core.tool_names();
        names.sort();
        assert_eq!(
            names,
            vec!["find_files", "list_directory", "read_file", "search_text"]
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
    fn session_title_truncates() {
        assert_eq!(session_title_from_text("  hello   world  "), "hello world");
        assert_eq!(session_title_from_text(""), "New session");
        let long = "x".repeat(80);
        let title = session_title_from_text(&long);
        assert!(title.ends_with('…'));
        assert_eq!(title.chars().count(), 72);
    }
}
