//! 统一 Command Router（P13-1）：CLI 与 GUI 共用的唯一命令/查询入口。
//!
//! `dispatch` 处理 [`AppCommandEnvelope`]，`dispatch_query` 处理
//! [`AppQueryEnvelope`]，两者都记录来源（[`CommandSource`]）与身份
//! （[`ActorIdentity`]），所有错误统一转为 [`core_api::ErrorContext`] 并包装为
//! [`AppResponse::Error`]。命令先经幂等检查（同 `command_id` / `idempotency_key`
//! 重放首次响应），执行成功后缓存响应；错误响应不缓存。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use agent_domain::{
    ModelId, ProviderId, QueryId, RunId, SessionId, TenantId, TerminalSessionId, WorkspaceId,
};
use core_api::{
    mask_credential_hint, QuotaOverviewQuery, QuotaOverviewView, QuotaScopeView, QuotaWindow,
    WindowReadEntry, WindowReadView,
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
    accepted_response_with_run, data_response, error_response, now_timestamp, AppServiceError,
};
use crate::idempotency::{should_cache, IdempotencyCheck, IdempotencyStore};
use crate::profile_resolver::{ModelLanding, ModelOverrideDecision, ModelOverrideRequest};
use crate::rate_limit::{RateLimiter, DEFAULT_RATE_LIMIT_BUFFER, DEFAULT_RATE_LIMIT_WINDOW};
use crate::supervisor::{RunRequest, RunSupervisor, DEFAULT_MAX_CONCURRENT_RUNS};
use crate::QuotaRuntime;

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
    quota_runtime: Mutex<Option<Arc<QuotaRuntime>>>,
    /// P17-5 主 run profile 解析（生产 ResourceLoader 装配注入）；未注入时
    /// RunStart 携带 profile 名一律 fail-closed。
    profile_resolver: Mutex<Option<Arc<dyn crate::profile_resolver::RunProfileResolver>>>,
    /// P17-5 隔离能力探测（默认生产 SandboxIsolationCapability；测试可覆盖）。
    isolation: Mutex<Option<Arc<dyn crate::profile_resolver::IsolationCapability>>>,
    /// P17-5 模型覆盖授权策略（缺省 DenyAll，fail-closed；宿主可注入生产
    /// 策略）。显式模型与 profile canonical 落点不同时由它裁决，绝不直接
    /// 信任 caller。
    model_override_policy: Mutex<Arc<dyn crate::profile_resolver::ModelOverridePolicy>>,
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
            quota_runtime: Mutex::new(None),
            profile_resolver: Mutex::new(None),
            isolation: Mutex::new(None),
            model_override_policy: Mutex::new(Arc::new(
                crate::profile_resolver::DenyAllModelOverridePolicy,
            )),
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

    /// 幂等 materialize（P17-7 跨 host/进程 resume）：以 registry 权威
    /// `core_session_id` 在本地聚合重建会话记录；已存在时 no-op。不生成
    /// 新 id、不重绑 registry 映射——重试/并发安全，不留 ghost session。
    pub fn materialize_session(
        &self,
        session_id: &SessionId,
        workspace_id: &WorkspaceId,
        title: String,
        created_at: agent_domain::Timestamp,
    ) -> Result<crate::aggregate::SessionRecord, AppServiceError> {
        self.aggregate
            .materialize_session(session_id.clone(), workspace_id.clone(), title, created_at)
            .map_err(Into::into)
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

    /// 按 ProviderId 取共享 Provider（User Hook 判定执行器 / 宿主装配用）。
    pub(crate) fn provider(&self, id: &ProviderId) -> Option<Arc<dyn ModelProvider>> {
        lock(&self.providers).get(id).cloned()
    }

    /// 按 ProviderId 升序取第一个已注册 Provider（User Hook 默认判定
    /// profile 的兜底落点；无注册时为 `None`，判定 fail-closed）。
    pub(crate) fn first_provider(&self) -> Option<Arc<dyn ModelProvider>> {
        lock(&self.providers).values().next().cloned()
    }

    pub fn provider_count(&self) -> usize {
        lock(&self.providers).len()
    }

    /// 事件广播订阅（供 GUI 协议 / 测试消费 Agent 事件流）。
    pub fn subscribe_agent_events(&self) -> agent_engine::Subscriber {
        self.broadcaster.subscribe()
    }

    /// 返回共享 Team 事件桥（P17-6）：TeamService 的 typed EventHub 出口。
    pub fn team_sink(&self) -> Arc<dyn teams::TeamEventSink> {
        self.supervisor.team_sink()
    }

    /// 注入共享 User Hooks 宿主（P17-1）：run 的 pre-prompt / pre-tool 权威
    /// 位点回灌 hooks 结果。幂等：同一实例重复注入为 no-op。
    pub fn set_user_hooks(&self, host: Arc<crate::user_hook::UserHookHost>) {
        self.supervisor.set_user_hooks(host);
    }

    /// 注入 run 的 workspace roots（P17-1）：run loop 的 pre-prompt / pre-tool
    /// 权威位点把它传给 UserHookHost（workspace 作用域匹配与 Command handler
    /// 的 cwd 解析）。与 [`Self::set_user_hooks`] 同生命周期，宿主装配时注入。
    pub fn set_workspace_roots(&self, roots: Vec<PathBuf>) {
        self.supervisor.set_workspace_roots(roots);
    }

    /// 是否已注入共享 User Hooks 宿主（宿主装配 / 诊断用）。
    pub fn user_hooks_active(&self) -> bool {
        self.supervisor.user_hooks_active()
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

    /// 注入进程内共享的 Quota 运行时（P14-8）。同时把同一 ledger 透传给
    /// supervisor，供成功 run 完成后幂等记账。幂等：重复注入同一实例为 no-op。
    pub fn set_quota_runtime(&self, runtime: Arc<QuotaRuntime>) {
        let mut guard = lock(&self.quota_runtime);
        let already = guard
            .as_ref()
            .is_some_and(|existing| Arc::ptr_eq(existing, &runtime));
        if already {
            return;
        }
        self.supervisor.set_quota_runtime(Arc::clone(&runtime));
        *guard = Some(runtime);
    }

    /// 当前注入的 Quota 运行时（测试 / 宿主诊断）。
    pub fn quota_runtime(&self) -> Option<Arc<QuotaRuntime>> {
        lock(&self.quota_runtime).clone()
    }

    /// 注入 P17-5 主 run profile 解析器（生产 ResourceLoader 装配）。幂等：
    /// 同一实例重复注入为 no-op。未注入时 RunStart 携带 profile 名一律
    /// fail-closed（无可用 profile 源）。
    pub fn set_profile_resolver(
        &self,
        resolver: Arc<dyn crate::profile_resolver::RunProfileResolver>,
    ) {
        let mut guard = lock(&self.profile_resolver);
        let already = guard
            .as_ref()
            .is_some_and(|existing| Arc::ptr_eq(existing, &resolver));
        if !already {
            *guard = Some(resolver);
        }
    }

    fn profile_resolver(&self) -> Option<Arc<dyn crate::profile_resolver::RunProfileResolver>> {
        lock(&self.profile_resolver).clone()
    }

    /// 注入 P17-5 隔离能力探测（默认生产 SandboxIsolationCapability；测试可
    /// 覆盖以断言 fail-closed 分支）。幂等：同一实例重复注入为 no-op。
    pub fn set_isolation_capability(
        &self,
        capability: Arc<dyn crate::profile_resolver::IsolationCapability>,
    ) {
        let mut guard = lock(&self.isolation);
        let already = guard
            .as_ref()
            .is_some_and(|existing| Arc::ptr_eq(existing, &capability));
        if !already {
            *guard = Some(capability);
        }
    }

    fn isolation_capability(&self) -> Arc<dyn crate::profile_resolver::IsolationCapability> {
        lock(&self.isolation)
            .clone()
            .unwrap_or_else(|| Arc::new(crate::profile_resolver::SandboxIsolationCapability))
    }

    /// 注入 P17-5 模型覆盖授权策略（生产装配
    /// [`crate::profile_resolver::ProductionModelOverridePolicy`]；缺省
    /// DenyAll fail-closed）。幂等：同一实例重复注入为 no-op。
    pub fn set_model_override_policy(
        &self,
        policy: Arc<dyn crate::profile_resolver::ModelOverridePolicy>,
    ) {
        let mut guard = lock(&self.model_override_policy);
        if !Arc::ptr_eq(&*guard, &policy) {
            *guard = policy;
        }
    }

    /// 注入 P17-5 后台任务管理器：background=true 的 run 经它注册 / 启动 /
    /// 完成 / 取消一个 TaskKind::Agent，复用既有状态机，不自建。幂等。
    pub fn set_task_manager(&self, manager: Arc<task_manager::TaskManager>) {
        self.supervisor.set_task_manager(manager);
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
            AppCommand::SessionClientContextReplace {
                session_id,
                snapshot,
            } => {
                // P17-9：Host 权威盖戳后，仅 Automation / LocalCli 可替换 IDE
                // 上下文。GUI 盖戳为 LocalGui/RemoteGui；插件与 MCP 也不得注入。
                match &envelope.source {
                    CommandSource::Automation | CommandSource::LocalCli { .. } => {}
                    other => {
                        return Err(AppServiceError::Authorization(format!(
                            "session_client_context_replace is not permitted for {:?}",
                            source_name(other)
                        )));
                    }
                }
                match self
                    .aggregate
                    .replace_client_context(session_id, snapshot.clone())
                {
                    Ok(snapshot) => Ok(data_response(
                        &request_id,
                        json!({
                            "session_id": session_id,
                            "revision": snapshot.revision,
                            "replaced": true,
                        }),
                    )),
                    Err(error) => Err(error.into()),
                }
            },
          AppCommand::RunStart {
              session_id,
              user_message,
              model,
              profile,
          } => self.handle_run_start(
              envelope,
              session_id.clone(),
              user_message.clone(),
              model.clone(),
              profile.clone(),
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
            AppQuery::QuotaOverview { query } => {
                self.handle_quota_overview(&request_id, &envelope.source, &envelope.identity, query)
            }
            AppQuery::PluginList => Ok(data_response(&request_id, json!([]))),
            AppQuery::McpList => Ok(data_response(&request_id, json!([]))),
        }
    }

    /// QuotaOverview 查询（P14-8）：授权 + 仅读缓存。
    ///
    /// 授权：默认作用域（local/local/default）允许 LocalCli / LocalGui + LocalUser，或任意
    /// System 身份读取任意 tenant/account；其余远程/插件/MCP 身份与非默认作用域
    /// 必须有显式 grant（当前无 grant 存储 → 一律拒绝，返回 Authorization 错误）。
    /// 契约：`provider_id` 必须显式提供（P14 review §2.4）；缺省或空字符串不再
    /// 选择“首个已注册 provider”或默认 ID，而是返回明确的 validation error。
    /// 多 provider 聚合语义待 P18 binding enumeration 成为事实源后再批量查询。
    /// 同步查询只读 quota-service 缓存：缓存空时每个窗口返回
    /// [`WindowReadView::NoData`] 且 `from_cache = false`，绝不触发 adapter / 网络。
    #[allow(clippy::too_many_arguments)]
    fn handle_quota_overview(
        &self,
        request_id: &QueryId,
        source: &CommandSource,
        identity: &ActorIdentity,
        query: &QuotaOverviewQuery,
    ) -> Result<AppResponseEnvelope, AppServiceError> {
        if !Self::authorize_quota_query(source, identity, query) {
            return Err(AppServiceError::Authorization(format!(
                "quota query for tenant {} / account {} is not permitted for this identity",
                query.tenant_id.as_str(),
                query.account_id,
            )));
        }
        let runtime = self.quota_runtime();
        // P14 review §2.4：不再静默选择首个已注册 provider 或空默认 ID；
        // 显式 provider 是查询的必要维度，缺失即拒绝。
        let provider_id = match query.provider_id.as_ref() {
            Some(provider) if !provider.as_str().is_empty() => provider.clone(),
            Some(_) | None => {
                return Err(AppServiceError::InvalidRequest(
                    "QuotaOverview requires an explicit non-empty provider_id; \
                     no default provider is selected"
                        .into(),
                ));
            }
        };
        let view = match runtime {
            Some(runtime) => Self::cached_quota_overview(&runtime, query, provider_id),
            None => Self::empty_quota_overview(query, provider_id),
        };
        let data = serde_json::to_value(&view).map_err(AppServiceError::Json)?;
        Ok(data_response(request_id, data))
    }

    /// 授权判定（P14-8）：见 [`Self::handle_quota_overview`] 文档。
    fn authorize_quota_query(
        source: &CommandSource,
        identity: &ActorIdentity,
        query: &QuotaOverviewQuery,
    ) -> bool {
        // System 身份可读任意作用域（内部监控）。
        if matches!(identity, ActorIdentity::System) {
            return true;
        }
        // 默认作用域（local/local/default）允许本地 CLI / GUI + LocalUser。
        let local_frontend = matches!(
            source,
            CommandSource::LocalCli { .. } | CommandSource::LocalGui { .. }
        );
        let local_user = matches!(identity, ActorIdentity::LocalUser { .. });
        if query.is_default_scope() && local_frontend && local_user {
            return true;
        }
        // 其余（RemoteGui / AuthenticatedClient / Plugin / Mcp，或非默认作用域）
        // 需要显式 grant；当前无 grant 存储，一律拒绝。
        false
    }

    /// 把 [`QuotaOverviewQuery`] 映射到 canonical scope + 窗口/单位，并通过
    /// `QuotaService::overview_cache_only` 同步读取缓存。该 API 不会触发 adapter、
    /// 网络或 singleflight。
    ///
    /// `pub(crate)`：仅 supervisor 在成功记账并刷新本地缓存后，为同一
    /// model/credential scope 构建 [`AppEvent::QuotaChanged`] 视图时复用；
    /// 不改动查询路径语义。
    pub(crate) fn cached_quota_overview(
        runtime: &Arc<QuotaRuntime>,
        query: &QuotaOverviewQuery,
        provider_id: ProviderId,
    ) -> QuotaOverviewView {
        let scope = quota_scope_for_query(query, provider_id.clone());
        let requested = requested_windows(query);
        // overview 需要 canonical（quota_service）窗口；core_api 与 canonical 1:1 镜像。
        let windows: Vec<quota_service::QuotaWindow> =
            requested.iter().map(|w| to_canonical_window(*w)).collect();
        let unit = query
            .unit
            .as_ref()
            .map(to_canonical_unit)
            .unwrap_or(quota_service::QuotaUnit::Token);
        match runtime.quota.overview_cache_only(&scope, &windows, &unit) {
            Ok(overview) => convert_cache_overview(query, &overview, &requested),
            Err(error) => failed_cache_overview(
                query,
                provider_id,
                &requested,
                // 缓存校验失败是查询级错误（scope 非法等），无 adapter 归属：
                // 包装为 domain failure，视图映射为 adapter_kind = None。
                &quota_service::service::QuotaFailure::domain(error),
            ),
        }
    }

    /// 无 quota 运行时 / 缓存未命中：每个请求窗口返回 NoData，from_cache=false。
    fn empty_quota_overview(
        query: &QuotaOverviewQuery,
        provider_id: ProviderId,
    ) -> QuotaOverviewView {
        let windows = requested_windows(query);
        let entries = windows
            .into_iter()
            .map(|window| WindowReadEntry {
                window,
                read: WindowReadView::NoData,
            })
            .collect();
        QuotaOverviewView {
            scope: empty_scope_view(query, provider_id),
            windows: entries,
            generated_at: now_timestamp(),
            from_cache: false,
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
        profile: Option<String>,
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
        // run 归属 workspace 从 session 聚合取（hooks 的 workspace 作用域匹配
        // 与 P17-5 profile 解析都依赖它）。
        let workspace_id = self
            .aggregate
            .get_session(&session_id)
            .map(|session| session.workspace_id);
        // P17-5：可选 profile 名解析为 loader 已校验的不可变 AgentProfileV2。
        // 未知 / 跨 workspace / 引用不可用 / 未注入解析器一律 fail-closed。
        let resolved_profile =
            self.resolve_run_profile(profile.as_deref(), workspace_id.as_ref())?;
        let (model, provider_id) = match (model.as_ref(), resolved_profile.as_ref()) {
            // 显式命令模型优先（caller 权威）：但与 profile canonical 落点
            // 不同时属于模型覆盖，必须经 ModelOverridePolicy 授权（缺省
            // fail-closed 全拒，绝不直接信任 caller）。profile 未声明模型 /
            // 同模型落点（别名归一后相同）不构成 override，不误拒。
            (Some(command_model), Some(resolved)) => {
                let from = self.profile_canonical_landing(&resolved.profile)?;
                let (model, provider) = self.resolve_model(Some(command_model))?;
                if let Some(from) = from {
                    let to = ModelLanding {
                        provider_id: provider.clone(),
                        model_id: model.clone(),
                    };
                    if from != to {
                        self.authorize_model_override(
                            &envelope.source,
                            &envelope.identity,
                            &resolved.workspace_id,
                            &resolved.profile.name,
                            &from,
                            &to,
                        )?;
                    }
                }
                (model, provider)
            }
            // 无 profile：显式命令模型直接解析（不存在可被覆盖的 canonical
            // 落点，无授权需求）。
            (Some(command_model), None) => self.resolve_model(Some(command_model))?,
            // 否则 profile 模型 canonical 解析（provider 必须已注册，fail-closed）。
            (None, Some(resolved)) => self.resolve_profile_model(&resolved.profile)?,
            // 既无命令模型也无 profile：默认解析。
            (None, None) => self.resolve_model(None)?,
        };
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
        // Run 前查询当前 scope 的缓存额度信号，注入 ProviderLoop（仅新鲜 Exact
        // exhaustion 才硬停）。无 quota 运行时 / 无缓存 → None（不影响运行）。
        let external_quota = self
            .quota_runtime()
            .and_then(|runtime| cached_quota_signal(&runtime, &provider_id, &model));
        let result = self.supervisor.start(
            RunRequest {
                run_id: run_id.clone(),
                session_id: session_id.clone(),
                // run 归属 workspace 从 session 聚合取（hooks 的 workspace
                // 作用域匹配与 P17-5 profile 解析都依赖它）。
                workspace_id,
                provider_id,
                model,
                source: envelope.source.clone(),
                command_id: envelope.command_id.clone(),
                user_message,
                external_quota,
                profile: resolved_profile,
            },
            provider,
        );
        if let Err(error) = result {
            self.aggregate.remove_run(&rollback_run_id);
            return Err(error.into());
        }
        // RunStart 响应必须携带本命令确定启动的 run id：并发来源各自从
        // 自己的响应绑定 run，不依赖全局状态（P17-7 评审 #3）。
        Ok(accepted_response_with_run(envelope, Some(run_id)))
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

    /// P17-5：把可选 profile 名解析为 loader 已校验的不可变 AgentProfileV2。
    ///
    /// fail-closed 语义：未知 / 跨 workspace / 引用不可用 / 未注入解析器 / 缺
    /// workspace 绑定均返回结构化错误，绝不静默回退默认模型或默认 profile。
    /// memory enabled + 显式 Unavailable 与 isolation 当前宿主无法真实满足
    /// （Container 无真实容器后端）也在此拒绝，绝不虚假可用 / 静默降级。
    fn resolve_run_profile(
        &self,
        profile: Option<&str>,
        workspace_id: Option<&WorkspaceId>,
    ) -> Result<Option<crate::profile_resolver::ResolvedRunProfile>, AppServiceError> {
        let Some(name) = profile else {
            return Ok(None);
        };
        let Some(workspace_id) = workspace_id else {
            return Err(AppServiceError::InvalidRequest(format!(
                "profile `{name}` requires a session bound to a workspace"
            )));
        };
        let Some(resolver) = self.profile_resolver() else {
            return Err(AppServiceError::Unavailable(format!(
                "profile `{name}` cannot be resolved: no profile resolver is configured"
            )));
        };
        let resolved = resolver
            .resolve(workspace_id, name)
            .map_err(|error| AppServiceError::InvalidRequest(error.to_string()))?;
        // memory：enabled + 显式 Unavailable 拒绝 run（绝不虚假可用）。
        if resolved.profile.memory.enabled
            && matches!(
                resolved.profile.memory.availability(),
                agent_domain::ProfileMemoryAvailability::Unavailable
            )
        {
            return Err(AppServiceError::Unavailable(format!(
                "profile `{name}` requests memory that is unavailable: {}",
                resolved
                    .profile
                    .memory
                    .unavailable
                    .as_deref()
                    .unwrap_or("unspecified")
            )));
        }
        // isolation：当前宿主无法真实满足时 fail-closed（Container 无真实
        // 容器后端必失败，绝不静默降级）。
        let isolation = self.isolation_capability();
        if !isolation.satisfiable(resolved.profile.isolation) {
            return Err(AppServiceError::Unavailable(format!(
                "profile `{name}` requires isolation `{:?}` that is unavailable on this host",
                resolved.profile.isolation
            )));
        }
        Ok(Some(resolved))
    }

    /// P17-5：profile.model canonical 解析。provider 必须已注册（fail-closed）；
    /// model 名优先取 registry canonical 条目，否则直接用作 ModelId。profile
    /// 既未声明 provider 也未声明 model 时返回错误（caller 应显式传模型）。
    fn resolve_profile_model(
        &self,
        profile: &agent_domain::AgentProfileV2,
    ) -> Result<(ModelId, ProviderId), AppServiceError> {
        let providers = lock(&self.providers);
        let Some(provider_name) = profile.model.provider.as_deref() else {
            return Err(AppServiceError::InvalidRequest(format!(
                "profile `{}` declares no model provider; pass an explicit model",
                profile.name
            )));
        };
        let provider_id = ProviderId::from(provider_name);
        if !providers.contains_key(&provider_id) {
            return Err(AppServiceError::Authentication(format!(
                "provider {provider_id} from profile `{}` is not available",
                profile.name
            )));
        }
        let Some(model_name) = profile.model.name.as_deref() else {
            return Err(AppServiceError::InvalidRequest(format!(
                "profile `{}` declares a provider but no model name",
                profile.name
            )));
        };
        // 优先 registry canonical 条目（同 provider）；否则直接用 profile 声明名。
        let model = self
            .model_registry
            .resolve(model_name)
            .filter(|entry| entry.provider == provider_id)
            .map(|entry| entry.id.clone())
            .unwrap_or_else(|| ModelId::from(model_name));
        Ok((model, provider_id))
    }

    /// P17-5：profile 的 canonical 模型落点。
    ///
    /// 仅当 profile 同时声明 provider 与 model 名时才存在（`Ok(Some)`）；
    /// 未声明时返回 `Ok(None)`——显式命令模型只是补全 profile 缺失的模型，
    /// 不构成 override。provider 声明但未注册时 fail-closed（与
    /// [`Self::resolve_profile_model`] 一致）。
    fn profile_canonical_landing(
        &self,
        profile: &agent_domain::AgentProfileV2,
    ) -> Result<Option<ModelLanding>, AppServiceError> {
        if profile.model.provider.is_none() || profile.model.name.is_none() {
            return Ok(None);
        }
        let (model_id, provider_id) = self.resolve_profile_model(profile)?;
        Ok(Some(ModelLanding {
            provider_id,
            model_id,
        }))
    }

    /// P17-5：模型覆盖授权（resolve 后 / record_run 前）。
    ///
    /// 显式模型与 profile canonical 落点不同时，以 source + identity +
    /// workspace + profile/from/to 提交给注入策略；Deny 返回结构化
    /// Authorization 错误，绝不静默放行。
    fn authorize_model_override(
        &self,
        source: &CommandSource,
        identity: &ActorIdentity,
        workspace_id: &WorkspaceId,
        profile_name: &str,
        from: &ModelLanding,
        to: &ModelLanding,
    ) -> Result<(), AppServiceError> {
        let request = ModelOverrideRequest {
            source: source.clone(),
            identity: identity.clone(),
            workspace_id: workspace_id.clone(),
            profile_name: profile_name.to_string(),
            from: from.clone(),
            to: to.clone(),
        };
        match lock(&self.model_override_policy).allow(&request) {
            ModelOverrideDecision::Allow => Ok(()),
            ModelOverrideDecision::Deny => Err(AppServiceError::Authorization(format!(
                "model override denied by policy: profile `{profile_name}` pins \
                 {}/{} but explicit model lands on {}/{}",
                from.provider_id, from.model_id, to.provider_id, to.model_id
            ))),
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

// =========================================================================
// Quota（P14-8）缓存查询 / 信号派生 / canonical → view 转换。
// =========================================================================

/// 默认查询窗口（Overall + 常用滚动窗口），与 quota-service 对齐。
const DEFAULT_QUOTA_WINDOWS: [QuotaWindow; 4] = [
    QuotaWindow::Overall,
    QuotaWindow::Rolling5h,
    QuotaWindow::Weekly,
    QuotaWindow::Monthly,
];

fn requested_windows(query: &QuotaOverviewQuery) -> Vec<QuotaWindow> {
    if query.windows.is_empty() {
        DEFAULT_QUOTA_WINDOWS.to_vec()
    } else {
        query.windows.clone()
    }
}

/// core_api 查询 → quota-service canonical scope。
fn quota_scope_for_query(
    query: &QuotaOverviewQuery,
    provider_id: ProviderId,
) -> quota_service::QuotaScope {
    let mut scope = quota_service::QuotaScope::new(
        query.tenant_id.clone(),
        quota_service::AccountId::new(query.account_id.clone()),
        provider_id,
        query.model_id.clone(),
    );
    if let Some(cred) = query.credential_id.clone() {
        scope = scope.with_credential_id(cred);
    }
    scope
}

/// 从缓存中派生 run 前额度信号（remaining_ratio_ppm + exhausted）。
///
/// 对当前 canonical scope 的 Token / Count 与全部 canonical window 做 provider-neutral
/// 扫描；不判断 Provider 名称，也不调用 adapter / 网络。任何 fresh Exact exhausted
/// 候选优先；其余候选严格按 Exact > Derived > Scraped，再按新鲜度和剩余比例
/// 择优，仅形成软信号。
fn cached_quota_signal(
    runtime: &Arc<QuotaRuntime>,
    provider_id: &ProviderId,
    model: &ModelId,
) -> Option<agent_engine::ExternalQuotaSignal> {
    let scope = quota_service::QuotaScope::new(
        TenantId::new(core_api::DEFAULT_QUOTA_TENANT),
        quota_service::AccountId::new(core_api::DEFAULT_QUOTA_ACCOUNT),
        provider_id.clone(),
        Some(model.clone()),
    );
    let windows = [
        quota_service::QuotaWindow::Overall,
        quota_service::QuotaWindow::Rolling5h,
        quota_service::QuotaWindow::Weekly,
        quota_service::QuotaWindow::Monthly,
    ];
    let units = [
        quota_service::QuotaUnit::Token,
        quota_service::QuotaUnit::Count,
    ];
    let mut candidates = Vec::new();
    for unit in units {
        let Ok(overview) = runtime.quota.overview_cache_only(&scope, &windows, &unit) else {
            continue;
        };
        for read in overview.windows.values() {
            if let Some((snapshot, stale)) = cache_window_read_snapshot(read) {
                if let Some(candidate) = quota_signal_from_snapshot(snapshot, stale) {
                    candidates.push(candidate);
                }
            }
        }
    }
    candidates.into_iter().max_by_key(quota_signal_rank)
}

/// 单窗口缓存读的压平视图：`(snapshot, stale)`。
///
/// quota-service 已压平 cache 结果（P14 review §3.6）：每个窗口都是扁平
/// [`quota_service::service::CacheRead`]，本函数是 app-service 对 cache 读
/// 形态的唯一匹配点，两个消费点（run 前额度信号、overview 视图）都经它
/// 取快照与新鲜度。
fn cache_window_read_snapshot(
    read: &quota_service::service::CacheRead,
) -> Option<(&quota_service::QuotaSnapshot, bool)> {
    match read {
        quota_service::service::CacheRead::Hit { snapshot } => Some((snapshot, false)),
        quota_service::service::CacheRead::Stale { snapshot } => Some((snapshot, true)),
        quota_service::service::CacheRead::NoData => None,
    }
}

fn quota_signal_from_snapshot(
    snapshot: &quota_service::QuotaSnapshot,
    cache_stale: bool,
) -> Option<agent_engine::ExternalQuotaSignal> {
    let limit = match snapshot.values.limit {
        quota_service::QuotaMeasure::Exact(value) => value,
        quota_service::QuotaMeasure::Infinite => {
            return Some(agent_engine::ExternalQuotaSignal {
                remaining_ratio_ppm: 1_000_000,
                exhausted: false,
                stale: cache_stale || snapshot.provenance.stale,
                confidence: convert_signal_confidence(snapshot.confidence),
            });
        }
        quota_service::QuotaMeasure::Unknown => return None,
    };
    let used = match snapshot.values.used {
        quota_service::QuotaMeasure::Exact(value) => Some(value),
        quota_service::QuotaMeasure::Infinite | quota_service::QuotaMeasure::Unknown => None,
    };
    let remaining = match snapshot.values.remaining {
        quota_service::QuotaMeasure::Exact(value) => value,
        quota_service::QuotaMeasure::Infinite => return None,
        quota_service::QuotaMeasure::Unknown => used.map(|value| limit.saturating_sub(value))?,
    };
    let remaining_ratio_ppm = if limit == 0 {
        0
    } else {
        (((remaining as u128) * 1_000_000u128) / (limit as u128)).min(1_000_000) as u64
    };
    let exhausted = limit == 0 || used.is_some_and(|value| value >= limit) || remaining == 0;
    Some(agent_engine::ExternalQuotaSignal {
        remaining_ratio_ppm,
        exhausted,
        stale: cache_stale || snapshot.provenance.stale,
        confidence: convert_signal_confidence(snapshot.confidence),
    })
}

fn convert_signal_confidence(
    confidence: quota_service::Confidence,
) -> agent_engine::QuotaSignalConfidence {
    match confidence {
        quota_service::Confidence::Exact => agent_engine::QuotaSignalConfidence::Exact,
        quota_service::Confidence::Derived => agent_engine::QuotaSignalConfidence::Derived,
        quota_service::Confidence::Scraped => agent_engine::QuotaSignalConfidence::Scraped,
    }
}

fn quota_signal_rank(signal: &agent_engine::ExternalQuotaSignal) -> (u8, u8, u8, u64) {
    let hard = signal.exhausted
        && !signal.stale
        && signal.confidence == agent_engine::QuotaSignalConfidence::Exact;
    (
        u8::from(hard),
        signal.confidence.priority(),
        u8::from(!signal.stale),
        1_000_000u64.saturating_sub(signal.remaining_ratio_ppm.min(1_000_000)),
    )
}

/// quota-service cache-only overview → core_api 安全视图（脱敏 credential）。
fn convert_cache_overview(
    query: &QuotaOverviewQuery,
    overview: &quota_service::service::CacheOverview,
    requested: &[QuotaWindow],
) -> QuotaOverviewView {
    let scope_view = scope_view_from(&overview.scope, query.credential_id.as_deref());
    let mut entries = Vec::with_capacity(requested.len());
    let mut from_cache = false;
    for window in requested {
        let read = overview.windows.get(&to_canonical_window(*window));
        let view = match read.and_then(cache_window_read_snapshot) {
            Some((snapshot, stale)) => {
                from_cache = true;
                WindowReadView::Ok {
                    snapshot: Box::new(snapshot_view_from(snapshot, stale)),
                    failures: Vec::new(),
                }
            }
            None => WindowReadView::NoData,
        };
        entries.push(WindowReadEntry {
            window: *window,
            read: view,
        });
    }
    QuotaOverviewView {
        scope: scope_view,
        windows: entries,
        generated_at: now_timestamp(),
        from_cache,
    }
}

fn failed_cache_overview(
    query: &QuotaOverviewQuery,
    provider_id: ProviderId,
    requested: &[QuotaWindow],
    failure: &quota_service::service::QuotaFailure,
) -> QuotaOverviewView {
    let failure = failure_view_from(failure);
    QuotaOverviewView {
        scope: empty_scope_view(query, provider_id),
        windows: requested
            .iter()
            .map(|window| WindowReadEntry {
                window: *window,
                read: WindowReadView::Failed {
                    failures: vec![failure.clone()],
                },
            })
            .collect(),
        generated_at: now_timestamp(),
        from_cache: false,
    }
}

fn empty_scope_view(query: &QuotaOverviewQuery, provider_id: ProviderId) -> QuotaScopeView {
    QuotaScopeView {
        tenant_id: query.tenant_id.clone(),
        account_id: query.account_id.clone(),
        provider_id,
        model_id: query.model_id.clone(),
        credential_hint: query
            .credential_id
            .as_deref()
            .and_then(mask_credential_hint),
    }
}

fn scope_view_from(
    scope: &quota_service::QuotaScope,
    credential_id: Option<&str>,
) -> QuotaScopeView {
    QuotaScopeView {
        tenant_id: scope.tenant_id.clone(),
        account_id: scope.account_id.as_str().to_string(),
        provider_id: scope.provider_id.clone(),
        model_id: scope.model_id.clone(),
        credential_hint: credential_id
            .or(scope.credential_id.as_deref())
            .and_then(mask_credential_hint),
    }
}

fn snapshot_view_from(
    snapshot: &quota_service::QuotaSnapshot,
    served_stale: bool,
) -> core_api::QuotaSnapshotView {
    let mut provenance = convert_provenance(&snapshot.provenance);
    provenance.stale |= served_stale;
    core_api::QuotaSnapshotView {
        scope: scope_view_from(&snapshot.scope, None),
        window: convert_window(snapshot.window),
        unit: convert_unit(&snapshot.unit),
        values: convert_values(snapshot.values),
        reset: convert_reset(snapshot.reset),
        confidence: convert_confidence(snapshot.confidence),
        provenance,
        served_stale,
    }
}

fn failure_view_from(failure: &quota_service::service::QuotaFailure) -> core_api::QuotaFailureView {
    core_api::QuotaFailureView {
        adapter_kind: failure.adapter_kind.map(convert_adapter_kind),
        error_code: quota_error_code(&failure.error),
        detail: format!("{}", failure.error),
        retry_after_ms: failure.error.retry_after_ms(),
    }
}

/// 把 [`quota_service::QuotaError`] 归类为稳定短码（不泄漏 detail 明文到 code）。
fn quota_error_code(error: &quota_service::QuotaError) -> String {
    use quota_service::QuotaError as E;
    match error {
        E::Unsupported { .. } => "unsupported",
        E::Unauthorized { .. } => "unauthorized",
        E::Forbidden { .. } => "forbidden",
        E::RateLimited { .. } => "rate_limited",
        E::ReauthorizationRequired { .. } => "reauthorization_required",
        E::Timeout { .. } => "timeout",
        E::Transient { .. } => "transient",
        E::Parse { .. } => "parse",
        E::Cancelled => "cancelled",
        E::Other { .. } => "other",
    }
    .into()
}

fn convert_window(window: quota_service::QuotaWindow) -> QuotaWindow {
    match window {
        quota_service::QuotaWindow::Overall => QuotaWindow::Overall,
        quota_service::QuotaWindow::Rolling5h => QuotaWindow::Rolling5h,
        quota_service::QuotaWindow::Weekly => QuotaWindow::Weekly,
        quota_service::QuotaWindow::Monthly => QuotaWindow::Monthly,
    }
}

/// core_api 窗口 → quota-service canonical 窗口（1:1 镜像）。
fn to_canonical_window(window: QuotaWindow) -> quota_service::QuotaWindow {
    match window {
        QuotaWindow::Overall => quota_service::QuotaWindow::Overall,
        QuotaWindow::Rolling5h => quota_service::QuotaWindow::Rolling5h,
        QuotaWindow::Weekly => quota_service::QuotaWindow::Weekly,
        QuotaWindow::Monthly => quota_service::QuotaWindow::Monthly,
    }
}

fn to_canonical_unit(unit: &core_api::QuotaUnit) -> quota_service::QuotaUnit {
    match unit {
        core_api::QuotaUnit::Count => quota_service::QuotaUnit::Count,
        core_api::QuotaUnit::Token => quota_service::QuotaUnit::Token,
        core_api::QuotaUnit::Cost { currency } => quota_service::QuotaUnit::Cost {
            currency: currency.clone(),
        },
    }
}

fn convert_unit(unit: &quota_service::QuotaUnit) -> core_api::QuotaUnit {
    match unit {
        quota_service::QuotaUnit::Count => core_api::QuotaUnit::Count,
        quota_service::QuotaUnit::Token => core_api::QuotaUnit::Token,
        quota_service::QuotaUnit::Cost { currency } => core_api::QuotaUnit::Cost {
            currency: currency.clone(),
        },
    }
}

fn convert_measure(measure: quota_service::QuotaMeasure) -> core_api::QuotaMeasure {
    match measure {
        quota_service::QuotaMeasure::Exact(v) => core_api::QuotaMeasure::Exact(v),
        quota_service::QuotaMeasure::Infinite => core_api::QuotaMeasure::Infinite,
        quota_service::QuotaMeasure::Unknown => core_api::QuotaMeasure::Unknown,
    }
}

fn convert_values(values: quota_service::QuotaValues) -> core_api::QuotaValues {
    core_api::QuotaValues {
        used: convert_measure(values.used),
        limit: convert_measure(values.limit),
        remaining: convert_measure(values.remaining),
    }
}

fn convert_confidence(confidence: quota_service::Confidence) -> core_api::QuotaConfidence {
    match confidence {
        quota_service::Confidence::Exact => core_api::QuotaConfidence::Exact,
        quota_service::Confidence::Derived => core_api::QuotaConfidence::Derived,
        quota_service::Confidence::Scraped => core_api::QuotaConfidence::Scraped,
    }
}

fn convert_adapter_kind(kind: quota_service::AdapterKind) -> core_api::QuotaAdapterKind {
    match kind {
        quota_service::AdapterKind::ApiKeyApi => core_api::QuotaAdapterKind::ApiKeyApi,
        quota_service::AdapterKind::OAuthApi => core_api::QuotaAdapterKind::OAuthApi,
        quota_service::AdapterKind::WebScrape => core_api::QuotaAdapterKind::WebScrape,
        quota_service::AdapterKind::LocalLedger => core_api::QuotaAdapterKind::LocalLedger,
    }
}

fn convert_reset(reset: quota_service::QuotaReset) -> core_api::QuotaReset {
    match reset {
        quota_service::QuotaReset::Absolute { at, uncertain } => {
            core_api::QuotaReset::Absolute { at, uncertain }
        }
        quota_service::QuotaReset::Relative {
            after_secs,
            observed_at,
            uncertain,
        } => core_api::QuotaReset::Relative {
            after_secs,
            observed_at,
            uncertain,
        },
        quota_service::QuotaReset::Unknown => core_api::QuotaReset::Unknown,
    }
}

fn convert_provenance(
    provenance: &quota_service::QuotaProvenance,
) -> core_api::QuotaProvenanceView {
    core_api::QuotaProvenanceView {
        adapter_kind: convert_adapter_kind(provenance.adapter_kind),
        source: provenance.source.clone(),
        endpoint: provenance.endpoint.clone(),
        fetched_at: provenance.fetched_at,
        observed_at: provenance.observed_at,
        stale: provenance.stale,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_domain::Timestamp;

    #[test]
    fn core_to_canonical_window_maps_all_variants() {
        let cases = [
            (
                core_api::QuotaWindow::Overall,
                quota_service::QuotaWindow::Overall,
            ),
            (
                core_api::QuotaWindow::Rolling5h,
                quota_service::QuotaWindow::Rolling5h,
            ),
            (
                core_api::QuotaWindow::Weekly,
                quota_service::QuotaWindow::Weekly,
            ),
            (
                core_api::QuotaWindow::Monthly,
                quota_service::QuotaWindow::Monthly,
            ),
        ];
        for (core, canonical) in cases {
            assert_eq!(to_canonical_window(core), canonical);
            assert_eq!(convert_window(canonical), core);
        }
    }

    #[test]
    fn core_to_canonical_unit_maps_all_variants_including_cost_currency() {
        let cases = [
            (core_api::QuotaUnit::Count, quota_service::QuotaUnit::Count),
            (core_api::QuotaUnit::Token, quota_service::QuotaUnit::Token),
            (
                core_api::QuotaUnit::Cost {
                    currency: "USD".into(),
                },
                quota_service::QuotaUnit::Cost {
                    currency: "USD".into(),
                },
            ),
            (
                core_api::QuotaUnit::Cost {
                    currency: "CNY".into(),
                },
                quota_service::QuotaUnit::Cost {
                    currency: "CNY".into(),
                },
            ),
        ];
        for (core, canonical) in cases {
            assert_eq!(to_canonical_unit(&core), canonical);
            assert_eq!(convert_unit(&canonical), core);
        }
    }

    #[test]
    fn cache_window_read_snapshot_flattens_hit_stale_no_data() {
        // 压平视图语义：Hit → (snapshot, false)，Stale → (snapshot, true)，
        // NoData → None。quota-service 压平 cache 结果后只改 helper 与构造。
        let snapshot = quota_service::QuotaSnapshot {
            scope: quota_service::QuotaScope::new(
                TenantId::new(core_api::DEFAULT_QUOTA_TENANT),
                quota_service::AccountId::new(core_api::DEFAULT_QUOTA_ACCOUNT),
                ProviderId::from("mock"),
                None,
            ),
            window: quota_service::QuotaWindow::Weekly,
            unit: quota_service::QuotaUnit::Token,
            values: quota_service::QuotaValues {
                used: quota_service::QuotaMeasure::Exact(1),
                limit: quota_service::QuotaMeasure::Exact(10),
                remaining: quota_service::QuotaMeasure::Exact(9),
            },
            reset: quota_service::QuotaReset::Unknown,
            confidence: quota_service::Confidence::Exact,
            provenance: quota_service::QuotaProvenance::new(
                quota_service::AdapterKind::ApiKeyApi,
                "mock-source",
                Timestamp::from_unix_millis(1),
            ),
        };
        let hit = quota_service::service::CacheRead::Hit {
            snapshot: snapshot.clone(),
        };
        let (read, stale) = cache_window_read_snapshot(&hit).expect("hit snapshot");
        assert_eq!(read.window, quota_service::QuotaWindow::Weekly);
        assert!(!stale, "fresh hit must not be stale");

        let stale_read = quota_service::service::CacheRead::Stale {
            snapshot: snapshot.clone(),
        };
        let (read, stale) = cache_window_read_snapshot(&stale_read).expect("stale snapshot");
        assert_eq!(read.window, quota_service::QuotaWindow::Weekly);
        assert!(stale, "stale read must be marked stale");

        let no_data = quota_service::service::CacheRead::NoData;
        assert_eq!(cache_window_read_snapshot(&no_data), None);
    }
}
