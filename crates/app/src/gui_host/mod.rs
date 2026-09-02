//! GUI Host 端口适配：把 AppCore 装配到 `pawork-app::gui_server::GuiHost`。
//!
//! S10 10b：Snapshot 基线、SessionGet 分页 Timeline、SessionCreate/Fork、
//! RunStart/RunCancel/ToolApprove、Terminal*、RunStart.model 切换。
//! 未支持命令一律结构化 fail-closed。

use std::collections::HashMap;
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex};

use crate::gui_server::{GuiHost, GuiHostError};
use async_trait::async_trait;
use futures::future::BoxFuture;
use pawork_domain::{CommandId, QueryId, SessionId, TenantId, WorkspaceId};
use pawork_engine::now_timestamp;
use pawork_exec::PtyService;
use pawork_protocol::{
    AppCommand, AppCommandEnvelope, AppEvent, AppEventEnvelope, AppQueryEnvelope, AppResponse,
    AppResponseEnvelope, GlobalSequence, Snapshot, SnapshotSection, SnapshotSectionKind,
    TimelinePage, DEFAULT_CONTROL_PLANE_TENANT,
};
use pawork_storage::session::{SessionRecord, SessionTree};

#[cfg(test)]
use pawork_domain::{AgentEvent, AgentEventEnvelope};
#[cfg(test)]
use pawork_engine::{AgentEventSink, EngineError};
use pawork_protocol::app::registry::{command_wire_name, query_wire_name};
#[cfg(test)]
use pawork_protocol::API_VERSION;
use serde_json::{json, Value};

use crate::{
    should_cache, AppCore, GuiApprovalHost, HubError, IdempotencyCheck, IdempotencyStore,
    PendingToolApproval, DEFAULT_HUB_CAPACITY, DEFAULT_IDEMPOTENCY_CAPACITY,
};

mod bus;
mod events;
mod handlers;
#[cfg(test)]
mod tests;

pub use bus::{ActiveGuiRun, GuiBroadcastSink, GuiEventBus, GuiRunRegistry};
use events::client_scope_from_source;
#[cfg(test)]
use handlers::run_start::{run_start_overview_owner, run_start_requested_provider_switch};

fn session_tree_entry(record: &SessionRecord, workspace_id: Option<WorkspaceId>) -> Value {
    let mut data = json!({
        "session_id": record.session_id,
        "title": record.title,
        "created_at_ms": record.created_at_ms,
        "updated_at_ms": record.updated_at_ms,
        "active_branch": record.active_branch,
        "archived": record.archived,
    });
    if let Some(workspace_id) = workspace_id {
        data["workspace_id"] = json!(workspace_id.as_str());
    }
    data
}

fn attach_session_branches(entry: &mut Value, tree: &SessionTree) {
    entry["branches"] = Value::Array(
        tree.branches
            .iter()
            .map(|branch| {
                json!({
                    "branch_id": branch.branch_id,
                    "parent_branch_id": branch.parent_branch_id,
                    "forked_from_event_id": branch.forked_from_event_id,
                    "head_sequence": branch.head_sequence,
                    "active": branch.active,
                })
            })
            .collect(),
    );
    if let Some(active) = tree.branches.iter().find(|branch| branch.active) {
        entry["parent_branch_id"] = json!(active.parent_branch_id);
        entry["forked_from_event_id"] = json!(active.forked_from_event_id);
        entry["active"] = json!(true);
    }
}

/// `gui_server` 模块的宿主实现。
pub struct GuiHostAdapter {
    core: Arc<tokio::sync::RwLock<AppCore>>,
    bus: Arc<GuiEventBus>,
    runs: Arc<GuiRunRegistry>,
    approvals: Arc<GuiApprovalHost>,
    waiters: IdempotencyStore,
    instance: pawork_domain::CoreInstanceId,
    next_gui_run: AtomicU64,
    next_fork: AtomicU64,
    pty: Arc<PtyService>,
    terminals: Mutex<HashMap<String, String>>,
    /// SET-2：按 provider_id 的认证单飞守卫（auth_start / auth_set_api_key /
    /// auth_cancel 共用；条目存在即 busy，Arc 身份用于安全移除自己的 flight）。
    /// SET-4：flight 值携带种类标记（api_key 验证 / OAuth 等待），auth_cancel
    /// 仅对 OAuth 等待放行（D3）。
    pub(crate) auth_flights: handlers::settings::AuthFlights,
}

impl GuiHostAdapter {
    pub fn new(core: Arc<AppCore>) -> Self {
        let approvals = Arc::new(GuiApprovalHost::new());
        Self::with_approvals(core, approvals)
    }

    pub fn with_approvals(core: Arc<AppCore>, approvals: Arc<GuiApprovalHost>) -> Self {
        let mut owned = Arc::try_unwrap(core)
            .unwrap_or_else(|_| panic!("GuiHostAdapter requires a uniquely owned AppCore Arc"));
        let mode = owned.approval_mode();
        let trusted = owned.workspace_trusted();
        owned.configure_approval(mode, trusted, approvals.clone());
        Self::from_locked(Arc::new(tokio::sync::RwLock::new(owned)), approvals)
    }

    pub fn from_locked(
        core: Arc<tokio::sync::RwLock<AppCore>>,
        approvals: Arc<GuiApprovalHost>,
    ) -> Self {
        let stamp = now_timestamp().as_unix_millis();
        let instance =
            pawork_domain::CoreInstanceId::from(format!("pawork-{stamp}-{}", std::process::id()));
        let bus = Arc::new(GuiEventBus::new(DEFAULT_HUB_CAPACITY));
        {
            let bus = Arc::clone(&bus);
            let instance = instance.clone();
            approvals.set_on_pending(move |ask| {
                let Some(session_id) = ask.session_id.clone() else {
                    return;
                };
                let reason = match ask.relative_path.as_deref() {
                    Some(path) if !path.is_empty() => {
                        format!("{} · {} · {}", ask.tool_name, path, ask.message)
                    }
                    _ => format!("{} · {}", ask.tool_name, ask.message),
                };
                bus.publish_raw(
                    instance.clone(),
                    &session_id,
                    AppEvent::ToolApprovalRequired {
                        run_id: ask.run_id.clone(),
                        tool_call_id: ask.tool_call_id.clone(),
                        reason,
                    },
                );
            });
        }
        Self {
            core,
            bus,
            runs: Arc::new(GuiRunRegistry::new()),
            approvals,
            waiters: IdempotencyStore::new(DEFAULT_IDEMPOTENCY_CAPACITY),
            instance,
            next_gui_run: AtomicU64::new(1),
            next_fork: AtomicU64::new(1),
            pty: Arc::new(PtyService::new()),
            terminals: Mutex::new(HashMap::new()),
            auth_flights: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn bus(&self) -> Arc<GuiEventBus> {
        Arc::clone(&self.bus)
    }

    pub fn runs(&self) -> Arc<GuiRunRegistry> {
        Arc::clone(&self.runs)
    }

    pub fn approvals(&self) -> Arc<GuiApprovalHost> {
        Arc::clone(&self.approvals)
    }

    pub fn core_instance_id(&self) -> pawork_domain::CoreInstanceId {
        self.instance.clone()
    }

    pub async fn session_store(
        &self,
    ) -> Result<pawork_storage::session::SessionStore, GuiHostError> {
        self.core
            .read()
            .await
            .store()
            .map(|store| store.clone())
            .map_err(Self::app_error)
    }

    pub fn pty(&self) -> Arc<PtyService> {
        Arc::clone(&self.pty)
    }

    pub async fn shutdown(self) -> Result<(), crate::AppError> {
        if let Err(error) = self.pty.shutdown().await {
            tracing::debug!(%error, "pty shutdown failed");
        }
        match Arc::try_unwrap(self.core) {
            Ok(lock) => lock.into_inner().shutdown().await,
            Err(_) => Ok(()),
        }
    }

    pub(crate) fn host_error(code: &str, message: impl Into<String>) -> GuiHostError {
        GuiHostError {
            code: code.to_string(),
            message: message.into(),
            retryable: false,
        }
    }

    pub(crate) fn app_error(error: crate::AppError) -> GuiHostError {
        let code = match error {
            crate::AppError::UnknownModel { .. }
            | crate::AppError::ModelBelongsToProvider { .. } => "unknown_model",
            _ => "app_error",
        };
        Self::host_error(code, error.to_string())
    }

    pub(crate) fn session_error(error: pawork_storage::session::SessionStoreError) -> GuiHostError {
        let code = match error {
            pawork_storage::session::SessionStoreError::SessionNotFound(_)
            | pawork_storage::session::SessionStoreError::ParentEventNotFound(_)
            | pawork_storage::session::SessionStoreError::BranchNotFound { .. } => "not_found",
            pawork_storage::session::SessionStoreError::BranchAlreadyExists { .. } => "conflict",
            _ => "app_error",
        };
        Self::host_error(code, error.to_string())
    }

    fn pty_error(error: pawork_exec::PtyError) -> GuiHostError {
        let code = match error {
            pawork_exec::PtyError::NotFound(_) => "not_found",
            pawork_exec::PtyError::Ownership(_, _) => "forbidden",
            pawork_exec::PtyError::Closed(_) => "conflict",
            _ => "app_error",
        };
        Self::host_error(code, error.to_string())
    }

    /// command() 在 dispatch 之后调用：成功响应写入 ledger，错误响应 release。
    /// record 失败不可回滚已执行命令，必须记数+打日志后仍返回响应。
    pub(crate) async fn persist_command_response(
        &self,
        ledger: &IdempotencyStore,
        tenant: &TenantId,
        command_id: &CommandId,
        idempotency_key: Option<&str>,
        cached: AppResponseEnvelope,
    ) {
        if should_cache(&cached) {
            if let Err(error) = ledger
                .record(tenant, command_id, idempotency_key, cached.clone())
                .await
            {
                self.on_command_record_failure(command_id.as_str(), &error);
                // DB 类错误（Closed / Other / StoreUnavailable）先重试一次
                // record：UPDATE WHERE status='inflight' 是幂等的。重试成功则
                // 行 completed，同 command_id 重试走 Replay 不重执行。
                // KeyConflict 语义是键已被另一命令占用，带键重试仍会 Replay
                // 键持有行；DuplicateCommand 行已 completed。这两种以及重试
                // 仍失败才 release。release 只删 inflight。已返回响应不变，
                // 不发客户端帧（波 C 决议）。
                let retry_ok = matches!(
                    error,
                    crate::IdempotencyError::Closed
                        | crate::IdempotencyError::Other(_)
                        | crate::IdempotencyError::StoreUnavailable,
                ) && ledger
                    .record(tenant, command_id, idempotency_key, cached)
                    .await
                    .is_ok();
                if !retry_ok {
                    ledger.release(tenant, command_id, idempotency_key).await;
                }
            }
        } else {
            ledger.release(tenant, command_id, idempotency_key).await;
        }
    }

    fn on_command_record_failure(&self, command_id: &str, error: &crate::IdempotencyError) {
        // IdempotencyStore::record already bumps record_failures. This helper
        // must not bump again (command() would double-count) and must not
        // swallow the error.
        tracing::error!(
            code = "degrade.idempotency_conflict",
            command_id,
            error = %error,
            "command ledger record failed after the command already executed"
        );
    }

    #[cfg(test)]
    pub(crate) async fn command_record_failure_count(&self) -> u64 {
        self.waiters.stats().await.record_failures
    }
}

#[async_trait]
impl GuiHost for GuiHostAdapter {
    fn instance_id(&self) -> pawork_domain::CoreInstanceId {
        self.instance.clone()
    }

    async fn snapshot(&self) -> Result<Snapshot, GuiHostError> {
        let core = self.core.read().await;
        let sessions = core.list_sessions().await.map_err(Self::app_error)?;
        let runs = self.runs.active();
        let provider_status = if core.provider_pending() {
            "authentication_required"
        } else {
            "ready"
        };
        // PendingToolApprovals is host-global, matching GuiApprovalHost::pending()
        // and SessionStore::waiting_tool_calls(): neither is session-scoped.
        // snapshot() has no session filter, so restart projections are merged in full.
        let mut pending = self.approvals.pending();
        if let Ok(store) = core.store() {
            if let Ok(waiting) = store.waiting_tool_calls().await {
                let live: std::collections::HashSet<_> = pending
                    .iter()
                    .map(|ask| ask.tool_call_id.as_str().to_string())
                    .collect();
                for item in waiting {
                    if live.contains(item.tool_call.tool_call_id.as_str()) {
                        continue;
                    }
                    let arguments =
                        serde_json::from_str(&item.tool_call.arguments_json).unwrap_or(Value::Null);
                    pending.push(PendingToolApproval {
                        run_id: item.tool_call.run_id.clone(),
                        session_id: Some(item.session_id.clone()),
                        tool_call_id: item.tool_call.tool_call_id.clone(),
                        tool_name: item.tool_call.name.clone(),
                        relative_path: crate::approval::relative_path_from_input(&arguments),
                        risk: pawork_policy::RiskLevel::Moderate,
                        message: "approval pending across restart".into(),
                        preview: None,
                    });
                }
            }
        }
        pending.sort_by(|a, b| {
            a.run_id
                .as_str()
                .cmp(b.run_id.as_str())
                .then_with(|| a.tool_call_id.as_str().cmp(b.tool_call_id.as_str()))
        });
        let mut session_entries = Vec::new();
        for record in &sessions {
            let mut entry = session_tree_entry(
                record,
                core.session_workspace_for_record(&record.session_id),
            );
            if let Ok(store) = core.store() {
                if let Ok(tree) = store
                    .session_tree(&SessionId::from(record.session_id.as_str()))
                    .await
                {
                    attach_session_branches(&mut entry, &tree);
                }
            }
            session_entries.push(entry);
        }
        let sections = vec![
            SnapshotSection {
                kind: SnapshotSectionKind::Workspaces,
                revision: self.bus.next_revision(),
                data: Some(Value::Array(
                    core.registered_workspaces()
                        .into_iter()
                        .map(|record| {
                            json!({
                                "id": record.workspace_id.as_str(),
                                "name": record.name,
                                "trusted": core.workspace_trusted(),
                            })
                        })
                        .collect(),
                )),
                artifact_id: None,
            },
            SnapshotSection {
                kind: SnapshotSectionKind::SessionTree,
                revision: self.bus.next_revision(),
                data: Some(Value::Array(session_entries)),
                artifact_id: None,
            },
            SnapshotSection {
                kind: SnapshotSectionKind::ActiveRuns,
                revision: self.bus.next_revision(),
                data: Some(Value::Array(
                    runs.iter()
                        .map(|run| {
                            json!({
                                "run_id": run.run_id.as_str(),
                                "session_id": run.session_id.as_str(),
                                "started_at_ms": run.started_at_ms,
                            })
                        })
                        .collect(),
                )),
                artifact_id: None,
            },
            SnapshotSection {
                kind: SnapshotSectionKind::PendingToolApprovals,
                revision: self.bus.next_revision(),
                data: Some(Value::Array(
                    pending
                        .iter()
                        .map(|ask| {
                            json!({
                                "run_id": ask.run_id.as_str(),
                                "session_id": ask.session_id.as_ref().map(|id| id.as_str()),
                                "tool_call_id": ask.tool_call_id.as_str(),
                                "tool_name": ask.tool_name,
                                "relative_path": ask.relative_path,
                                "risk": format!("{:?}", ask.risk).to_ascii_lowercase(),
                                "message": ask.message,
                                "preview": ask.preview,
                            })
                        })
                        .collect(),
                )),
                artifact_id: None,
            },
            SnapshotSection {
                kind: SnapshotSectionKind::TerminalSessions,
                revision: self.bus.next_revision(),
                data: Some(Value::Array(self.terminal_snapshots())),
                artifact_id: None,
            },
            SnapshotSection {
                kind: SnapshotSectionKind::ProviderStatus,
                revision: self.bus.next_revision(),
                data: Some(json!([{
                    "provider_id": core.provider_id().as_str(),
                    "model": core.model().as_str(),
                    "status": provider_status,
                }])),
                artifact_id: None,
            },
        ];
        Ok(Snapshot {
            instance_id: self.instance.clone(),
            snapshot_sequence: GlobalSequence(self.bus.current_sequence()),
            generated_at: now_timestamp(),
            sections,
        })
    }

    async fn timeline(
        &self,
        session_id: &SessionId,
        after: Option<u64>,
        limit: Option<u32>,
    ) -> Result<TimelinePage, GuiHostError> {
        const DEFAULT_LIMIT: u32 = 200;
        const MAX_LIMIT: u32 = 500;
        let limit = limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT) as usize;
        let core = self.core.read().await;
        let head = core
            .next_sequence(session_id)
            .await
            .map_err(Self::app_error)?
            .saturating_sub(1);
        let from = after.map(|value| value.saturating_add(1)).unwrap_or(1);
        if from > head {
            return Ok(TimelinePage {
                items: Vec::new(),
                next_sequence: None,
                head_sequence: head,
                complete: true,
            });
        }
        let store = core.store().map_err(Self::app_error)?;
        let active_branch = store
            .get_session(session_id)
            .await
            .map_err(|error| Self::host_error("app_error", error.to_string()))?
            .active_branch;
        let envelopes = store
            .events_on_lineage(session_id, &active_branch, from, limit)
            .await
            .map_err(|error| Self::host_error("app_error", error.to_string()))?;
        let items: Vec<_> = envelopes.iter().filter_map(project_timeline_item).collect();
        // 分页窗口和游标属于持久化事件序列，而非过滤后的 presentation items。
        // ContextPrepared/UsageUpdated 等无 TimelineItem 的事件仍必须推进游标；
        // 否则整页不可投影时会被误报 complete，历史在首个空页即被截断。
        let page_cursor = envelopes.last().map(|envelope| envelope.sequence.0);
        let complete =
            (envelopes.len() < limit) || page_cursor.is_some_and(|sequence| sequence >= head);
        let next_sequence = if complete { None } else { page_cursor };
        Ok(TimelinePage {
            items,
            next_sequence,
            head_sequence: head,
            complete,
        })
    }

    async fn query(&self, envelope: &AppQueryEnvelope) -> Result<AppResponse, GuiHostError> {
        let Some((_, handler)) = QUERY_HANDLERS
            .iter()
            .find(|(wire_name, _)| *wire_name == query_wire_name(&envelope.query))
        else {
            return Err(Self::host_error(
                "unsupported",
                format!(
                    "query {} is not part of the S7 wave A slice",
                    query_wire_name(&envelope.query)
                ),
            ));
        };
        handler(self, &envelope.query).await
    }

    async fn command(&self, envelope: &AppCommandEnvelope) -> Result<AppResponse, GuiHostError> {
        let tenant = TenantId::new(DEFAULT_CONTROL_PLANE_TENANT);
        let command_id = envelope.command_id.clone();
        let idempotency_key = envelope.idempotency_key.clone();
        let scope = client_scope_from_source(&envelope.source);
        let mut ledger = {
            let core = self.core.read().await;
            let store = core
                .store()
                .map_err(|_| Self::host_error("store_unavailable", "session store is not open"))?;
            IdempotencyStore::for_store(store.command_ledger()).with_scope(scope)
        };
        // 进程内 Notify 必须跨 command() 共享；SQLite 仍是权威 CAS。
        ledger.share_waiters_from(&self.waiters);
        loop {
            match ledger
                .check(&tenant, &command_id, idempotency_key.as_deref())
                .await
            {
                Ok(IdempotencyCheck::Replay(cached)) => return Ok(cached.response),
                Ok(IdempotencyCheck::InFlight(notify)) => {
                    // Hazard 1: 同 idempotency_key、不同 command_id 的占位会让
                    // check 返回 InFlight，但 waiter 按调用方自己的 command_id
                    // 注册；record/release 只唤醒记录方 command_id。
                    // Hazard 2: notify_waiters 不存 permit，check 返回与
                    // notified() 被 poll 之间的窗口可能丢唤醒。
                    // SQLite 是权威 CAS；有界等待后回到 loop 重查，轮询兜底。
                    let notified = notify.notified();
                    tokio::pin!(notified);
                    tokio::select! {
                        _ = &mut notified => {}
                        _ = tokio::time::sleep(std::time::Duration::from_millis(50)) => {}
                    }
                }
                Ok(IdempotencyCheck::New) => break,
                Err(error) => {
                    return Err(Self::host_error("idempotency", error.to_string()));
                }
            }
        }
        let response = match self.dispatch_command(envelope).await {
            Ok(response) => response,
            Err(error) => {
                ledger
                    .release(&tenant, &command_id, idempotency_key.as_deref())
                    .await;
                return Err(error);
            }
        };
        let cached = AppResponseEnvelope {
            api_version: envelope.api_version,
            request_id: QueryId::from(envelope.command_id.as_str()),
            responded_at: now_timestamp(),
            response: response.clone(),
        };
        self.persist_command_response(
            &ledger,
            &tenant,
            &command_id,
            idempotency_key.as_deref(),
            cached,
        )
        .await;
        Ok(response)
    }

    fn subscribe_events(&self) -> tokio::sync::broadcast::Receiver<AppEventEnvelope> {
        self.bus.subscribe()
    }

    fn current_sequence(&self) -> GlobalSequence {
        GlobalSequence(self.bus.current_sequence())
    }

    fn earliest_available(&self) -> Option<GlobalSequence> {
        self.bus.hub().earliest_available()
    }

    fn replay(
        &self,
        from: GlobalSequence,
        through: Option<GlobalSequence>,
    ) -> Result<Vec<AppEventEnvelope>, GuiHostError> {
        self.bus.replay(from, through).map_err(|error| match error {
            HubError::ReplayUnavailable {
                requested_from,
                earliest_available,
            } => Self::host_error(
                "replay_unavailable",
                format!(
                    "replay from {} unavailable; earliest is {}",
                    requested_from.0, earliest_available.0
                ),
            ),
            other => Self::host_error("internal", other.to_string()),
        })
    }

    fn publish_event_stream_lagged(
        &self,
        missed: Option<u64>,
        client_id: Option<&str>,
    ) -> Option<AppEventEnvelope> {
        Some(
            self.bus
                .publish_event_stream_lagged(self.instance.clone(), missed, client_id)
                .0,
        )
    }
}

type QueryHandler = for<'a> fn(
    &'a GuiHostAdapter,
    &'a pawork_protocol::AppQuery,
) -> BoxFuture<'a, Result<AppResponse, GuiHostError>>;

type CommandHandler = for<'a> fn(
    &'a GuiHostAdapter,
    &'a AppCommandEnvelope,
    &'a AppCommand,
) -> BoxFuture<'a, Result<AppResponse, GuiHostError>>;

fn query_workspace_list<'a>(
    adapter: &'a GuiHostAdapter,
    query: &'a pawork_protocol::AppQuery,
) -> BoxFuture<'a, Result<AppResponse, GuiHostError>> {
    Box::pin(handlers::query::workspace_list(adapter, query))
}

fn query_session_get<'a>(
    adapter: &'a GuiHostAdapter,
    query: &'a pawork_protocol::AppQuery,
) -> BoxFuture<'a, Result<AppResponse, GuiHostError>> {
    Box::pin(handlers::query::session_get(adapter, query))
}

fn query_run_status<'a>(
    adapter: &'a GuiHostAdapter,
    query: &'a pawork_protocol::AppQuery,
) -> BoxFuture<'a, Result<AppResponse, GuiHostError>> {
    Box::pin(handlers::query::run_status(adapter, query))
}

fn query_model_list<'a>(
    adapter: &'a GuiHostAdapter,
    query: &'a pawork_protocol::AppQuery,
) -> BoxFuture<'a, Result<AppResponse, GuiHostError>> {
    Box::pin(handlers::query::model_list(adapter, query))
}

fn query_diff_list_files<'a>(
    adapter: &'a GuiHostAdapter,
    query: &'a pawork_protocol::AppQuery,
) -> BoxFuture<'a, Result<AppResponse, GuiHostError>> {
    Box::pin(handlers::query::diff_list_files(adapter, query))
}

fn query_diff_get<'a>(
    adapter: &'a GuiHostAdapter,
    query: &'a pawork_protocol::AppQuery,
) -> BoxFuture<'a, Result<AppResponse, GuiHostError>> {
    Box::pin(handlers::query::diff_get(adapter, query))
}

fn query_quota_overview<'a>(
    adapter: &'a GuiHostAdapter,
    query: &'a pawork_protocol::AppQuery,
) -> BoxFuture<'a, Result<AppResponse, GuiHostError>> {
    Box::pin(handlers::query::quota_overview(adapter, query))
}

fn query_mcp_list<'a>(
    adapter: &'a GuiHostAdapter,
    query: &'a pawork_protocol::AppQuery,
) -> BoxFuture<'a, Result<AppResponse, GuiHostError>> {
    Box::pin(handlers::query::mcp_list(adapter, query))
}

fn query_provider_auth_status<'a>(
    adapter: &'a GuiHostAdapter,
    query: &'a pawork_protocol::AppQuery,
) -> BoxFuture<'a, Result<AppResponse, GuiHostError>> {
    Box::pin(handlers::settings::provider_auth_status(adapter, query))
}

fn query_general_settings<'a>(
    adapter: &'a GuiHostAdapter,
    query: &'a pawork_protocol::AppQuery,
) -> BoxFuture<'a, Result<AppResponse, GuiHostError>> {
    Box::pin(handlers::settings::general_settings(adapter, query))
}

fn query_permissions_settings<'a>(
    adapter: &'a GuiHostAdapter,
    query: &'a pawork_protocol::AppQuery,
) -> BoxFuture<'a, Result<AppResponse, GuiHostError>> {
    Box::pin(handlers::settings::permissions_settings(adapter, query))
}

fn command_workspace_add<'a>(
    adapter: &'a GuiHostAdapter,
    envelope: &'a AppCommandEnvelope,
    command: &'a AppCommand,
) -> BoxFuture<'a, Result<AppResponse, GuiHostError>> {
    Box::pin(handlers::command::workspace_add(adapter, envelope, command))
}

fn command_workspace_trust<'a>(
    adapter: &'a GuiHostAdapter,
    envelope: &'a AppCommandEnvelope,
    command: &'a AppCommand,
) -> BoxFuture<'a, Result<AppResponse, GuiHostError>> {
    Box::pin(handlers::settings::workspace_trust(
        adapter, envelope, command,
    ))
}

fn command_session_create<'a>(
    adapter: &'a GuiHostAdapter,
    envelope: &'a AppCommandEnvelope,
    command: &'a AppCommand,
) -> BoxFuture<'a, Result<AppResponse, GuiHostError>> {
    Box::pin(handlers::session::session_create(
        adapter, envelope, command,
    ))
}

fn command_session_open<'a>(
    adapter: &'a GuiHostAdapter,
    envelope: &'a AppCommandEnvelope,
    command: &'a AppCommand,
) -> BoxFuture<'a, Result<AppResponse, GuiHostError>> {
    Box::pin(handlers::session::session_open(adapter, envelope, command))
}

fn command_session_fork<'a>(
    adapter: &'a GuiHostAdapter,
    envelope: &'a AppCommandEnvelope,
    command: &'a AppCommand,
) -> BoxFuture<'a, Result<AppResponse, GuiHostError>> {
    Box::pin(handlers::session::session_fork(adapter, envelope, command))
}

fn command_run_start<'a>(
    adapter: &'a GuiHostAdapter,
    envelope: &'a AppCommandEnvelope,
    command: &'a AppCommand,
) -> BoxFuture<'a, Result<AppResponse, GuiHostError>> {
    Box::pin(handlers::run_start::run_start(adapter, envelope, command))
}

fn command_run_cancel<'a>(
    adapter: &'a GuiHostAdapter,
    envelope: &'a AppCommandEnvelope,
    command: &'a AppCommand,
) -> BoxFuture<'a, Result<AppResponse, GuiHostError>> {
    Box::pin(handlers::command::run_cancel(adapter, envelope, command))
}

fn command_auth_start<'a>(
    adapter: &'a GuiHostAdapter,
    envelope: &'a AppCommandEnvelope,
    command: &'a AppCommand,
) -> BoxFuture<'a, Result<AppResponse, GuiHostError>> {
    Box::pin(handlers::settings::auth_start(adapter, envelope, command))
}

fn command_auth_remove<'a>(
    adapter: &'a GuiHostAdapter,
    envelope: &'a AppCommandEnvelope,
    command: &'a AppCommand,
) -> BoxFuture<'a, Result<AppResponse, GuiHostError>> {
    Box::pin(handlers::settings::auth_remove(adapter, envelope, command))
}

fn command_auth_set_api_key<'a>(
    adapter: &'a GuiHostAdapter,
    envelope: &'a AppCommandEnvelope,
    command: &'a AppCommand,
) -> BoxFuture<'a, Result<AppResponse, GuiHostError>> {
    Box::pin(handlers::settings::auth_set_api_key(
        adapter, envelope, command,
    ))
}

fn command_auth_cancel<'a>(
    adapter: &'a GuiHostAdapter,
    envelope: &'a AppCommandEnvelope,
    command: &'a AppCommand,
) -> BoxFuture<'a, Result<AppResponse, GuiHostError>> {
    Box::pin(handlers::settings::auth_cancel(adapter, envelope, command))
}

fn command_set_default_model<'a>(
    adapter: &'a GuiHostAdapter,
    envelope: &'a AppCommandEnvelope,
    command: &'a AppCommand,
) -> BoxFuture<'a, Result<AppResponse, GuiHostError>> {
    Box::pin(handlers::settings::set_default_model(
        adapter, envelope, command,
    ))
}

fn command_set_proxy_url<'a>(
    adapter: &'a GuiHostAdapter,
    envelope: &'a AppCommandEnvelope,
    command: &'a AppCommand,
) -> BoxFuture<'a, Result<AppResponse, GuiHostError>> {
    Box::pin(handlers::settings::set_proxy_url(
        adapter, envelope, command,
    ))
}

fn command_set_approval_mode<'a>(
    adapter: &'a GuiHostAdapter,
    envelope: &'a AppCommandEnvelope,
    command: &'a AppCommand,
) -> BoxFuture<'a, Result<AppResponse, GuiHostError>> {
    Box::pin(handlers::settings::set_approval_mode(
        adapter, envelope, command,
    ))
}

fn command_tool_approve<'a>(
    adapter: &'a GuiHostAdapter,
    envelope: &'a AppCommandEnvelope,
    command: &'a AppCommand,
) -> BoxFuture<'a, Result<AppResponse, GuiHostError>> {
    Box::pin(handlers::approval::tool_approve(adapter, envelope, command))
}

fn command_terminal_create<'a>(
    adapter: &'a GuiHostAdapter,
    envelope: &'a AppCommandEnvelope,
    command: &'a AppCommand,
) -> BoxFuture<'a, Result<AppResponse, GuiHostError>> {
    Box::pin(handlers::terminal::terminal_create(
        adapter, envelope, command,
    ))
}

fn command_terminal_write<'a>(
    adapter: &'a GuiHostAdapter,
    envelope: &'a AppCommandEnvelope,
    command: &'a AppCommand,
) -> BoxFuture<'a, Result<AppResponse, GuiHostError>> {
    Box::pin(handlers::terminal::terminal_write(
        adapter, envelope, command,
    ))
}

fn command_terminal_resize<'a>(
    adapter: &'a GuiHostAdapter,
    envelope: &'a AppCommandEnvelope,
    command: &'a AppCommand,
) -> BoxFuture<'a, Result<AppResponse, GuiHostError>> {
    Box::pin(handlers::terminal::terminal_resize(
        adapter, envelope, command,
    ))
}

fn command_terminal_close<'a>(
    adapter: &'a GuiHostAdapter,
    envelope: &'a AppCommandEnvelope,
    command: &'a AppCommand,
) -> BoxFuture<'a, Result<AppResponse, GuiHostError>> {
    Box::pin(handlers::terminal::terminal_close(
        adapter, envelope, command,
    ))
}

static QUERY_HANDLERS: &[(&str, QueryHandler)] = &[
    ("workspace_list", query_workspace_list),
    ("session_get", query_session_get),
    ("run_status", query_run_status),
    ("model_list", query_model_list),
    ("diff_list_files", query_diff_list_files),
    ("diff_get", query_diff_get),
    ("quota_overview", query_quota_overview),
    ("mcp_list", query_mcp_list),
    ("provider_auth_status", query_provider_auth_status),
    ("general_settings", query_general_settings),
    ("permissions_settings", query_permissions_settings),
];

static COMMAND_HANDLERS: &[(&str, CommandHandler)] = &[
    ("workspace_add", command_workspace_add),
    ("workspace_trust", command_workspace_trust),
    ("session_create", command_session_create),
    ("session_open", command_session_open),
    ("session_fork", command_session_fork),
    ("run_start", command_run_start),
    ("run_cancel", command_run_cancel),
    ("auth_start", command_auth_start),
    ("auth_remove", command_auth_remove),
    ("auth_set_api_key", command_auth_set_api_key),
    ("auth_cancel", command_auth_cancel),
    ("set_default_model", command_set_default_model),
    ("set_proxy_url", command_set_proxy_url),
    ("set_approval_mode", command_set_approval_mode),
    ("tool_approve", command_tool_approve),
    ("terminal_create", command_terminal_create),
    ("terminal_write", command_terminal_write),
    ("terminal_resize", command_terminal_resize),
    ("terminal_close", command_terminal_close),
];

impl GuiHostAdapter {
    async fn dispatch_command(
        &self,
        envelope: &AppCommandEnvelope,
    ) -> Result<AppResponse, GuiHostError> {
        let Some((_, handler)) = COMMAND_HANDLERS
            .iter()
            .find(|(wire_name, _)| *wire_name == command_wire_name(&envelope.command))
        else {
            return Err(Self::host_error(
                "unsupported",
                format!(
                    "command {} is not supported",
                    command_wire_name(&envelope.command)
                ),
            ));
        };
        handler(self, envelope, &envelope.command).await
    }
}

// Timeline 条目映射已下沉 pawork-protocol::projection（R3 波 C）；保留原名
// 供 app 内 timeline() 与既有 re-export 消费，wire 形状不变。
pub use pawork_protocol::projection::project_event as project_timeline_item;
