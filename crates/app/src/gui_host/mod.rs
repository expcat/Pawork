//! GUI Host 端口适配：把 AppCore 装配到 `pawork-app::gui_server::GuiHost`。
//!
//! S10 10b：Snapshot 基线、SessionGet 分页 Timeline、SessionCreate/Fork、
//! RunStart/RunCancel/ToolApprove、Terminal*、RunStart.model 切换。
//! 未支持命令一律结构化 fail-closed。

use std::collections::HashMap;
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use futures::future::BoxFuture;
use pawork_domain::{
    QueryId, SessionId, TenantId, WorkspaceId,
};
use pawork_exec::PtyService;
use pawork_storage::session::{SessionRecord, SessionTree};
use pawork_engine::now_timestamp;
use crate::gui_server::{GuiHost, GuiHostError};
use pawork_protocol::{
    AppCommand, AppCommandEnvelope, AppEvent, AppEventEnvelope, AppQueryEnvelope, AppResponse,
    AppResponseEnvelope, GlobalSequence, Snapshot, SnapshotSection, SnapshotSectionKind,
    TimelinePage, DEFAULT_CONTROL_PLANE_TENANT,
};

#[cfg(test)]
use pawork_domain::{AgentEvent, AgentEventEnvelope};
#[cfg(test)]
use pawork_engine::{AgentEventSink, EngineError};
#[cfg(test)]
use pawork_protocol::API_VERSION;
use serde_json::{json, Value};
use pawork_protocol::app::registry::{command_wire_name, query_wire_name};

use crate::{
    should_cache, AppCore, GuiApprovalHost, HubError, IdempotencyCheck, IdempotencyStore,
    DEFAULT_HUB_CAPACITY,
};

mod bus;
mod events;
mod handlers;
#[cfg(test)]
mod tests;

pub use bus::{ActiveGuiRun, GuiBroadcastSink, GuiEventBus, GuiRunRegistry};
#[cfg(test)]
use handlers::run_start::{run_start_overview_owner, run_start_requested_provider_switch};
use events::scoped_idempotency;


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
    idempotency: IdempotencyStore,
    instance: pawork_domain::CoreInstanceId,
    next_gui_run: AtomicU64,
    next_fork: AtomicU64,
    pty: Arc<PtyService>,
    terminals: Mutex<HashMap<String, String>>,
}

impl GuiHostAdapter {
    pub fn new(core: Arc<AppCore>) -> Self {
        let approvals = Arc::new(GuiApprovalHost::new());
        Self::with_approvals(core, approvals)
    }

    pub fn with_approvals(core: Arc<AppCore>, approvals: Arc<GuiApprovalHost>) -> Self {
        let mut owned = Arc::try_unwrap(core).unwrap_or_else(|_| {
            panic!("GuiHostAdapter requires a uniquely owned AppCore Arc")
        });
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
        let instance = pawork_domain::CoreInstanceId::from(format!(
            "pawork-{stamp}-{}",
            std::process::id()
        ));
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
            idempotency: IdempotencyStore::default(),
            instance,
            next_gui_run: AtomicU64::new(1),
            next_fork: AtomicU64::new(1),
            pty: Arc::new(PtyService::new()),
            terminals: Mutex::new(HashMap::new()),
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

    pub async fn session_store(&self) -> Result<pawork_storage::session::SessionStore, GuiHostError> {
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
        let _ = self.pty.shutdown().await;
        match Arc::try_unwrap(self.core) {
            Ok(lock) => lock.into_inner().shutdown().await,
            Err(_) => Ok(()),
        }
    }

    fn host_error(code: &str, message: impl Into<String>) -> GuiHostError {
        GuiHostError {
            code: code.to_string(),
            message: message.into(),
            retryable: false,
        }
    }

    fn app_error(error: crate::AppError) -> GuiHostError {
        let code = match error {
            crate::AppError::UnknownModel { .. }
            | crate::AppError::ModelBelongsToProvider { .. } => "unknown_model",
            _ => "app_error",
        };
        Self::host_error(code, error.to_string())
    }

    fn session_error(error: pawork_storage::session::SessionStoreError) -> GuiHostError {
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
        let pending = self.approvals.pending();
        let mut session_entries = Vec::new();
        for record in &sessions {
            let mut entry =
                session_tree_entry(record, core.session_workspace_for_record(&record.session_id));
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
                data: Some(json!([{
                    "id": core.workspace_id().as_str(),
                    "name": core.workspace_name(),
                    "trusted": core.workspace_trusted(),
                }])),
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
        let (command_id, idempotency_key) = scoped_idempotency(envelope);
        loop {
            match self
                .idempotency
                .check(&tenant, &command_id, idempotency_key.as_deref())
            {
                IdempotencyCheck::Replay(cached) => return Ok(cached.response),
                IdempotencyCheck::InFlight(notify) => notify.notified().await,
                IdempotencyCheck::New => break,
            }
        }
        let response = match self.dispatch_command(envelope).await {
            Ok(response) => response,
            Err(error) => {
                self.idempotency
                    .release(&tenant, &command_id, idempotency_key.as_deref());
                return Err(error);
            }
        };
        let cached = AppResponseEnvelope {
            api_version: envelope.api_version,
            request_id: QueryId::from(envelope.command_id.as_str()),
            responded_at: now_timestamp(),
            response: response.clone(),
        };
        if should_cache(&cached) {
            let _ = self.idempotency.record(
                &tenant,
                &command_id,
                idempotency_key.as_deref(),
                cached,
            );
        } else {
            self.idempotency
                .release(&tenant, &command_id, idempotency_key.as_deref());
        }
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

fn command_workspace_add<'a>(
    adapter: &'a GuiHostAdapter,
    envelope: &'a AppCommandEnvelope,
    command: &'a AppCommand,
) -> BoxFuture<'a, Result<AppResponse, GuiHostError>> {
    Box::pin(handlers::command::workspace_add(adapter, envelope, command))
}

fn command_session_create<'a>(
    adapter: &'a GuiHostAdapter,
    envelope: &'a AppCommandEnvelope,
    command: &'a AppCommand,
) -> BoxFuture<'a, Result<AppResponse, GuiHostError>> {
    Box::pin(handlers::session::session_create(adapter, envelope, command))
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
    Box::pin(handlers::terminal::terminal_create(adapter, envelope, command))
}

fn command_terminal_write<'a>(
    adapter: &'a GuiHostAdapter,
    envelope: &'a AppCommandEnvelope,
    command: &'a AppCommand,
) -> BoxFuture<'a, Result<AppResponse, GuiHostError>> {
    Box::pin(handlers::terminal::terminal_write(adapter, envelope, command))
}

fn command_terminal_resize<'a>(
    adapter: &'a GuiHostAdapter,
    envelope: &'a AppCommandEnvelope,
    command: &'a AppCommand,
) -> BoxFuture<'a, Result<AppResponse, GuiHostError>> {
    Box::pin(handlers::terminal::terminal_resize(adapter, envelope, command))
}

static QUERY_HANDLERS: &[(&str, QueryHandler)] = &[
    ("workspace_list", query_workspace_list),
    ("session_get", query_session_get),
    ("run_status", query_run_status),
    ("model_list", query_model_list),
    ("diff_list_files", query_diff_list_files),
    ("diff_get", query_diff_get),
    ("quota_overview", query_quota_overview),
];

static COMMAND_HANDLERS: &[(&str, CommandHandler)] = &[
    ("workspace_add", command_workspace_add),
    ("session_create", command_session_create),
    ("session_open", command_session_open),
    ("session_fork", command_session_fork),
    ("run_start", command_run_start),
    ("run_cancel", command_run_cancel),
    ("tool_approve", command_tool_approve),
    ("terminal_create", command_terminal_create),
    ("terminal_write", command_terminal_write),
    ("terminal_resize", command_terminal_resize),
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
