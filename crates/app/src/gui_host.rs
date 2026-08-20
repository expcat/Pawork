//! GUI Host 端口适配：把 AppCore 装配到 `pawork-app::gui_server::GuiHost`。
//!
//! S10 10b：Snapshot 基线、SessionGet 分页 Timeline、SessionCreate/Fork、
//! RunStart/RunCancel/ToolApprove、Terminal*、RunStart.model 切换。
//! 未支持命令一律结构化 fail-closed。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use pawork_domain::{
    AgentEvent, AgentEventEnvelope, ApprovalDecision, CancellationToken, CommandId, ContentPart,
    ErrorCategory, ErrorContext, EventId, Message, MessageId, MessageRole, QueryId, RunId,
    SessionId, TenantId, TextContent, WorkspaceId,
};
use pawork_exec::{OwnerSessionId, PtyCreateSpec, PtyEvent, PtyService, PtyWindowSize, TerminalId};
use pawork_storage::session::{SessionRecord, SessionTree};
use pawork_engine::{now_timestamp, AgentEventSink, EngineError};
use crate::gui_server::{GuiHost, GuiHostError};
use pawork_protocol::{
    AppCommand, AppCommandEnvelope, AppEvent, AppEventEnvelope, AppQuery, AppQueryEnvelope,
    AppResponse, AppResponseEnvelope, CommandSource, DiagnosticLevel, EventSource, EventStream,
    GlobalSequence, RunState, Snapshot, SnapshotSection, SnapshotSectionKind, TimelineItem,
    TimelineItemKind, TimelinePage, WorkspaceRelativePath, API_VERSION,
    DEFAULT_CONTROL_PLANE_TENANT,
};
use pawork_workspace::resolve_relative_path;
use serde_json::{json, Value};
use pawork_protocol::app::registry::{command_wire_name, query_wire_name};

use crate::{
    should_cache, AppCore, EventHub, GuiApprovalHost, HubError, IdempotencyCheck, IdempotencyStore,
    DEFAULT_HUB_CAPACITY,
};

fn session_view_json(
    session_id: &SessionId,
    workspace_id: &WorkspaceId,
    title: &str,
    active_branch: Option<&str>,
) -> Value {
    let mut data = json!({
        "session_id": session_id.as_str(),
        "workspace_id": workspace_id.as_str(),
        "title": title,
        "revision": 0,
        "open": true,
    });
    if let Some(branch) = active_branch {
        data["active_branch"] = json!(branch);
    }
    data
}

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

fn protocol_to_domain_decision(
    decision: &pawork_protocol::ApprovalDecision,
) -> pawork_domain::ApprovalDecision {
    match decision {
        pawork_protocol::ApprovalDecision::ApproveOnce => ApprovalDecision::ApprovedOnce,
        pawork_protocol::ApprovalDecision::ApproveForRun => ApprovalDecision::ApprovedForRun,
        pawork_protocol::ApprovalDecision::Deny => ApprovalDecision::Denied,
        pawork_protocol::ApprovalDecision::Cancel => ApprovalDecision::Cancelled,
    }
}

/// 单实例事件总线：内部唯一 EventHub，组信封后 `publish`，容量默认 4096。
pub struct GuiEventBus {
    hub: EventHub,
    revision: AtomicU64,
    next_event: AtomicU64,
}

impl GuiEventBus {
    pub fn new(capacity: usize) -> Self {
        Self {
            hub: EventHub::with_capacity(capacity),
            revision: AtomicU64::new(1),
            next_event: AtomicU64::new(1),
        }
    }

    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<AppEventEnvelope> {
        self.hub.subscribe_receiver()
    }

    pub fn hub(&self) -> &EventHub {
        &self.hub
    }

    pub fn replay(
        &self,
        from: GlobalSequence,
        to: Option<GlobalSequence>,
    ) -> Result<Vec<AppEventEnvelope>, HubError> {
        self.hub.replay(from, to)
    }

    pub fn current_sequence(&self) -> u64 {
        self.hub.current().0
    }

    fn next_revision(&self) -> u64 {
        self.revision.fetch_add(1, Ordering::Relaxed)
    }

    fn next_event_id(&self) -> EventId {
        let n = self.next_event.fetch_add(1, Ordering::Relaxed);
        EventId::from(format!("app-evt-{n}"))
    }

    fn publish(
        &self,
        instance: pawork_domain::CoreInstanceId,
        envelope: &AgentEventEnvelope,
        event: AppEvent,
    ) {
        let app_envelope = AppEventEnvelope {
            api_version: API_VERSION,
            instance_id: instance,
            event_id: self.next_event_id(),
            global_sequence: GlobalSequence(0),
            stream: EventStream::Session(envelope.session_id.clone()),
            stream_sequence: envelope.sequence.0,
            timestamp: envelope.timestamp,
            source: EventSource::Core,
            payload: event,
        };
        self.hub.publish(app_envelope);
    }

    fn publish_raw(
        &self,
        instance: pawork_domain::CoreInstanceId,
        session_id: &SessionId,
        event: AppEvent,
    ) {
        let app_envelope = AppEventEnvelope {
            api_version: API_VERSION,
            instance_id: instance,
            event_id: self.next_event_id(),
            global_sequence: GlobalSequence(0),
            stream: EventStream::Session(session_id.clone()),
            stream_sequence: 0,
            timestamp: now_timestamp(),
            source: EventSource::Core,
            payload: event,
        };
        self.hub.publish(app_envelope);
    }

    fn publish_terminal(
        &self,
        instance: pawork_domain::CoreInstanceId,
        terminal_session_id: &str,
        event: AppEvent,
    ) {
        let app_envelope = AppEventEnvelope {
            api_version: API_VERSION,
            instance_id: instance,
            event_id: self.next_event_id(),
            global_sequence: GlobalSequence(0),
            stream: EventStream::Terminal(terminal_session_id.to_string()),
            stream_sequence: 0,
            timestamp: now_timestamp(),
            source: EventSource::Core,
            payload: event,
        };
        self.hub.publish(app_envelope);
    }
}

impl GuiEventBus {
    pub fn publish_diagnostic(
        &self,
        instance: pawork_domain::CoreInstanceId,
        session_id: &SessionId,
        code: &str,
        details: Value,
    ) {
        self.publish_raw(
            instance,
            session_id,
            AppEvent::Diagnostic {
                level: DiagnosticLevel::Info,
                code: code.to_string(),
                message: details.to_string(),
            },
        );
    }
}

/// GUI 侧渲染 sink：把 persist 之后的事件映射为 App 事件广播出去。
pub struct GuiBroadcastSink {
    bus: Arc<GuiEventBus>,
    instance: pawork_domain::CoreInstanceId,
}

impl GuiBroadcastSink {
    pub fn new(bus: Arc<GuiEventBus>, instance: pawork_domain::CoreInstanceId) -> Self {
        Self { bus, instance }
    }
}

#[async_trait]
impl AgentEventSink for GuiBroadcastSink {
    async fn emit(&self, envelope: AgentEventEnvelope) -> Result<(), EngineError> {
        if let Some(event) = broadcast_event(&envelope) {
            self.bus.publish(self.instance.clone(), &envelope, event);
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct ActiveGuiRun {
    pub run_id: RunId,
    pub session_id: SessionId,
    pub started_at_ms: u64,
}

/// 活动 Run 注册表：GUI RunStart 登记，RunCancel 找令牌，完成后摘除。
#[derive(Default)]
pub struct GuiRunRegistry {
    runs: Mutex<HashMap<String, (ActiveGuiRun, CancellationToken)>>,
}

impl GuiRunRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    fn register(&self, run: ActiveGuiRun, token: CancellationToken) {
        self.runs
            .lock()
            .expect("gui run registry poisoned")
            .insert(run.run_id.as_str().to_string(), (run, token));
    }

    fn remove(&self, run_id: &RunId) {
        self.runs
            .lock()
            .expect("gui run registry poisoned")
            .remove(run_id.as_str());
    }

    pub fn cancel(&self, run_id: &RunId) -> bool {
        let mut runs = self.runs.lock().expect("gui run registry poisoned");
        match runs.remove(run_id.as_str()) {
            Some((_, token)) => {
                token.cancel();
                true
            }
            None => false,
        }
    }

    pub fn active(&self) -> Vec<ActiveGuiRun> {
        let runs = self.runs.lock().expect("gui run registry poisoned");
        let mut list: Vec<_> = runs.values().map(|(run, _)| run.clone()).collect();
        list.sort_by(|a, b| a.started_at_ms.cmp(&b.started_at_ms));
        list
    }

    pub fn contains(&self, run_id: &RunId) -> bool {
        self.runs
            .lock()
            .expect("gui run registry poisoned")
            .contains_key(run_id.as_str())
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

    fn terminal_owner(&self, terminal_session_id: &str) -> Result<OwnerSessionId, GuiHostError> {
        self.terminals
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(terminal_session_id)
            .cloned()
            .map(OwnerSessionId::new)
            .ok_or_else(|| {
                Self::host_error(
                    "not_found",
                    format!("terminal {terminal_session_id} is not registered"),
                )
            })
    }

    fn remember_terminal(&self, terminal_id: &TerminalId, owner: &OwnerSessionId) {
        self.terminals
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(terminal_id.as_str().to_string(), owner.as_str().to_string());
    }

    fn spawn_terminal_forwarder(&self, terminal_id: TerminalId, owner: OwnerSessionId) {
        let Ok(mut receiver) = self.pty.subscribe(&terminal_id, &owner) else {
            return;
        };
        let bus = Arc::clone(&self.bus);
        let instance = self.instance.clone();
        let terminal_session_id = terminal_id.as_str().to_string();
        tokio::spawn(async move {
            loop {
                match receiver.recv().await {
                    Ok(PtyEvent::Output { data, .. }) => {
                        bus.publish_terminal(
                            instance.clone(),
                            &terminal_session_id,
                            AppEvent::TerminalOutput {
                                terminal_session_id: terminal_session_id.clone(),
                                delta: String::from_utf8_lossy(&data).into_owned(),
                            },
                        );
                    }
                    Ok(PtyEvent::Exit { .. }) => break,
                    Err(_) => break,
                }
            }
        });
    }

    fn resolve_terminal_cwd(
        core: &crate::AppCore,
        workspace_id: &WorkspaceId,
        working_directory: Option<&WorkspaceRelativePath>,
    ) -> Result<Option<PathBuf>, GuiHostError> {
        let roots = if workspace_id.as_str() == core.workspace_id().as_str() {
            core.workspace_roots.clone()
        } else {
            core.workspaces
                .get(workspace_id)
                .map_err(|error| Self::host_error("app_error", error.to_string()))?
                .map(|workspace| workspace.roots)
                .unwrap_or_default()
        };
        match working_directory {
            None => Ok(roots.first().cloned()),
            Some(relative) => {
                if roots.is_empty() {
                    return Err(Self::host_error(
                        "not_found",
                        format!("workspace {} has no roots", workspace_id.as_str()),
                    ));
                }
                resolve_relative_path(&roots, relative.as_str())
                    .map(|resolved| Some(resolved.absolute))
                    .map_err(|error| Self::host_error("invalid_argument", error.to_string()))
            }
        }
    }

    fn terminal_snapshots(&self) -> Vec<Value> {
        let registered: Vec<(String, String)> = self
            .terminals
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .map(|(id, owner)| (id.clone(), owner.clone()))
            .collect();
        registered
            .into_iter()
            .filter_map(|(id, owner)| {
                let terminal_id = TerminalId::new(id);
                let owner = OwnerSessionId::new(owner);
                let snapshot = self.pty.snapshot(&terminal_id, &owner).ok()?;
                Some(json!({
                    "terminal_session_id": snapshot.terminal_id.as_str(),
                    "owner_session": snapshot.owner_session.as_str(),
                    "state": format!("{:?}", snapshot.state).to_ascii_lowercase(),
                    "columns": snapshot.size.cols,
                    "rows": snapshot.size.rows,
                    "dropped_events": snapshot.dropped_events,
                }))
            })
            .collect()
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
        let limit = limit.unwrap_or(DEFAULT_LIMIT).min(MAX_LIMIT) as usize;
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
        let complete = (items.len() < limit)
            || items.last().is_some_and(|item| item.sequence >= head);
        let next_sequence = if complete {
            None
        } else {
            items.last().map(|item| item.sequence)
        };
        Ok(TimelinePage {
            items,
            next_sequence,
            head_sequence: head,
            complete,
        })
    }

    async fn query(&self, envelope: &AppQueryEnvelope) -> Result<AppResponse, GuiHostError> {
        match &envelope.query {
            AppQuery::WorkspaceList => {
                let core = self.core.read().await;
                let roots: Vec<Value> = core
                    .workspace_roots
                    .iter()
                    .map(|path| json!({ "path": path.display().to_string() }))
                    .collect();
                Ok(AppResponse::Data(json!([{
                    "id": core.workspace_id().as_str(),
                    "name": core.workspace_name(),
                    "trusted": core.workspace_trusted(),
                    "roots": roots,
                }])))
            }
            AppQuery::SessionGet {
                session_id,
                timeline_after_sequence,
                timeline_limit,
            } => {
                let record = self
                    .core
                    .read()
                    .await
                    .get_session(session_id)
                    .await
                    .map_err(Self::app_error)?;
                let workspace_id = self
                    .core
                    .read()
                    .await
                    .session_workspace_for_record(record.session_id.as_str());
                let mut data = session_tree_entry(&record, workspace_id);
                if timeline_after_sequence.is_some() || timeline_limit.is_some() {
                    let page = self
                        .timeline(session_id, *timeline_after_sequence, *timeline_limit)
                        .await?;
                    data["timeline_page"] = serde_json::to_value(page)
                        .map_err(|error| Self::host_error("internal", error.to_string()))?;
                }
                Ok(AppResponse::Data(data))
            }
            AppQuery::ModelList { provider_id } => {
                // 与 `pawork models` 同一聚合目录，供 Desktop 切换已配置
                // provider/model；单通道 `model_catalog` 只含当前宿主。
                let catalog = self.core.read().await.models_overview().await;
                let entries: Vec<_> = catalog
                    .iter()
                    .filter(|entry| {
                        provider_id
                            .as_ref()
                            .map(|id| id.as_str() == entry.provider.as_str())
                            .unwrap_or(true)
                    })
                    .map(|entry| {
                        json!({
                            "provider_id": entry.provider.as_str(),
                            "id": entry.id.as_str(),
                            "display_name": entry.display_name,
                            "context_window_tokens": entry.context_window_tokens,
                        })
                    })
                    .collect();
                Ok(AppResponse::Data(Value::Array(entries)))
            }
            AppQuery::RunStatus { run_id } => {
                let state = if self.runs.contains(run_id) {
                    "running"
                } else {
                    "unknown"
                };
                Ok(AppResponse::Data(json!({
                    "run_id": run_id.as_str(),
                    "state": state,
                })))
            }
            AppQuery::DiffListFiles { .. } => {
                let core = self.core.read().await;
                match core.resolve_session("latest").await {
                    Ok(session) => {
                        let diff = core.session_diff(&session).await.map_err(Self::app_error)?;
                        Ok(AppResponse::Data(json!({
                            "session_id": session.as_str(),
                            "files": diff.files.iter().map(|file| json!({
                                "path": file.path,
                                "status": file.status,
                                "additions": file.additions,
                                "deletions": file.deletions,
                                "binary": file.binary,
                            })).collect::<Vec<_>>(),
                            "git": diff.git.as_ref().map(|git| json!({
                                "branch": git.branch,
                                "work_dir": git.work_dir,
                                "dirty_files": git.dirty_files,
                            })),
                        })))
                    }
                    Err(crate::AppError::SessionNotFound(_)) => {
                        Ok(AppResponse::Data(json!({ "files": [] })))
                    }
                    Err(error) => Err(Self::app_error(error)),
                }
            }
            AppQuery::QuotaOverview { query } => {
                let provider = query
                    .provider_id
                    .as_ref()
                    .map(|id| id.as_str().to_string())
                    .filter(|id| !id.is_empty());
                let session = None;
                let overview = self
                    .core
                    .read()
                    .await
                    .usage_overview(provider.as_deref(), session)
                    .await
                    .map_err(|error| Self::host_error("quota", error.to_string()))?;
                Ok(AppResponse::Data(
                    serde_json::to_value(overview)
                        .map_err(|error| Self::host_error("internal", error.to_string()))?,
                ))
            }
            AppQuery::DiffGet { path, cursor, .. } => {
                let core = self.core.read().await;
                let session = match core.resolve_session("latest").await {
                    Ok(session) => session,
                    Err(crate::AppError::SessionNotFound(_)) => {
                        return Ok(AppResponse::Data(json!({
                            "path": path.as_str(),
                            "files": [],
                            "complete": true,
                        })));
                    }
                    Err(error) => return Err(Self::app_error(error)),
                };
                let diff = core.session_diff(&session).await.map_err(Self::app_error)?;
                let Some(file) = diff
                    .files
                    .iter()
                    .find(|file| file.path == path.as_str())
                    .cloned()
                else {
                    return Ok(AppResponse::Data(json!({
                        "path": path.as_str(),
                        "files": [],
                        "complete": true,
                    })));
                };
                let page = cursor
                    .as_deref()
                    .and_then(|value| value.parse::<usize>().ok())
                    .unwrap_or(1);
                let paged = crate::paginate_diff(vec![file], page, 1);
                Ok(AppResponse::Data(json!({
                    "session_id": session.as_str(),
                    "path": path.as_str(),
                    "page": paged.page,
                    "total_files": paged.total_files,
                    "files": paged.files,
                    "complete": page >= paged.page && paged.files.is_empty() || paged.page * 1 >= paged.total_files,
                })))
            }
            other => Err(Self::host_error(
                "unsupported",
                format!(
                    "query {} is not part of the S7 wave A slice",
                    query_wire_name(other)
                ),
            )),
        }
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

impl GuiHostAdapter {
    async fn dispatch_command(
        &self,
        envelope: &AppCommandEnvelope,
    ) -> Result<AppResponse, GuiHostError> {
        match &envelope.command {
            AppCommand::WorkspaceAdd { root_path } => {
                let mut core = self.core.write().await;
                core.attach_workspace(std::path::Path::new(root_path))
                    .map_err(Self::app_error)?;
                Ok(AppResponse::Data(json!({
                    "id": core.workspace_id().as_str(),
                    "name": core.workspace_name(),
                })))
            }
            AppCommand::SessionCreate {
                title,
                workspace_id,
            } => {
                let title = title.clone().unwrap_or_else(|| "New session".into());
                let session_id = self
                    .core
                    .read()
                    .await
                    .create_session_with_workspace(title.clone(), workspace_id.clone())
                    .await
                    .map_err(Self::app_error)?;
                Ok(AppResponse::Data(session_view_json(
                    &session_id,
                    workspace_id,
                    &title,
                    None,
                )))
            }
            AppCommand::SessionOpen { session_id } => {
                let core = self.core.read().await;
                let record = core.get_session(session_id).await.map_err(Self::app_error)?;
                let workspace_id = core
                    .session_workspace(session_id)
                    .unwrap_or_else(|| core.workspace_id().clone());
                Ok(AppResponse::Data(session_view_json(
                    session_id,
                    &workspace_id,
                    &record.title,
                    Some(&record.active_branch),
                )))
            }
            AppCommand::RunStart {
                session_id,
                user_message,
                model,
                provider,
                profile: _,
            } => {
                let history = {
                    let core = self.core.read().await;
                    core.get_session(session_id)
                        .await
                        .map_err(Self::app_error)?;
                    if core.provider_pending() {
                        return Ok(AppResponse::Error(ErrorContext {
                            category: ErrorCategory::Authentication,
                            message: format!(
                                "provider {} 未装配凭证：先 pawork auth set-key {} 或 pawork auth login {}",
                                core.provider_id().as_str(),
                                core.provider_id().as_str(),
                                core.provider_id().as_str()
                            ),
                            retryable: false,
                            retry_after_ms: None,
                            diagnostics: Default::default(),
                        }));
                    }
                    core.resume_messages(session_id)
                        .await
                        .map_err(Self::app_error)?
                };
                let current = {
                    let core = self.core.read().await;
                    (
                        core.provider_id().as_str().to_string(),
                        core.model().as_str().to_string(),
                    )
                };
                if let Some((requested_provider, requested_model)) =
                    run_start_requested_provider_switch(
                        &current.0,
                        &current.1,
                        provider.as_ref().map(|id| id.as_str()),
                        model.as_ref().map(|id| id.as_str()),
                    )
                {
                    let mut core = self.core.write().await;
                    core.switch_provider(
                        Some(session_id),
                        &requested_provider,
                        requested_model.as_deref(),
                    )
                    .await
                    .map_err(Self::app_error)?;
                    let confirmed = (
                        core.provider_id().as_str().to_string(),
                        core.model().as_str().to_string(),
                    );
                    drop(core);
                    if confirmed != current {
                        self.bus.publish_diagnostic(
                            self.instance.clone(),
                            session_id,
                            "model.switched",
                            json!({
                                "from": {
                                    "provider": current.0,
                                    "model": current.1
                                },
                                "to": {
                                    "provider": confirmed.0,
                                    "model": confirmed.1,
                                }
                            }),
                        );
                    }
                } else if provider.is_none() {
                    if let Some(model) = model {
                        if model.as_str() != current.1 {
                        let mut core = self.core.write().await;
                        let switched = match core
                            .switch_model(Some(session_id), model.as_str())
                            .await
                        {
                            Ok(()) => Ok(()),
                            Err(crate::AppError::ModelBelongsToProvider { owner, .. }) => {
                                core.switch_provider(
                                    Some(session_id),
                                    &owner,
                                    Some(model.as_str()),
                                )
                                .await
                            }
                            Err(error @ crate::AppError::UnknownModel { .. }) => {
                                let owner = core
                                    .models_overview()
                                    .await
                                    .into_iter()
                                    .find(|entry| entry.id.as_str() == model.as_str())
                                    .map(|entry| entry.provider.as_str().to_string());
                                match owner {
                                    Some(owner) if owner != current.0 => {
                                        match core
                                            .switch_provider(
                                                Some(session_id),
                                                &owner,
                                                Some(model.as_str()),
                                            )
                                            .await
                                        {
                                            Ok(()) => Ok(()),
                                            Err(crate::AppError::UnknownModel { .. }) => {
                                                match core
                                                    .switch_provider(
                                                        Some(session_id),
                                                        &owner,
                                                        None,
                                                    )
                                                    .await
                                                {
                                                    Ok(()) => {
                                                        core.switch_model(
                                                            Some(session_id),
                                                            model.as_str(),
                                                        )
                                                        .await
                                                    }
                                                    Err(other) => Err(other),
                                                }
                                            }
                                            Err(other) => Err(other),
                                        }
                                    }
                                    _ => Err(error),
                                }
                            }
                            Err(error) => Err(error),
                        };
                        switched.map_err(Self::app_error)?;
                        let confirmed = (
                            core.provider_id().as_str().to_string(),
                            core.model().as_str().to_string(),
                        );
                        drop(core);
                        self.bus.publish_diagnostic(
                            self.instance.clone(),
                            session_id,
                            "model.switched",
                            json!({
                                "from": {
                                    "provider": current.0,
                                    "model": current.1
                                },
                                "to": {
                                    "provider": confirmed.0,
                                    "model": confirmed.1,
                                }
                            }),
                        );
                        }
                    }
                }
                let n = self.next_gui_run.fetch_add(1, Ordering::Relaxed);
                let run_id = RunId::from(format!(
                    "run-gui-{}-{n}",
                    now_timestamp().as_unix_millis()
                ));
                let token = CancellationToken::new();
                self.runs.register(
                    ActiveGuiRun {
                        run_id: run_id.clone(),
                        session_id: session_id.clone(),
                        started_at_ms: now_timestamp().as_unix_millis(),
                    },
                    token.clone(),
                );
                let core = Arc::clone(&self.core);
                let bus = Arc::clone(&self.bus);
                let runs = Arc::clone(&self.runs);
                let approvals = Arc::clone(&self.approvals);
                let instance = self.instance.clone();
                let session = session_id.clone();
                let run = run_id.clone();
                let mut messages = history;
                messages.push(Message {
                    id: MessageId::from("pending"),
                    role: MessageRole::User,
                    content: vec![ContentPart::Text(TextContent {
                        text: user_message.clone(),
                    })],
                    metadata: Default::default(),
                });
                tokio::spawn(async move {
                    let sink = GuiBroadcastSink::new(Arc::clone(&bus), instance.clone());
                    let outcome = {
                        let core = core.read().await;
                        core.chat_turn_with_run_id(
                            run.clone(),
                            &session,
                            messages,
                            &sink,
                            token,
                        )
                        .await
                    };
                    if let Err(error) = outcome {
                        bus.publish_raw(
                            instance.clone(),
                            &session,
                            AppEvent::RunChanged {
                                run_id: run.clone(),
                                state: RunState::Failed,
                            },
                        );
                        bus.publish_diagnostic(
                            instance,
                            &session,
                            "run.failed",
                            json!({ "message": error.to_string() }),
                        );
                    }
                    approvals.clear_run(&run);
                    runs.remove(&run);
                });
                Ok(AppResponse::Accepted {
                    command_id: envelope.command_id.clone(),
                    run_id: Some(run_id),
                })
            }
            AppCommand::RunCancel { run_id } => {
                if self.runs.cancel(run_id) {
                    Ok(AppResponse::Accepted {
                        command_id: envelope.command_id.clone(),
                        run_id: None,
                    })
                } else {
                    Err(Self::host_error(
                        "not_found",
                        format!("run {} is not active", run_id.as_str()),
                    ))
                }
            }
            AppCommand::ToolApprove {
                run_id,
                tool_call_id,
                decision,
            } => {
                self.approvals
                    .resolve(run_id, tool_call_id, protocol_to_domain_decision(decision))
                    .map_err(|message| Self::host_error("conflict", message))?;
                Ok(AppResponse::Accepted {
                    command_id: envelope.command_id.clone(),
                    run_id: None,
                })
            }
            AppCommand::SessionFork {
                session_id,
                parent_event_id,
            } => {
                let core = self.core.read().await;
                let record = core.get_session(session_id).await.map_err(Self::app_error)?;
                let store = core.store().map_err(Self::app_error)?;
                let n = self.next_fork.fetch_add(1, Ordering::Relaxed);
                let branch_id = format!(
                    "fork-{}-{n}",
                    now_timestamp().as_unix_millis()
                );
                store
                    .fork_from_event(session_id, &branch_id, parent_event_id)
                    .await
                    .map_err(Self::session_error)?;
                store
                    .switch_branch(session_id, &branch_id)
                    .await
                    .map_err(Self::session_error)?;
                let workspace_id = core
                    .session_workspace(session_id)
                    .unwrap_or_else(|| core.workspace_id().clone());
                let mut data = session_view_json(
                    session_id,
                    &workspace_id,
                    &record.title,
                    Some(&branch_id),
                );
                data["branch_id"] = json!(branch_id);
                data["parent_event_id"] = json!(parent_event_id.as_str());
                Ok(AppResponse::Data(data))
            }
            AppCommand::TerminalCreate {
                workspace_id,
                working_directory,
            } => {
                let core = self.core.read().await;
                let cwd = Self::resolve_terminal_cwd(
                    &core,
                    workspace_id,
                    working_directory.as_ref(),
                )?;
                drop(core);
                let owner = OwnerSessionId::new(workspace_id.as_str());
                let spec = PtyCreateSpec {
                    owner_session: owner.clone(),
                    cwd,
                    size: PtyWindowSize::default(),
                    ..PtyCreateSpec::default()
                };
                let terminal_id = self.pty.create(spec).await.map_err(Self::pty_error)?;
                self.remember_terminal(&terminal_id, &owner);
                self.spawn_terminal_forwarder(terminal_id.clone(), owner);
                Ok(AppResponse::Data(json!({
                    "terminal_session_id": terminal_id.as_str(),
                    "uncontrolled": true,
                    "note": "本机不受控终端：不经沙箱与审批",
                })))
            }
            AppCommand::TerminalWrite {
                terminal_session_id,
                data,
            } => {
                let owner = self.terminal_owner(terminal_session_id)?;
                self.pty
                    .write(
                        &TerminalId::new(terminal_session_id),
                        &owner,
                        data.as_bytes().to_vec(),
                    )
                    .await
                    .map_err(Self::pty_error)?;
                Ok(AppResponse::Accepted {
                    command_id: envelope.command_id.clone(),
                    run_id: None,
                })
            }
            AppCommand::TerminalResize {
                terminal_session_id,
                columns,
                rows,
            } => {
                let owner = self.terminal_owner(terminal_session_id)?;
                self.pty
                    .resize(
                        &TerminalId::new(terminal_session_id),
                        &owner,
                        PtyWindowSize {
                            rows: *rows,
                            cols: *columns,
                            pixel_width: 0,
                            pixel_height: 0,
                        },
                    )
                    .await
                    .map_err(Self::pty_error)?;
                Ok(AppResponse::Accepted {
                    command_id: envelope.command_id.clone(),
                    run_id: None,
                })
            }
            other => Err(Self::host_error(
                "unsupported",
                format!("command {} is not supported", command_wire_name(other)),
            )),
        }
    }
}

/// 把持久化的 Agent 事件投影为 presentation-safe 的 Timeline 条目。
pub fn project_timeline_item(envelope: &AgentEventEnvelope) -> Option<TimelineItem> {
    let (kind, text, tool_name, status, detail) = match &envelope.payload {
        AgentEvent::MessageCommitted { message } => match message.role {
            MessageRole::User => (
                TimelineItemKind::UserMessage,
                Some(join_text(&message.content)),
                None,
                None,
                None,
            ),
            MessageRole::Assistant => (
                TimelineItemKind::AssistantMessage,
                Some(join_text(&message.content)),
                None,
                None,
                None,
            ),
            _ => return None,
        },
        AgentEvent::AssistantTextDelta { delta, .. } => (
            TimelineItemKind::AssistantDelta,
            Some(delta.clone()),
            None,
            None,
            None,
        ),
        AgentEvent::ToolCallStarted { name, .. } => (
            TimelineItemKind::ToolStarted,
            None,
            Some(name.clone()),
            Some("running".into()),
            None,
        ),
        AgentEvent::ToolOutputDelta { delta, .. } => (
            TimelineItemKind::ToolOutput,
            Some(delta.clone()),
            None,
            None,
            None,
        ),
        AgentEvent::ToolExecutionCompleted { result, .. } => (
            TimelineItemKind::ToolCompleted,
            Some(join_text(&result.content)),
            result.tool_name.clone(),
            Some(if result.is_error { "failed" } else { "succeeded" }.into()),
            sandbox_timeline_detail(&result.metadata),
        ),
        AgentEvent::ToolApprovalRequested { reason, .. } => (
            TimelineItemKind::ApprovalRequested,
            None,
            None,
            Some("pending".into()),
            Some(reason.clone()),
        ),
        AgentEvent::ToolApprovalResponded { decision, .. } => (
            TimelineItemKind::ApprovalResponded,
            None,
            None,
            Some(decision_status(decision)),
            None,
        ),
        AgentEvent::RunStarted { .. } => {
            (TimelineItemKind::RunStarted, None, None, None, None)
        }
        AgentEvent::RunCompleted { .. } => {
            (TimelineItemKind::RunCompleted, None, None, None, None)
        }
        AgentEvent::RunCancelled { .. } => {
            (TimelineItemKind::RunCancelled, None, None, None, None)
        }
        AgentEvent::RunFailed { error, .. } => (
            TimelineItemKind::RunFailed,
            None,
            None,
            Some("failed".into()),
            Some(error.message.clone()),
        ),
        AgentEvent::Diagnostic { code, details } => (
            TimelineItemKind::Diagnostic,
            None,
            None,
            None,
            Some(format!("{code}: {details}")),
        ),
        AgentEvent::CheckpointCreated { checkpoint_id, .. } => (
            TimelineItemKind::Other,
            None,
            None,
            None,
            Some(format!("checkpoint {}", checkpoint_id.as_str())),
        ),
        AgentEvent::CheckpointRolledBack { checkpoint_id } => (
            TimelineItemKind::Other,
            None,
            None,
            None,
            Some(format!("rollback {}", checkpoint_id.as_str())),
        ),
        _ => return None,
    };
    Some(TimelineItem {
        sequence: envelope.sequence.0,
        event_id: envelope.event_id.as_str().to_string(),
        kind,
        run_id: Some(envelope.run_id.as_str().to_string()),
        text,
        tool_name,
        status,
        detail,
        timestamp: envelope.timestamp.as_unix_millis().to_string(),
    })
}

fn sandbox_timeline_detail(metadata: &serde_json::Value) -> Option<String> {
    let sandbox = metadata.get("sandbox")?;
    if !sandbox.get("fallback")?.as_bool()? {
        return None;
    }
    let isolation = sandbox
        .get("isolation")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unknown");
    let backend = sandbox
        .get("backend")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unknown");
    Some(format!("沙箱回退：isolation={isolation} backend={backend}"))
}

fn join_text(parts: &[ContentPart]) -> String {
    let mut text = String::new();
    for part in parts {
        if let ContentPart::Text(content) = part {
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(&content.text);
        }
    }
    text
}

fn decision_status(decision: &ApprovalDecision) -> String {
    match decision {
        ApprovalDecision::ApprovedOnce => "approve_once".into(),
        ApprovalDecision::ApprovedForRun => "approve_for_run".into(),
        ApprovalDecision::Denied => "deny".into(),
        ApprovalDecision::Cancelled => "cancelled".into(),
    }
}

/// 选择要广播给 GUI 的 App 事件；其余事件仍持久化，只是不进实时流。
fn broadcast_event(envelope: &AgentEventEnvelope) -> Option<AppEvent> {
    let run = envelope.run_id.clone();
    Some(match &envelope.payload {
        AgentEvent::RunStarted { .. } => AppEvent::RunChanged {
            run_id: run,
            state: RunState::Created,
        },
        AgentEvent::AssistantTextDelta { message_id, delta } => AppEvent::AssistantDelta {
            run_id: run,
            message_id: message_id.clone(),
            delta: delta.clone(),
        },
        AgentEvent::ToolCallStarted {
            tool_call_id,
            name,
        } => AppEvent::ToolStarted {
            run_id: run,
            tool_call_id: tool_call_id.clone(),
            name: name.clone(),
        },
        AgentEvent::ToolOutputDelta {
            tool_call_id,
            delta,
            ..
        } => AppEvent::ToolOutput {
            run_id: run,
            tool_call_id: tool_call_id.clone(),
            delta: delta.clone(),
            truncated: false,
            artifact_id: None,
        },
        AgentEvent::ToolApprovalRequested { .. } => {
            // Live 卡片由 GuiApprovalHost::decide 注册时广播；engine 在
            // decide 返回后才发 Requested/Responded 对，再映射会把已决
            // 策的卡片重新点亮。
            return None;
        }
        AgentEvent::ToolExecutionCompleted { result, .. } => AppEvent::ToolCompleted {
            run_id: run,
            tool_call_id: result.tool_call_id.clone(),
            success: !result.is_error,
        },
        AgentEvent::RunCompleted { .. } => AppEvent::RunChanged {
            run_id: run,
            state: RunState::Completed,
        },
        AgentEvent::RunCancelled { .. } => AppEvent::RunChanged {
            run_id: run,
            state: RunState::Cancelled,
        },
        AgentEvent::RunFailed { .. } => AppEvent::RunChanged {
            run_id: run,
            state: RunState::Failed,
        },
        AgentEvent::Diagnostic { code, details } => AppEvent::Diagnostic {
            level: DiagnosticLevel::Info,
            code: code.clone(),
            message: details.to_string(),
        },
        _ => return None,
    })
}

/// 幂等按 GUI 客户端隔离：各连接独立生成 `gui-cmd-N`，不得把 A 的
/// SessionCreate 重放成 B 的 RunCancel。
fn scoped_idempotency(envelope: &AppCommandEnvelope) -> (CommandId, Option<String>) {
    let client_id = match &envelope.source {
        CommandSource::LocalGui { client_id } | CommandSource::RemoteGui { client_id, .. } => {
            Some(client_id.as_str())
        }
        _ => None,
    };
    match client_id {
        Some(client_id) => (
            CommandId::from(format!("{client_id}/{}", envelope.command_id.as_str())),
            envelope
                .idempotency_key
                .as_ref()
                .map(|key| format!("{client_id}/{key}")),
        ),
        None if matches!(envelope.source, CommandSource::Automation) => (
            CommandId::from(format!("automation/{}", envelope.command_id.as_str())),
            envelope
                .idempotency_key
                .as_ref()
                .map(|key| format!("automation/{key}")),
        ),
        None => (
            envelope.command_id.clone(),
            envelope.idempotency_key.clone(),
        ),
    }
}

/// 有 `RunStart.provider` 时按用户所选通道切换，禁止回退 catalog 首项。
fn run_start_requested_provider_switch(
    current_provider: &str,
    current_model: &str,
    requested_provider: Option<&str>,
    requested_model: Option<&str>,
) -> Option<(String, Option<String>)> {
    let provider = requested_provider?;
    let already = current_provider == provider
        && requested_model.is_none_or(|model| current_model == model);
    if already {
        None
    } else {
        Some((provider.to_string(), requested_model.map(str::to_string)))
    }
}

/// 旧客户端兼容：仅有 model 时按 overview 顺序取首个同 id。
#[cfg(test)]
fn run_start_overview_owner<'a, P, M>(
    model: &str,
    overview: impl IntoIterator<Item = &'a (P, M)>,
) -> Option<String>
where
    P: AsRef<str> + 'a,
    M: AsRef<str> + 'a,
{
    overview.into_iter().find_map(|(provider, id)| {
        (id.as_ref() == model).then(|| provider.as_ref().to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::approval::ApprovalPromptHost;
    use pawork_domain::{CommandId, MessageMetadata, Timestamp, WorkspaceId};
    use pawork_protocol::{ActorIdentity, CommandSource};
    use std::sync::atomic::{AtomicU64, Ordering};
    use pawork_testkit::{MockProvider, MockScript};

    struct NoopSink;

    #[async_trait]
    impl AgentEventSink for NoopSink {
        async fn emit(&self, _envelope: AgentEventEnvelope) -> Result<(), EngineError> {
            Ok(())
        }
    }

    fn next_test_command_id() -> CommandId {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        CommandId::from(format!(
            "cmd-test-{}",
            NEXT.fetch_add(1, Ordering::Relaxed)
        ))
    }

    fn command_envelope(command: AppCommand) -> AppCommandEnvelope {
        command_envelope_with(next_test_command_id(), None, command)
    }

    fn command_envelope_with(
        command_id: CommandId,
        idempotency_key: Option<&str>,
        command: AppCommand,
    ) -> AppCommandEnvelope {
        AppCommandEnvelope {
            api_version: API_VERSION,
            command_id,
            source: CommandSource::Automation,
            identity: ActorIdentity::System,
            expected_revision: None,
            idempotency_key: idempotency_key.map(str::to_string),
            issued_at: Timestamp::from_unix_millis(1),
            command,
        }
    }

    fn session_titles(snapshot: &Snapshot, title: &str) -> usize {
        snapshot
            .sections
            .iter()
            .find(|section| section.kind == SnapshotSectionKind::SessionTree)
            .and_then(|section| section.data.clone())
            .and_then(|data| data.as_array().cloned())
            .unwrap_or_default()
            .iter()
            .filter(|entry| entry.get("title").and_then(Value::as_str) == Some(title))
            .count()
    }

    fn query_envelope(query: AppQuery) -> AppQueryEnvelope {
        AppQueryEnvelope {
            api_version: API_VERSION,
            request_id: pawork_domain::QueryId::from("query-test-1"),
            source: CommandSource::Automation,
            identity: ActorIdentity::System,
            issued_at: Timestamp::from_unix_millis(1),
            query,
        }
    }

    async fn core_with_turn() -> (Arc<AppCore>, tempfile::TempDir, SessionId) {
        let dir = tempfile::tempdir().expect("tempdir");
        let (store, _) = pawork_storage::session::SessionStore::open(dir.path().join("session.db"))
            .await
            .expect("store");
        let provider = MockProvider::sequence(vec![
            MockScript::new().text("hello from mock").complete(),
        ]);
        let core = AppCore::from_parts(
            Arc::new(provider),
            None,
            pawork_domain::ModelId::from("model-1"),
            pawork_domain::ProviderId::from("mock"),
            Some(store),
        );
        let session = core.create_session("gui").await.expect("session");
        let message = Message {
            id: MessageId::from("m-1"),
            role: MessageRole::User,
            content: vec![ContentPart::Text(TextContent {
                text: "hi".into(),
            })],
            metadata: MessageMetadata::default(),
        };
        core.chat_turn(&session, vec![message], &NoopSink, CancellationToken::new())
            .await
            .expect("turn");
        (Arc::new(core), dir, session)
    }

    #[tokio::test]
    async fn timeline_projects_and_pages_by_sequence() {
        let (core, _dir, session) = core_with_turn().await;
        let adapter = GuiHostAdapter::new(core);
        let page = adapter.timeline(&session, None, Some(500)).await.expect("page");
        assert!(page.complete, "single page covers the whole session");
        assert_eq!(page.next_sequence, None);
        let kinds: Vec<_> = page.items.iter().map(|item| item.kind.clone()).collect();
        assert!(kinds.contains(&TimelineItemKind::UserMessage));
        assert!(kinds.contains(&TimelineItemKind::AssistantMessage));
        assert!(
            page.items.iter().all(|item| item.sequence >= 1),
            "sequences are session-level and monotonic"
        );

        let first = page.items.first().expect("non-empty").sequence;
        let second = adapter
            .timeline(&session, Some(first), Some(500))
            .await
            .expect("second page");
        assert!(second.complete);
        assert!(second.items.iter().all(|item| item.sequence > first));
    }

    #[tokio::test]
    async fn model_list_uses_aggregated_overview() {
        let (core, _dir, _session) = core_with_turn().await;
        let host: Arc<dyn GuiHost> = Arc::new(GuiHostAdapter::new(core));
        let response = host
            .query(&query_envelope(AppQuery::ModelList { provider_id: None }))
            .await
            .expect("model list");
        let AppResponse::Data(data) = response else {
            panic!("model list must return data");
        };
        let entries = data.as_array().expect("model list array");
        let providers: std::collections::BTreeSet<String> = entries
            .iter()
            .filter_map(|entry| {
                entry
                    .get("provider_id")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .collect();
        for expected in ["xai", "glm-coding", "opencode-go", "qwen-token-plan", "deepseek"] {
            assert!(
                providers.contains(expected),
                "ModelList must include {expected}: {providers:?}"
            );
        }
    }

    #[tokio::test]
    async fn workspace_list_includes_registered_roots() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (store, _) = pawork_storage::session::SessionStore::open(dir.path().join("session.db"))
            .await
            .expect("store");
        let provider = MockProvider::sequence(vec![MockScript::new().complete()]);
        let mut core = AppCore::from_parts(
            Arc::new(provider),
            None,
            pawork_domain::ModelId::from("model-1"),
            pawork_domain::ProviderId::from("mock"),
            Some(store),
        );
        core.attach_workspace(dir.path()).expect("attach workspace");
        let adapter = GuiHostAdapter::new(Arc::new(core));
        let response = adapter
            .query(&query_envelope(AppQuery::WorkspaceList))
            .await
            .expect("workspace list");
        let AppResponse::Data(value) = response else {
            panic!("WorkspaceList must return Data, got {response:?}");
        };
        let roots = value
            .as_array()
            .and_then(|entries| entries.first())
            .and_then(|entry| entry.get("roots"))
            .and_then(Value::as_array)
            .expect("WorkspaceList roots array");
        assert_eq!(roots.len(), 1, "one attached root: {roots:?}");
        let listed = roots[0]
            .get("path")
            .and_then(Value::as_str)
            .expect("root path");
        let expected = dir.path().canonicalize().unwrap_or_else(|_| dir.path().to_path_buf());
        let listed_path = std::path::PathBuf::from(listed);
        let listed_canon = listed_path
            .canonicalize()
            .unwrap_or(listed_path);
        assert_eq!(listed_canon, expected);
    }

    #[tokio::test]
    async fn snapshot_contains_baseline_sections() {
        let (core, _dir, _session) = core_with_turn().await;
        let adapter = GuiHostAdapter::new(core);
        let snapshot = adapter.snapshot().await.expect("snapshot");
        let kinds: Vec<_> = snapshot.sections.iter().map(|s| s.kind.clone()).collect();
        for expected in [
            SnapshotSectionKind::Workspaces,
            SnapshotSectionKind::SessionTree,
            SnapshotSectionKind::ActiveRuns,
            SnapshotSectionKind::PendingToolApprovals,
            SnapshotSectionKind::TerminalSessions,
            SnapshotSectionKind::ProviderStatus,
        ] {
            assert!(kinds.contains(&expected), "missing section {expected:?}");
        }
        assert_eq!(snapshot.instance_id, adapter.instance_id());
    }

    #[tokio::test]
    async fn run_start_reports_run_and_registry_drains_after_completion() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (store, _) = pawork_storage::session::SessionStore::open(dir.path().join("session.db"))
            .await
            .expect("store");
        let provider = MockProvider::sequence(vec![
            MockScript::new().wait_for_cancellation(),
        ]);
        let core = AppCore::from_parts(
            Arc::new(provider),
            None,
            pawork_domain::ModelId::from("model-1"),
            pawork_domain::ProviderId::from("mock"),
            Some(store),
        );
        let session = core.create_session("gui-cancel").await.expect("session");
        let adapter = GuiHostAdapter::new(Arc::new(core));
        let runs = adapter.runs();
        let host: Arc<dyn GuiHost> = Arc::new(adapter);
        let response = host
            .command(&command_envelope(AppCommand::RunStart {
                session_id: session.clone(),
                user_message: "another turn".into(),
                model: None,
                provider: None,
                profile: None,
            }))
            .await
            .expect("run accepted");
        let AppResponse::Accepted { run_id: Some(run), .. } = response else {
            panic!("run start must report the run id");
        };
        assert!(runs.contains(&run));

        let cancel_response = host
            .command(&command_envelope(AppCommand::RunCancel { run_id: run.clone() }))
            .await
            .expect("cancel accepted");
        assert!(matches!(cancel_response, AppResponse::Accepted { .. }));
        for _ in 0..100 {
            if !runs.contains(&run) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert!(
            !runs.contains(&run),
            "registry must drain after the run finishes"
        );
    }

    #[tokio::test]
    async fn run_start_switches_same_registry_model_and_unknown_fails_closed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (store, _) = pawork_storage::session::SessionStore::open(dir.path().join("session.db"))
            .await
            .expect("store");
        let provider = MockProvider::sequence(vec![
            MockScript::new().text("hello from other").complete(),
        ])
        .with_models(vec![pawork_domain::ModelDefinition {
            id: pawork_domain::ModelId::from("model-2"),
            display_name: "Model 2".into(),
            context_window_tokens: 8_000,
            max_output_tokens: 1_024,
            capabilities: Default::default(),
        }]);
        let core = AppCore::from_parts(
            Arc::new(provider),
            None,
            pawork_domain::ModelId::from("model-1"),
            pawork_domain::ProviderId::from("mock"),
            Some(store),
        );
        let session = core.create_session("switch").await.expect("session");
        let adapter = GuiHostAdapter::new(Arc::new(core));
        let host: Arc<dyn GuiHost> = Arc::new(adapter);
        let accepted = host
            .command(&command_envelope(AppCommand::RunStart {
                session_id: session.clone(),
                user_message: "hi".into(),
                model: Some(pawork_domain::ModelId::from("model-2")),
                provider: None,
                profile: None,
            }))
            .await
            .expect("same-registry model switch");
        assert!(matches!(accepted, AppResponse::Accepted { .. }));

        let error = host
            .command(&command_envelope(AppCommand::RunStart {
                session_id: session,
                user_message: "nope".into(),
                model: Some(pawork_domain::ModelId::from("missing-model")),
                provider: None,
                profile: None,
            }))
            .await
            .expect_err("unknown model must fail closed");
        assert_eq!(error.code, "unknown_model");
    }

    #[tokio::test]
    async fn tool_approve_resolves_pending_snapshot() {
        let host = Arc::new(GuiApprovalHost::new());
        let ask = crate::ApprovalAsk {
            run_id: RunId::from("run-wait"),
            session_id: Some(SessionId::from("ses-wait")),
            tool_name: "write_file".into(),
            tool_call_id: pawork_domain::ToolCallId::from("call-wait"),
            relative_path: Some("notes.txt".into()),
            message: "Approve workspace file write".into(),
            risk: pawork_policy::RiskLevel::Moderate,
            preview: Some("1 lines\nhello".into()),
        };
        let waiter = {
            let host = Arc::clone(&host);
            let ask = ask.clone();
            tokio::spawn(async move { host.decide(&ask, CancellationToken::new()).await })
        };
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        let dir = tempfile::tempdir().expect("tempdir");
        let (store, _) = pawork_storage::session::SessionStore::open(dir.path().join("session.db"))
            .await
            .expect("store");
        let core = AppCore::from_parts(
            Arc::new(MockProvider::sequence(vec![
                MockScript::new().text("idle").complete(),
            ])),
            None,
            pawork_domain::ModelId::from("model-1"),
            pawork_domain::ProviderId::from("mock"),
            Some(store),
        );
        let adapter = GuiHostAdapter::with_approvals(Arc::new(core), Arc::clone(&host));
        let snapshot = adapter.snapshot().await.expect("snapshot");
        let pending = snapshot
            .sections
            .iter()
            .find(|section| section.kind == SnapshotSectionKind::PendingToolApprovals)
            .and_then(|section| section.data.clone())
            .expect("pending section");
        assert_eq!(pending.as_array().map(|items| items.len()), Some(1));
        let response = adapter
            .command(&command_envelope(AppCommand::ToolApprove {
                run_id: RunId::from("run-wait"),
                tool_call_id: pawork_domain::ToolCallId::from("call-wait"),
                decision: pawork_protocol::ApprovalDecision::ApproveOnce,
            }))
            .await
            .expect("approve");
        assert!(matches!(response, AppResponse::Accepted { .. }));
        let decision = waiter.await.expect("join");
        assert_eq!(decision, ApprovalDecision::ApprovedOnce);
        let snapshot = adapter.snapshot().await.expect("snapshot after");
        let pending = snapshot
            .sections
            .iter()
            .find(|section| section.kind == SnapshotSectionKind::PendingToolApprovals)
            .and_then(|section| section.data.clone())
            .expect("pending section");
        assert_eq!(pending.as_array().map(|items| items.len()), Some(0));
    }

    #[test]
    fn run_registry_cancel_flips_token() {
        let registry = GuiRunRegistry::new();
        let token = CancellationToken::new();
        registry.register(
            ActiveGuiRun {
                run_id: RunId::from("run-x"),
                session_id: SessionId::from("sess-x"),
                started_at_ms: 1,
            },
            token.clone(),
        );
        assert!(registry.cancel(&RunId::from("run-x")));
        assert!(token.is_cancelled());
        assert!(!registry.cancel(&RunId::from("run-missing")));
        assert_eq!(registry.active().len(), 0);
    }

    #[test]
    fn gui_event_bus_publishes_through_event_hub() {
        let bus = GuiEventBus::new(4);
        let instance = pawork_domain::CoreInstanceId::from("instance-1");
        let session = SessionId::from("sess-1");
        bus.publish_diagnostic(instance.clone(), &session, "a", json!({}));
        bus.publish_diagnostic(instance, &session, "b", json!({}));
        assert_eq!(bus.current_sequence(), 2);
        let events = bus.replay(GlobalSequence(1), None).expect("replay");
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].global_sequence, GlobalSequence(1));
        assert_eq!(events[1].global_sequence, GlobalSequence(2));
        assert_eq!(bus.hub().earliest_available(), Some(GlobalSequence(1)));
        assert_eq!(bus.hub().capacity(), 4);
    }

    #[tokio::test]
    async fn command_idempotency_replays_first_response_without_repeating_side_effects() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (store, _) = pawork_storage::session::SessionStore::open(dir.path().join("session.db"))
            .await
            .expect("store");
        let provider = MockProvider::sequence(vec![MockScript::new().wait_for_cancellation()]);
        let core = AppCore::from_parts(
            Arc::new(provider),
            None,
            pawork_domain::ModelId::from("model-1"),
            pawork_domain::ProviderId::from("mock"),
            Some(store),
        );
        let session = core.create_session("idem").await.expect("session");
        let adapter = GuiHostAdapter::new(Arc::new(core));

        let create = command_envelope_with(
            CommandId::from("cmd-create-1"),
            Some("create-once"),
            AppCommand::SessionCreate {
                workspace_id: WorkspaceId::from("ws-default"),
                title: Some("once".into()),
            },
        );
        let first_create = adapter.command(&create).await.expect("create");
        assert!(matches!(first_create, AppResponse::Data(_)));
        let replay_create_id = adapter.command(&create).await.expect("replay command_id");
        assert_eq!(replay_create_id, first_create);
        let replay_create_key = adapter
            .command(&command_envelope_with(
                CommandId::from("cmd-create-2"),
                Some("create-once"),
                AppCommand::SessionCreate {
                    workspace_id: WorkspaceId::from("ws-default"),
                    title: Some("once-again".into()),
                },
            ))
            .await
            .expect("replay idempotency_key");
        assert_eq!(replay_create_key, first_create);
        let snapshot = adapter.snapshot().await.expect("snapshot after create");
        assert_eq!(
            session_titles(&snapshot, "once"),
            1,
            "SessionCreate must not repeat"
        );
        assert_eq!(session_titles(&snapshot, "once-again"), 0);

        let start = command_envelope_with(
            CommandId::from("cmd-run-1"),
            Some("run-once"),
            AppCommand::RunStart {
                session_id: session.clone(),
                user_message: "go".into(),
                model: None,
                provider: None,
                profile: None,
            },
        );
        let first_start = adapter.command(&start).await.expect("run start");
        let AppResponse::Accepted {
            run_id: Some(run), ..
        } = &first_start
        else {
            panic!("run start must report the run id");
        };
        assert_eq!(adapter.runs().active().len(), 1);
        assert!(adapter.runs().contains(run));

        let replay_start_id = adapter.command(&start).await.expect("replay run command_id");
        assert_eq!(replay_start_id, first_start);
        assert_eq!(adapter.runs().active().len(), 1);

        let replay_start_key = adapter
            .command(&command_envelope_with(
                CommandId::from("cmd-run-2"),
                Some("run-once"),
                AppCommand::RunStart {
                    session_id: session,
                    user_message: "go again".into(),
                    model: None,
                    provider: None,
                profile: None,
                },
            ))
            .await
            .expect("replay run idempotency_key");
        assert_eq!(replay_start_key, first_start);
        assert_eq!(
            adapter.runs().active().len(),
            1,
            "RunStart must not spawn a second run"
        );
    }

    fn gui_command_envelope(
        client_id: &str,
        command_id: &str,
        command: AppCommand,
    ) -> AppCommandEnvelope {
        AppCommandEnvelope {
            api_version: API_VERSION,
            command_id: CommandId::from(command_id),
            source: CommandSource::LocalGui {
                client_id: pawork_domain::GuiClientId::from(client_id),
            },
            identity: ActorIdentity::LocalUser {
                actor_id: pawork_domain::ActorId::from(client_id),
                display_name: None,
            },
            expected_revision: None,
            idempotency_key: None,
            issued_at: Timestamp::from_unix_millis(1),
            command,
        }
    }

    #[tokio::test]
    async fn distinct_gui_clients_do_not_collide_on_command_id() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (store, _) = pawork_storage::session::SessionStore::open(dir.path().join("session.db"))
            .await
            .expect("store");
        let provider = MockProvider::sequence(vec![MockScript::new().wait_for_cancellation()]);
        let core = AppCore::from_parts(
            Arc::new(provider),
            None,
            pawork_domain::ModelId::from("model-1"),
            pawork_domain::ProviderId::from("mock"),
            Some(store),
        );
        let session = core.create_session("collide").await.expect("session");
        let adapter = GuiHostAdapter::new(Arc::new(core));

        adapter
            .command(&gui_command_envelope(
                "client-a",
                "gui-cmd-0",
                AppCommand::SessionCreate {
                    workspace_id: WorkspaceId::from("ws-default"),
                    title: Some("from-a".into()),
                },
            ))
            .await
            .expect("create");
        let start = adapter
            .command(&gui_command_envelope(
                "client-a",
                "gui-cmd-1",
                AppCommand::RunStart {
                    session_id: session,
                    user_message: "go".into(),
                    model: None,
                    provider: None,
                profile: None,
                },
            ))
            .await
            .expect("start");
        let AppResponse::Accepted {
            run_id: Some(run_id),
            ..
        } = start
        else {
            panic!("RunStart must report the run id");
        };
        assert!(adapter.runs().contains(&run_id));

        adapter
            .command(&gui_command_envelope(
                "client-b",
                "gui-cmd-0",
                AppCommand::RunCancel {
                    run_id: run_id.clone(),
                },
            ))
            .await
            .expect("cancel");
        assert!(
            !adapter.runs().contains(&run_id),
            "second GUI must not replay the first client's SessionCreate"
        );
    }

    #[tokio::test]
    async fn session_create_command_creates_session() {
        let (core, _dir, _session) = core_with_turn().await;
        let host: Arc<dyn GuiHost> = Arc::new(GuiHostAdapter::new(core));
        let response = host
            .command(&command_envelope(AppCommand::SessionCreate {
                workspace_id: WorkspaceId::from("ws-default"),
                title: Some("from gui".into()),
            }))
            .await
            .expect("accepted");
        let AppResponse::Data(data) = &response else {
            panic!("SessionCreate must return Data: {response:?}");
        };
        assert_eq!(data["title"], json!("from gui"));
        assert_eq!(data["workspace_id"], json!("ws-default"));
        assert!(data["session_id"].as_str().is_some());
        let snapshot = host.snapshot().await.expect("snapshot after create");
        let sessions = snapshot
            .sections
            .iter()
            .find(|section| section.kind == SnapshotSectionKind::SessionTree)
            .and_then(|section| section.data.clone())
            .and_then(|data| data.as_array().cloned())
            .unwrap_or_default();
        assert!(
            sessions.iter().any(|entry| {
                entry.get("title").and_then(Value::as_str) == Some("from gui")
                    && entry.get("workspace_id").and_then(Value::as_str) == Some("ws-default")
            }),
            "SessionCreate must bind workspace_id in the next snapshot: {sessions:?}"
        );
    }

    #[tokio::test]
    async fn session_fork_command_creates_branch_and_switches() {
        let (core, _dir, session) = core_with_turn().await;
        let parent = {
            let events = core
                .store()
                .expect("store")
                .replay_events(&session, 1, 32)
                .await
                .expect("replay");
            events
                .into_iter()
                .next()
                .expect("persisted event")
                .event_id
        };
        let adapter = GuiHostAdapter::new(core);
        let response = adapter
            .command(&command_envelope(AppCommand::SessionFork {
                session_id: session.clone(),
                parent_event_id: parent.clone(),
            }))
            .await
            .expect("fork");
        let AppResponse::Data(data) = response else {
            panic!("SessionFork must return Data: {response:?}");
        };
        assert_eq!(data.get("session_id").and_then(Value::as_str), Some(session.as_str()));
        let branch_id = data
            .get("branch_id")
            .and_then(Value::as_str)
            .expect("branch_id");
        assert!(branch_id.starts_with("fork-"));
        let snapshot = adapter.snapshot().await.expect("snapshot");
        let sessions = snapshot
            .sections
            .iter()
            .find(|section| section.kind == SnapshotSectionKind::SessionTree)
            .and_then(|section| section.data.clone())
            .and_then(|data| data.as_array().cloned())
            .unwrap_or_default();
        let entry = sessions
            .iter()
            .find(|entry| entry.get("session_id").and_then(Value::as_str) == Some(session.as_str()))
            .expect("session in tree");
        assert_eq!(
            entry.get("forked_from_event_id").and_then(Value::as_str),
            Some(parent.as_str())
        );
        let branches = entry
            .get("branches")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert!(
            branches.iter().any(|branch| {
                branch.get("branch_id").and_then(Value::as_str) == Some(branch_id)
                    && branch.get("active").and_then(Value::as_bool) == Some(true)
            }),
            "forked branch must be active: {branches:?}"
        );
        assert!(
            snapshot
                .sections
                .iter()
                .any(|section| section.kind == SnapshotSectionKind::TerminalSessions),
            "snapshot must include TerminalSessions"
        );
    }

    #[tokio::test]
    async fn run_start_second_turn_includes_session_history() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (store, _) = pawork_storage::session::SessionStore::open(dir.path().join("session.db"))
            .await
            .expect("store");
        let provider = MockProvider::sequence(vec![
            MockScript::new().text("pong").complete(),
            MockScript::new().text("BLUE-PINE").complete(),
        ]);
        let core = AppCore::from_parts(
            Arc::new(provider),
            None,
            pawork_domain::ModelId::from("model-1"),
            pawork_domain::ProviderId::from("mock"),
            Some(store),
        );
        let session = core.create_session("history").await.expect("session");
        let db = dir.path().join("session.db");
        let adapter = GuiHostAdapter::new(Arc::new(core));
        let mut events = adapter.subscribe_events();

        async fn wait_completed(
            events: &mut tokio::sync::broadcast::Receiver<AppEventEnvelope>,
            run_id: &RunId,
        ) {
            let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
            loop {
                let envelope = tokio::time::timeout_at(deadline, events.recv())
                    .await
                    .expect("run should complete")
                    .expect("event channel");
                if let AppEvent::RunChanged {
                    run_id: id,
                    state:
                        RunState::Completed | RunState::Failed | RunState::Cancelled | RunState::Interrupted,
                } = &envelope.payload
                {
                    if id == run_id {
                        return;
                    }
                }
            }
        }

        let first = adapter
            .command(&command_envelope(AppCommand::RunStart {
                session_id: session.clone(),
                user_message: "Remember the codeword BLUE-PINE. Reply with exactly one word: pong"
                    .into(),
                model: None,
                provider: None,
                profile: None,
            }))
            .await
            .expect("first run");
        let AppResponse::Accepted {
            run_id: Some(first_run),
            ..
        } = first
        else {
            panic!("first RunStart must report a run id: {first:?}");
        };
        wait_completed(&mut events, &first_run).await;

        let second = adapter
            .command(&command_envelope(AppCommand::RunStart {
                session_id: session.clone(),
                user_message: "What is the codeword? Reply with only the codeword.".into(),
                model: None,
                provider: None,
                profile: None,
            }))
            .await
            .expect("second run");
        let AppResponse::Accepted {
            run_id: Some(second_run),
            ..
        } = second
        else {
            panic!("second RunStart must report a run id: {second:?}");
        };
        wait_completed(&mut events, &second_run).await;
        drop(adapter);

        let (store, _) = pawork_storage::session::SessionStore::open(db)
            .await
            .expect("reopen store");
        let prepared: Vec<u64> = store
            .replay_events(&session, 1, 500)
            .await
            .expect("replay")
            .into_iter()
            .filter_map(|envelope| match envelope.payload {
                AgentEvent::ContextPrepared { message_count, .. } => Some(message_count),
                _ => None,
            })
            .collect();
        assert_eq!(prepared.len(), 2, "two turns should prepare context twice: {prepared:?}");
        assert!(
            prepared[0] >= 1,
            "first turn must send at least the trigger: {prepared:?}"
        );
        assert!(
            prepared[1] >= prepared[0] + 2,
            "second turn must include prior user+assistant+new user, got {prepared:?}"
        );
    }

    #[test]
    fn run_start_provider_hits_requested_channel_not_catalog_first() {
        let catalog = [
            ("opencode-go", "deepseek-v4-flash"),
            ("deepseek", "deepseek-v4-flash"),
        ];
        let first =
            run_start_overview_owner("deepseek-v4-flash", catalog.iter()).expect("catalog first");
        assert_eq!(
            first, "opencode-go",
            "catalog first item would mis-route without RunStart.provider"
        );

        let target = run_start_requested_provider_switch(
            "mock",
            "model-1",
            Some("deepseek"),
            Some("deepseek-v4-flash"),
        );
        assert_eq!(
            target,
            Some(("deepseek".into(), Some("deepseek-v4-flash".into())))
        );
        assert_ne!(
            target.as_ref().map(|(provider, _)| provider.as_str()),
            Some(first.as_str())
        );

        assert_eq!(
            run_start_requested_provider_switch(
                "deepseek",
                "deepseek-v4-flash",
                Some("deepseek"),
                Some("deepseek-v4-flash"),
            ),
            None
        );
        assert_eq!(
            run_start_overview_owner("deepseek-v4-flash", catalog.iter()).as_deref(),
            Some("opencode-go")
        );
    }

    #[tokio::test]
    async fn run_start_with_provider_does_not_silently_keep_same_model_id() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (store, _) = pawork_storage::session::SessionStore::open(dir.path().join("session.db"))
            .await
            .expect("store");
        let core = AppCore::from_parts(
            Arc::new(MockProvider::sequence(vec![
                MockScript::new().text("ok").complete(),
            ])),
            None,
            pawork_domain::ModelId::from("deepseek-v4-flash"),
            pawork_domain::ProviderId::from("deepseek"),
            Some(store),
        );
        let session = core.create_session("provider-pair").await.expect("session");
        let adapter = GuiHostAdapter::new(Arc::new(core));
        let error = adapter
            .command(&command_envelope(AppCommand::RunStart {
                session_id: session,
                user_message: "hi".into(),
                model: Some(pawork_domain::ModelId::from("deepseek-v4-flash")),
                provider: Some(pawork_domain::ProviderId::from("opencode-go")),
                profile: None,
            }))
            .await
            .expect_err("same model id on another channel must not silently accept");
        assert!(
            error.message.contains("opencode-go"),
            "RunStart.provider must target the requested channel, got {}",
            error.message
        );
    }
}
