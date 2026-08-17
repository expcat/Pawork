//! GUI Host 端口适配：把 AppCore 装配到 pawork-gui-server 的 GuiHost。
//!
//! S7 波 C：snapshot 基线（含真实 PendingToolApprovals）、SessionGet 分页
//! Timeline 投影、SessionCreate/RunStart/RunCancel/ToolApprove，以及
//! RunStart.model 切换。未支持命令一律结构化 fail-closed。

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use pawork_domain::{
    AgentEvent, AgentEventEnvelope, ApprovalDecision, CancellationToken, ContentPart, EventId,
    Message, MessageId, MessageRole, QueryId, RunId, SessionId, TenantId, TextContent, WorkspaceId,
};
use pawork_session::SessionRecord;
use pawork_engine::{now_timestamp, AgentEventSink, EngineError};
use pawork_gui_server::{GuiHost, GuiHostError};
use pawork_protocol::{
    AppCommand, AppCommandEnvelope, AppEvent, AppEventEnvelope, AppQuery, AppQueryEnvelope,
    AppResponse, AppResponseEnvelope, DiagnosticLevel, EventSource, EventStream, GlobalSequence,
    RunState, Snapshot, SnapshotSection, SnapshotSectionKind, TimelineItem, TimelineItemKind,
    TimelinePage, API_VERSION, DEFAULT_CONTROL_PLANE_TENANT,
};
use serde_json::{json, Value};

use crate::{
    should_cache, AppCore, EventHub, GuiApprovalHost, HubError, IdempotencyCheck, IdempotencyStore,
    DEFAULT_HUB_CAPACITY,
};

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
        let runs = self.runs.lock().expect("gui run registry poisoned");
        match runs.get(run_id.as_str()) {
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

/// pawork-gui-server 的宿主实现。
pub struct GuiHostAdapter {
    core: Arc<tokio::sync::RwLock<AppCore>>,
    bus: Arc<GuiEventBus>,
    runs: Arc<GuiRunRegistry>,
    approvals: Arc<GuiApprovalHost>,
    idempotency: IdempotencyStore,
    instance: pawork_domain::CoreInstanceId,
    next_gui_run: AtomicU64,
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
                data: Some(Value::Array(
                    sessions
                        .iter()
                        .map(|record| session_tree_entry(record, core.session_workspace_for_record(&record.session_id)))
                        .collect(),
                )),
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
        let envelopes = store
            .replay_events(session_id, from, limit)
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
                Ok(AppResponse::Data(json!([{
                    "id": core.workspace_id().as_str(),
                    "name": core.workspace_name(),
                    "trusted": core.workspace_trusted(),
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
                    query_name(other)
                ),
            )),
        }
    }

    async fn command(&self, envelope: &AppCommandEnvelope) -> Result<AppResponse, GuiHostError> {
        let tenant = TenantId::new(DEFAULT_CONTROL_PLANE_TENANT);
        match self.idempotency.check(
            &tenant,
            &envelope.command_id,
            envelope.idempotency_key.as_deref(),
        ) {
            IdempotencyCheck::Replay(cached) => return Ok(cached.response),
            IdempotencyCheck::New => {}
        }
        let response = self.dispatch_command(envelope).await?;
        let cached = AppResponseEnvelope {
            api_version: envelope.api_version,
            request_id: QueryId::from(envelope.command_id.as_str()),
            responded_at: now_timestamp(),
            response: response.clone(),
        };
        if should_cache(&cached) {
            let _ = self.idempotency.record(
                &tenant,
                &envelope.command_id,
                envelope.idempotency_key.as_deref(),
                cached,
            );
        }
        Ok(response)
    }

    fn subscribe_events(&self) -> tokio::sync::broadcast::Receiver<AppEventEnvelope> {
        self.bus.subscribe()
    }
}

impl GuiHostAdapter {
    async fn dispatch_command(
        &self,
        envelope: &AppCommandEnvelope,
    ) -> Result<AppResponse, GuiHostError> {
        match &envelope.command {
            AppCommand::SessionCreate {
                title,
                workspace_id,
            } => {
                self.core
                    .read()
                    .await
                    .create_session_with_workspace(
                        title.clone().unwrap_or_else(|| "New session".into()),
                        workspace_id.clone(),
                    )
                    .await
                    .map_err(Self::app_error)?;
                Ok(AppResponse::Accepted {
                    command_id: envelope.command_id.clone(),
                    run_id: None,
                })
            }
            AppCommand::SessionOpen { session_id } => {
                self.core
                    .read()
                    .await
                    .get_session(session_id)
                    .await
                    .map_err(Self::app_error)?;
                Ok(AppResponse::Accepted {
                    command_id: envelope.command_id.clone(),
                    run_id: None,
                })
            }
            AppCommand::RunStart {
                session_id,
                user_message,
                model,
                ..
            } => {
                {
                    let core = self.core.read().await;
                    core.get_session(session_id)
                        .await
                        .map_err(Self::app_error)?;
                }
                if let Some(model) = model {
                    let current = {
                        let core = self.core.read().await;
                        (
                            core.provider_id().as_str().to_string(),
                            core.model().as_str().to_string(),
                        )
                    };
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
                let message = Message {
                    id: MessageId::from("pending"),
                    role: MessageRole::User,
                    content: vec![ContentPart::Text(TextContent {
                        text: user_message.clone(),
                    })],
                    metadata: Default::default(),
                };
                tokio::spawn(async move {
                    let sink = GuiBroadcastSink::new(bus, instance);
                    let core = core.read().await;
                    let _ = core
                        .chat_turn_with_run_id(run.clone(), &session, vec![message], &sink, token)
                        .await;
                    drop(core);
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
            other => Err(Self::host_error(
                "unsupported",
                format!(
                    "command {} is not part of the S7 wave C slice",
                    command_name(other)
                ),
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
            None,
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
        AgentEvent::RunFailed { error } => (
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

fn query_name(query: &AppQuery) -> &'static str {
    match query {
        AppQuery::WorkspaceList => "workspace_list",
        AppQuery::SessionGet { .. } => "session_get",
        AppQuery::RunStatus { .. } => "run_status",
        AppQuery::ModelList { .. } => "model_list",
        AppQuery::DiffListFiles { .. } => "diff_list_files",
        AppQuery::DiffGet { .. } => "diff_get",
        AppQuery::ArtifactRead { .. } => "artifact_read",
        AppQuery::QuotaOverview { .. } => "quota_overview",
        AppQuery::SnapshotFetch => "snapshot_fetch",
        AppQuery::PluginList => "plugin_list",
        AppQuery::McpList => "mcp_list",
    }
}

fn command_name(command: &AppCommand) -> &'static str {
    match command {
        AppCommand::CoreInitialize => "core_initialize",
        AppCommand::WorkspaceAdd { .. } => "workspace_add",
        AppCommand::WorkspaceTrust { .. } => "workspace_trust",
        AppCommand::SessionCreate { .. } => "session_create",
        AppCommand::SessionOpen { .. } => "session_open",
        AppCommand::SessionFork { .. } => "session_fork",
        AppCommand::SessionCompact { .. } => "session_compact",
        AppCommand::SessionClientContextReplace { .. } => "session_client_context_replace",
        AppCommand::RunStart { .. } => "run_start",
        AppCommand::RunCancel { .. } => "run_cancel",
        AppCommand::RunRetry { .. } => "run_retry",
        AppCommand::RunTool { .. } => "run_tool",
        AppCommand::AuthStart { .. } => "auth_start",
        AppCommand::AuthRemove { .. } => "auth_remove",
        AppCommand::ToolApprove { .. } => "tool_approve",
        AppCommand::GitStage { .. } => "git_stage",
        AppCommand::TerminalCreate { .. } => "terminal_create",
        AppCommand::TerminalWrite { .. } => "terminal_write",
        AppCommand::TerminalResize { .. } => "terminal_resize",
    }
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
        let (store, _) = pawork_session::SessionStore::open(dir.path().join("session.db"))
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
            SnapshotSectionKind::ProviderStatus,
        ] {
            assert!(kinds.contains(&expected), "missing section {expected:?}");
        }
        assert_eq!(snapshot.instance_id, adapter.instance_id());
    }

    #[tokio::test]
    async fn run_start_reports_run_and_registry_drains_after_completion() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (store, _) = pawork_session::SessionStore::open(dir.path().join("session.db"))
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
        let (store, _) = pawork_session::SessionStore::open(dir.path().join("session.db"))
            .await
            .expect("store");
        let provider = MockProvider::sequence(vec![
            MockScript::new().text("hello from other").complete(),
        ])
        .with_models(vec![pawork_api::ModelDefinition {
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
        let (store, _) = pawork_session::SessionStore::open(dir.path().join("session.db"))
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
        assert_eq!(registry.active().len(), 1);
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
        let (store, _) = pawork_session::SessionStore::open(dir.path().join("session.db"))
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
        assert!(matches!(first_create, AppResponse::Accepted { .. }));
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
        assert!(matches!(response, AppResponse::Accepted { .. }));
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
}
