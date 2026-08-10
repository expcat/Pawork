//! 统一 Command Router（P13-1）：CLI 与 GUI 共用的唯一命令/查询入口。
//!
//! `dispatch` 处理 [`AppCommandEnvelope`]，`dispatch_query` 处理
//! [`AppQueryEnvelope`]，两者都记录来源（[`CommandSource`]）与身份
//! （[`ActorIdentity`]），所有错误统一转为 [`core_api::ErrorContext`] 并包装为
//! [`AppResponse::Error`]。命令先经幂等检查（同 `command_id` / `idempotency_key`
//! 重放首次响应），执行成功后缓存响应；错误响应不缓存。

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use agent_domain::{
    ModelId, ProviderId, QueryId, RunId, SessionId, TerminalSessionId, WorkspaceId,
};
use core_api::{
    ActorIdentity, AppCommand, AppCommandEnvelope, AppQuery, AppQueryEnvelope, AppResponse,
    AppResponseEnvelope, CommandSource, ProviderStatus, API_VERSION,
};
use provider_api::ModelProvider;
use serde_json::json;
use workspace_service::{TrustState, WorkspaceService};

use crate::aggregate::AggregateState;
use crate::approval::ApprovalRegistry;
use crate::error::{
    accepted_response, data_response, error_response, now_timestamp, AppServiceError,
};
use crate::idempotency::{should_cache, IdempotencyCheck, IdempotencyStore};
use crate::rate_limit::{RateLimiter, DEFAULT_RATE_LIMIT_BUFFER, DEFAULT_RATE_LIMIT_WINDOW};
use crate::supervisor::{RunRequest, RunSupervisor, DEFAULT_MAX_CONCURRENT_RUNS};

/// 默认幂等缓存容量。
pub const DEFAULT_IDEMPOTENCY_CAPACITY: usize = 4096;

/// Router 配置。
#[derive(Clone, Debug)]
pub struct RouterConfig {
    pub instance: String,
    pub idempotency_capacity: usize,
    pub rate_limit_window: Duration,
    pub rate_limit_buffer: usize,
    pub max_concurrent_runs: usize,
}

impl Default for RouterConfig {
    fn default() -> Self {
        Self {
            instance: "default".into(),
            idempotency_capacity: DEFAULT_IDEMPOTENCY_CAPACITY,
            rate_limit_window: DEFAULT_RATE_LIMIT_WINDOW,
            rate_limit_buffer: DEFAULT_RATE_LIMIT_BUFFER,
            max_concurrent_runs: DEFAULT_MAX_CONCURRENT_RUNS,
        }
    }
}

/// 统一命令路由器：聚合状态 + 幂等存储 + 限流器 + Run 监督器 + 审批注册表。
pub struct CommandRouter {
    config: RouterConfig,
    instance_id: agent_domain::CoreInstanceId,
    aggregate: Arc<AggregateState>,
    idempotency: IdempotencyStore,
    approvals: Arc<ApprovalRegistry>,
    supervisor: RunSupervisor,
    workspace_service: WorkspaceService,
    model_registry: model_registry::ModelRegistry,
    broadcaster: agent_engine::EventBroadcaster,
    providers: Mutex<BTreeMap<ProviderId, Arc<dyn ModelProvider>>>,
    sources: Mutex<BTreeMap<String, u64>>,
    identities: Mutex<BTreeMap<String, u64>>,
    commands_handled: AtomicU64,
    last_started_run: Mutex<Option<RunId>>,
}

impl CommandRouter {
    pub fn new(config: RouterConfig) -> Self {
        let aggregate = Arc::new(AggregateState::new());
        let approvals = Arc::new(ApprovalRegistry::new());
        let limiter = Arc::new(RateLimiter::new(
            config.rate_limit_window,
            config.rate_limit_buffer,
        ));
        let instance_id = agent_domain::CoreInstanceId::from(config.instance.clone());
        let broadcaster = agent_engine::EventBroadcaster::new();
        let supervisor = RunSupervisor::new(
            config.max_concurrent_runs,
            Arc::clone(&aggregate),
            Arc::clone(&approvals),
            Arc::clone(&limiter),
            broadcaster.clone(),
            instance_id.clone(),
        );
        Self {
            config,
            instance_id,
            aggregate,
            idempotency: IdempotencyStore::new(DEFAULT_IDEMPOTENCY_CAPACITY),
            approvals,
            supervisor,
            workspace_service: WorkspaceService::new(),
            model_registry: model_registry::ModelRegistry::builtin(),
            broadcaster,
            providers: Mutex::new(BTreeMap::new()),
            sources: Mutex::new(BTreeMap::new()),
            identities: Mutex::new(BTreeMap::new()),
            commands_handled: AtomicU64::new(0),
            last_started_run: Mutex::new(None),
        }
    }

    pub fn config(&self) -> &RouterConfig {
        &self.config
    }

    pub fn instance_id(&self) -> &agent_domain::CoreInstanceId {
        &self.instance_id
    }

    pub fn aggregate(&self) -> &AggregateState {
        &self.aggregate
    }

    pub fn supervisor(&self) -> &RunSupervisor {
        &self.supervisor
    }

    pub fn approvals(&self) -> &ApprovalRegistry {
        &self.approvals
    }

    /// 注册一个 Provider 实现（测试注入 MockProvider；正式宿主后续由 provider-runtime 注入）。
    pub fn register_provider(&self, provider: Arc<dyn ModelProvider>) -> ProviderId {
        let id = provider.id();
        lock(&self.providers).insert(id.clone(), provider);
        self.aggregate.record_provider(id.clone(), false, 0);
        id
    }

    pub fn provider_count(&self) -> usize {
        lock(&self.providers).len()
    }

    /// 事件广播订阅（供 GUI 协议 / 测试消费 Agent 事件流）。
    pub fn subscribe_agent_events(&self) -> agent_engine::Subscriber {
        self.broadcaster.subscribe()
    }

    /// 冲刷并取回已限流合并的应用事件。
    pub fn drain_events(&self) -> Vec<core_api::AppEventEnvelope> {
        self.supervisor.drain_events()
    }

    pub fn source_stats(&self) -> BTreeMap<String, u64> {
        lock(&self.sources).clone()
    }

    pub fn identity_stats(&self) -> BTreeMap<String, u64> {
        lock(&self.identities).clone()
    }

    pub fn commands_handled(&self) -> u64 {
        self.commands_handled.load(Ordering::SeqCst)
    }

    /// 最近一次成功启动的 run id（legacy `pawork run` 回显用）。
    pub fn last_started_run(&self) -> Option<RunId> {
        lock(&self.last_started_run).clone()
    }

    /// 统一命令入口。
    pub fn dispatch(&self, envelope: AppCommandEnvelope) -> AppResponseEnvelope {
        self.record_envelope(&envelope.source, &envelope.identity);
        self.commands_handled.fetch_add(1, Ordering::SeqCst);
        let request_id = QueryId::from(envelope.command_id.as_str());

        if !envelope.api_version.is_compatible_with(API_VERSION) {
            return error_response(
                &request_id,
                &AppServiceError::IncompatibleApiVersion {
                    found: envelope.api_version,
                    expected: API_VERSION,
                },
            );
        }

        match self
            .idempotency
            .check(&envelope.command_id, envelope.idempotency_key.as_deref())
        {
            IdempotencyCheck::Replay(response) => return response,
            IdempotencyCheck::New => {}
        }

        let response = match self.execute_command(&envelope) {
            Ok(response) => response,
            Err(error) => error_response(&request_id, &error),
        };
        if should_cache(&response) {
            if let Err(error) = self.idempotency.record(
                &envelope.command_id,
                envelope.idempotency_key.as_deref(),
                response.clone(),
            ) {
                return error_response(&request_id, &AppServiceError::Idempotency(error));
            }
        }
        response
    }

    /// 统一查询入口。
    pub fn dispatch_query(&self, envelope: AppQueryEnvelope) -> AppResponseEnvelope {
        self.record_envelope(&envelope.source, &envelope.identity);
        let request_id = envelope.request_id.clone();
        if !envelope.api_version.is_compatible_with(API_VERSION) {
            return error_response(
                &request_id,
                &AppServiceError::IncompatibleApiVersion {
                    found: envelope.api_version,
                    expected: API_VERSION,
                },
            );
        }
        match self.execute_query(&envelope) {
            Ok(response) => response,
            Err(error) => error_response(&request_id, &error),
        }
    }

    fn execute_command(
        &self,
        envelope: &AppCommandEnvelope,
    ) -> Result<AppResponseEnvelope, AppServiceError> {
        let request_id = QueryId::from(envelope.command_id.as_str());
        match &envelope.command {
            AppCommand::CoreInitialize => {
                self.aggregate.mark_core_ready();
                Ok(data_response(
                    &request_id,
                    json!({
                        "instance": self.config.instance,
                        "api_version": API_VERSION,
                        "core_ready": true,
                    }),
                ))
            }
            AppCommand::WorkspaceAdd { root_path } => {
                self.handle_workspace_add(&request_id, root_path)
            }
            AppCommand::WorkspaceTrust {
                workspace_id,
                trusted,
            } => self.handle_workspace_trust(&request_id, workspace_id, *trusted),
            AppCommand::SessionCreate {
                workspace_id,
                title,
            } => match self.aggregate.create_session(
                workspace_id.clone(),
                title.clone().unwrap_or_default(),
                now_timestamp(),
            ) {
                Ok(session) => Ok(data_response(
                    &request_id,
                    serde_json::to_value(session).map_err(AppServiceError::Json)?,
                )),
                Err(error) => Err(error.into()),
            },
            AppCommand::SessionOpen { session_id } => match self.aggregate.open_session(session_id) {
                Ok(session) => Ok(data_response(
                    &request_id,
                    serde_json::to_value(session).map_err(AppServiceError::Json)?,
                )),
                Err(error) => Err(error.into()),
            },
            AppCommand::SessionFork {
                session_id,
                parent_event_id,
            } => match self.aggregate.fork_session(session_id, parent_event_id.clone()) {
                Ok(session) => Ok(data_response(
                    &request_id,
                    serde_json::to_value(session).map_err(AppServiceError::Json)?,
                )),
                Err(error) => Err(error.into()),
            },
            AppCommand::SessionCompact { session_id } => {
                match self.aggregate.compact_session(session_id) {
                    Ok(session) => Ok(data_response(
                        &request_id,
                        serde_json::to_value(session).map_err(AppServiceError::Json)?,
                    )),
                    Err(error) => Err(error.into()),
                }
            }
            AppCommand::RunStart {
                session_id,
                user_message,
                model,
            } => self.handle_run_start(
                envelope,
                session_id.clone(),
                user_message.clone(),
                model.clone(),
            ),
            AppCommand::RunCancel { run_id } => {
                match self.supervisor.cancel(run_id) {
                    Ok(outcome) => Ok(data_response(
                        &request_id,
                        json!({
                            "run_id": run_id,
                            "cancelled": true,
                            "already_cancelled": outcome.already_cancelled,
                        }),
                    )),
                    Err(error) => Err(error.into()),
                }
            }
            AppCommand::RunRetry { run_id } => match self.supervisor.retry(run_id) {
                Ok(()) => Ok(data_response(
                    &request_id,
                    json!({ "run_id": run_id, "retried": true }),
                )),
                Err(error) => Err(error.into()),
            },
            AppCommand::RunTool {
                run_id,
                tool_name,
                ..
            } => Err(AppServiceError::Unavailable(format!(
                "RunTool `{tool_name}` for run {run_id} is not available until tool-runtime integration"
            ))),
            AppCommand::AuthStart { provider_id, flow } => {
                self.aggregate.record_auth_flow(provider_id, flow);
                Ok(data_response(
                    &request_id,
                    json!({ "provider_id": provider_id, "flow": flow, "status": "started" }),
                ))
            }
            AppCommand::AuthRemove { provider_id } => {
                match self
                    .aggregate
                    .set_provider_status(provider_id, ProviderStatus::AuthenticationRequired)
                {
                    Ok(()) => Ok(data_response(
                        &request_id,
                        json!({ "provider_id": provider_id, "removed": true }),
                    )),
                    Err(error) => Err(error.into()),
                }
            }
            AppCommand::ToolApprove {
                run_id,
                tool_call_id,
                decision,
            } => self.handle_tool_approve(&request_id, run_id, tool_call_id, decision),
            AppCommand::GitStage {
                workspace_id,
                paths,
            } => {
                let paths: Vec<String> = paths
                    .iter()
                    .map(|path| path.as_str().to_string())
                    .collect();
                self.aggregate
                    .record_git_stage(workspace_id.clone(), paths.clone());
                Ok(data_response(
                    &request_id,
                    json!({ "workspace_id": workspace_id, "staged": paths }),
                ))
            }
            AppCommand::TerminalCreate {
                workspace_id,
                working_directory,
            } => {
                let terminal_session_id = TerminalSessionId::from(self.aggregate.next_id("terminal"));
                self.aggregate.record_terminal(
                    workspace_id.clone(),
                    terminal_session_id.clone(),
                    working_directory
                        .as_ref()
                        .map(|path| path.as_str().to_string()),
                );
                Ok(data_response(
                    &request_id,
                    json!({
                        "terminal_session_id": terminal_session_id,
                        "workspace_id": workspace_id,
                    }),
                ))
            }
            AppCommand::TerminalWrite {
                terminal_session_id,
                data,
            } => {
                self.aggregate.record_terminal_output(
                    &TerminalSessionId::from(terminal_session_id.clone()),
                    data,
                );
                Ok(data_response(
                    &request_id,
                    json!({ "terminal_session_id": terminal_session_id, "written": data.len() }),
                ))
            }
            AppCommand::TerminalResize {
                terminal_session_id,
                columns,
                rows,
            } => {
                self.aggregate.record_terminal_resize(
                    &TerminalSessionId::from(terminal_session_id.clone()),
                    *columns,
                    *rows,
                );
                Ok(data_response(
                    &request_id,
                    json!({
                        "terminal_session_id": terminal_session_id,
                        "columns": columns,
                        "rows": rows,
                    }),
                ))
            }
        }
    }

    fn execute_query(
        &self,
        envelope: &AppQueryEnvelope,
    ) -> Result<AppResponseEnvelope, AppServiceError> {
        let request_id = envelope.request_id.clone();
        match &envelope.query {
            AppQuery::WorkspaceList => Ok(data_response(
                &request_id,
                serde_json::to_value(
                    self.workspace_service
                        .list()
                        .map_err(AppServiceError::Workspace)?,
                )
                .map_err(AppServiceError::Json)?,
            )),
            AppQuery::SessionGet { session_id } => match self.aggregate.get_session(session_id) {
                Some(session) => Ok(data_response(
                    &request_id,
                    serde_json::to_value(session).map_err(AppServiceError::Json)?,
                )),
                None => Err(AppServiceError::NotFound(format!("session {session_id}"))),
            },
            AppQuery::RunStatus { run_id } => match self.aggregate.get_run(run_id) {
                Some(run) => Ok(data_response(
                    &request_id,
                    serde_json::to_value(run).map_err(AppServiceError::Json)?,
                )),
                None => Err(AppServiceError::NotFound(format!("run {run_id}"))),
            },
            AppQuery::ModelList { provider_id } => {
                let entries: Vec<_> = self
                    .model_registry
                    .list()
                    .into_iter()
                    .filter(|entry| {
                        provider_id
                            .as_ref()
                            .is_none_or(|provider| &entry.provider == provider)
                    })
                    .collect();
                Ok(data_response(
                    &request_id,
                    serde_json::to_value(entries).map_err(AppServiceError::Json)?,
                ))
            }
            AppQuery::DiffListFiles { workspace_id } => Ok(data_response(
                &request_id,
                serde_json::to_value(self.aggregate.diffs(workspace_id))
                    .map_err(AppServiceError::Json)?,
            )),
            AppQuery::DiffGet {
                workspace_id, path, ..
            } => match self.aggregate.diff_file(workspace_id, path.as_str()) {
                Some(file) => Ok(data_response(
                    &request_id,
                    serde_json::to_value(file).map_err(AppServiceError::Json)?,
                )),
                None => Err(AppServiceError::NotFound(format!(
                    "diff for {} in workspace {workspace_id}",
                    path.as_str()
                ))),
            },
            AppQuery::ArtifactRead {
                artifact_id,
                offset: _,
                limit: _,
            } => match self.aggregate.artifact(artifact_id) {
                Some(record) => Ok(AppResponseEnvelope {
                    api_version: API_VERSION,
                    request_id,
                    responded_at: now_timestamp(),
                    response: AppResponse::Artifact {
                        artifact_id: record.artifact_id,
                        byte_length: record.byte_length,
                        media_type: record.media_type,
                    },
                }),
                None => Err(AppServiceError::NotFound(format!("artifact {artifact_id}"))),
            },
            AppQuery::SnapshotFetch => Ok(data_response(
                &request_id,
                serde_json::to_value(self.aggregate.snapshot()).map_err(AppServiceError::Json)?,
            )),
            AppQuery::PluginList => Ok(data_response(&request_id, json!([]))),
            AppQuery::McpList => Ok(data_response(&request_id, json!([]))),
        }
    }

    fn handle_workspace_add(
        &self,
        request_id: &QueryId,
        root_path: &str,
    ) -> Result<AppResponseEnvelope, AppServiceError> {
        let workspace_id = WorkspaceId::from(self.aggregate.next_id("workspace"));
        let name = Path::new(root_path)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| root_path.to_string());
        let workspace =
            self.workspace_service
                .add(workspace_id.clone(), name, [root_path], now_timestamp())?;
        self.aggregate.record_workspace(workspace.clone());
        Ok(data_response(
            request_id,
            serde_json::to_value(workspace).map_err(AppServiceError::Json)?,
        ))
    }

    fn handle_workspace_trust(
        &self,
        request_id: &QueryId,
        workspace_id: &WorkspaceId,
        trusted: bool,
    ) -> Result<AppResponseEnvelope, AppServiceError> {
        let trust = if trusted {
            TrustState::Trusted
        } else {
            TrustState::Untrusted
        };
        self.workspace_service.set_trust(workspace_id, trust)?;
        let workspace = self
            .workspace_service
            .get(workspace_id)?
            .ok_or_else(|| AppServiceError::NotFound(format!("workspace {workspace_id}")))?;
        self.aggregate.record_workspace(workspace.clone());
        Ok(data_response(
            request_id,
            serde_json::to_value(workspace).map_err(AppServiceError::Json)?,
        ))
    }

    fn handle_run_start(
        &self,
        envelope: &AppCommandEnvelope,
        session_id: SessionId,
        user_message: String,
        model: Option<ModelId>,
    ) -> Result<AppResponseEnvelope, AppServiceError> {
        if user_message.trim().is_empty() {
            return Err(AppServiceError::InvalidRequest(
                "RunStart requires a non-empty user_message".into(),
            ));
        }
        if !self.aggregate.session_exists(&session_id) {
            return Err(AppServiceError::NotFound(format!("session {session_id}")));
        }
        if tokio::runtime::Handle::try_current().is_err() {
            return Err(AppServiceError::NoRuntime);
        }
        let (model, provider_id) = self.resolve_model(model.as_ref())?;
        let provider = {
            let providers = lock(&self.providers);
            providers.get(&provider_id).cloned().ok_or_else(|| {
                AppServiceError::Authentication(format!(
                    "provider {provider_id} is not available; authenticate first"
                ))
            })?
        };
        let run_id = RunId::from(self.aggregate.next_id("run"));
        let rollback_run_id = run_id.clone();
        self.aggregate.record_run(
            run_id.clone(),
            session_id.clone(),
            model.clone(),
            provider_id.clone(),
            envelope.source.clone(),
            now_timestamp(),
        )?;
        let result = self.supervisor.start(
            RunRequest {
                run_id: run_id.clone(),
                session_id,
                provider_id,
                model,
                source: envelope.source.clone(),
                command_id: envelope.command_id.clone(),
                user_message,
            },
            provider,
        );
        if let Err(error) = result {
            self.aggregate.remove_run(&rollback_run_id);
            return Err(error.into());
        }
        *lock(&self.last_started_run) = Some(run_id.clone());
        Ok(accepted_response(envelope))
    }

    fn handle_tool_approve(
        &self,
        request_id: &QueryId,
        run_id: &agent_domain::RunId,
        tool_call_id: &agent_domain::ToolCallId,
        decision: &core_api::ApprovalDecision,
    ) -> Result<AppResponseEnvelope, AppServiceError> {
        self.approvals
            .decide(run_id, tool_call_id, decision.clone())?;
        self.aggregate
            .decide_approval(run_id, tool_call_id, decision.clone())?;
        Ok(data_response(
            request_id,
            json!({
                "run_id": run_id,
                "tool_call_id": tool_call_id,
                "decision": decision,
            }),
        ))
    }

    /// 解析模型与 Provider：显式模型走目录解析；缺省用第一个已注册 Provider
    /// （正式宿主无凭据/未注册时返回结构化 Authentication 错误，绝不 panic）。
    fn resolve_model(
        &self,
        model: Option<&ModelId>,
    ) -> Result<(ModelId, ProviderId), AppServiceError> {
        let providers = lock(&self.providers);
        if providers.is_empty() {
            return Err(AppServiceError::Authentication(
                "no provider is registered; authenticate with a provider first".into(),
            ));
        }
        match model {
            Some(model) => {
                if let Some(entry) = self.model_registry.resolve(model.as_str()) {
                    if providers.contains_key(&entry.provider) {
                        return Ok((entry.id.clone(), entry.provider.clone()));
                    }
                    return Err(AppServiceError::Authentication(format!(
                        "provider {} is not available",
                        entry.provider
                    )));
                }
                Err(AppServiceError::InvalidRequest(format!(
                    "unknown model {model}"
                )))
            }
            None => {
                let provider_id = providers
                    .keys()
                    .next()
                    .expect("non-empty providers checked above")
                    .clone();
                let model = self
                    .model_registry
                    .list()
                    .into_iter()
                    .find(|entry| entry.provider == provider_id)
                    .map(|entry| entry.id.clone())
                    .unwrap_or_else(|| ModelId::from("default-model"));
                Ok((model, provider_id))
            }
        }
    }

    fn record_envelope(&self, source: &CommandSource, identity: &ActorIdentity) {
        let source_name = source_name(source);
        lock(&self.sources)
            .entry(source_name.to_string())
            .and_modify(|count| *count += 1)
            .or_insert(1);
        let identity_name = identity_name(identity);
        lock(&self.identities)
            .entry(identity_name)
            .and_modify(|count| *count += 1)
            .or_insert(1);
    }
}

impl std::fmt::Debug for CommandRouter {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CommandRouter")
            .field("instance", &self.config.instance)
            .field("commands_handled", &self.commands_handled())
            .field("providers", &self.provider_count())
            .finish_non_exhaustive()
    }
}

/// 来源 → 稳定统计键（与 legacy `source_count` 语义一致）。
pub fn source_name(source: &CommandSource) -> &'static str {
    match source {
        CommandSource::LocalCli { .. } => "local_cli",
        CommandSource::LocalGui { .. } => "local_gui",
        CommandSource::RemoteGui { .. } => "remote_gui",
        CommandSource::Automation => "automation",
        CommandSource::Plugin => "plugin",
        CommandSource::Mcp => "mcp",
    }
}

fn identity_name(identity: &ActorIdentity) -> String {
    match identity {
        ActorIdentity::LocalUser { actor_id, .. } => format!("local_user:{}", actor_id),
        ActorIdentity::AuthenticatedClient { subject, .. } => {
            format!("authenticated_client:{subject}")
        }
        ActorIdentity::Automation { name } => format!("automation:{name}"),
        ActorIdentity::Plugin { plugin_id } => format!("plugin:{plugin_id}"),
        ActorIdentity::McpServer { server_id } => format!("mcp_server:{server_id}"),
        ActorIdentity::System => "system".into(),
    }
}

fn lock<T>(inner: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    inner
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
