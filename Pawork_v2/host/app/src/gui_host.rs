//! GUI Host 端口适配：把 AppCore 装配到 pawork-gui-server 的 GuiHost。
//!
//! S7 波 A 最小切片：snapshot 基线（Workspaces/SessionTree/ActiveRuns/
//! PendingToolApprovals/ProviderStatus）、SessionGet 分页 Timeline 投影、
//! SessionCreate/RunStart/RunCancel 命令与事件扇出。审批/模型切换的 GUI
//! 语义在波 C 接线，未支持命令一律结构化 fail-closed。

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use pawork_domain::{
    AgentEvent, AgentEventEnvelope, ApprovalDecision, CancellationToken, ContentPart, EventId,
    Message, MessageId, MessageRole, RunId, SessionId, TextContent,
};
use pawork_engine::{now_timestamp, AgentEventSink, EngineError};
use pawork_gui_server::{GuiHost, GuiHostError};
use pawork_protocol::{
    AppCommand, AppCommandEnvelope, AppEvent, AppEventEnvelope, AppQuery, AppQueryEnvelope,
    AppResponse, EventSource, EventStream, GlobalSequence, RunState, Snapshot, SnapshotSection,
    SnapshotSectionKind, TimelineItem, TimelineItemKind, TimelinePage, API_VERSION,
};
use serde_json::{json, Value};

use crate::AppCore;

/// 单实例事件总线：给 GUI 连接扇出 App 事件，全局序号单调连续。
pub struct GuiEventBus {
    tx: tokio::sync::broadcast::Sender<AppEventEnvelope>,
    global_sequence: AtomicU64,
    revision: AtomicU64,
}

impl GuiEventBus {
    pub fn new(capacity: usize) -> Self {
        let (tx, _) = tokio::sync::broadcast::channel(capacity);
        Self {
            tx,
            global_sequence: AtomicU64::new(0),
            revision: AtomicU64::new(1),
        }
    }

    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<AppEventEnvelope> {
        self.tx.subscribe()
    }

    fn next_global_sequence(&self) -> u64 {
        self.global_sequence.fetch_add(1, Ordering::Relaxed) + 1
    }

    pub fn current_sequence(&self) -> u64 {
        self.global_sequence.load(Ordering::Relaxed)
    }

    fn next_revision(&self) -> u64 {
        self.revision.fetch_add(1, Ordering::Relaxed)
    }

    fn publish(
        &self,
        instance: pawork_domain::CoreInstanceId,
        envelope: &AgentEventEnvelope,
        event: AppEvent,
    ) {
        let sequence = self.next_global_sequence();
        let app_envelope = AppEventEnvelope {
            api_version: API_VERSION,
            instance_id: instance,
            event_id: EventId::from(format!("app-evt-{sequence}")),
            global_sequence: GlobalSequence(sequence),
            stream: EventStream::Session(envelope.session_id.clone()),
            stream_sequence: envelope.sequence.0,
            timestamp: envelope.timestamp,
            source: EventSource::Core,
            payload: event,
        };
        // 无订阅者或队列满时丢弃：S7 单客户端重连可重新 Snapshot，
        // 不允许慢消费反压 Core。
        let _ = self.tx.send(app_envelope);
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
    core: Arc<AppCore>,
    bus: Arc<GuiEventBus>,
    runs: Arc<GuiRunRegistry>,
    instance: pawork_domain::CoreInstanceId,
    next_gui_run: AtomicU64,
}

impl GuiHostAdapter {
    pub fn new(core: Arc<AppCore>) -> Self {
        let stamp = now_timestamp().as_unix_millis();
        let instance = pawork_domain::CoreInstanceId::from(format!(
            "pawork-{stamp}-{}",
            std::process::id()
        ));
        Self {
            core,
            bus: Arc::new(GuiEventBus::new(1024)),
            runs: Arc::new(GuiRunRegistry::new()),
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

    fn host_error(code: &str, message: impl Into<String>) -> GuiHostError {
        GuiHostError {
            code: code.to_string(),
            message: message.into(),
            retryable: false,
        }
    }

    fn app_error(error: crate::AppError) -> GuiHostError {
        Self::host_error("app_error", error.to_string())
    }
}

#[async_trait]
impl GuiHost for GuiHostAdapter {
    fn instance_id(&self) -> pawork_domain::CoreInstanceId {
        self.instance.clone()
    }

    async fn snapshot(&self) -> Result<Snapshot, GuiHostError> {
        let sessions = self.core.list_sessions().await.map_err(Self::app_error)?;
        let runs = self.runs.active();
        let provider_status = if self.core.provider_pending() {
            "authentication_required"
        } else {
            "ready"
        };
        let sections = vec![
            SnapshotSection {
                kind: SnapshotSectionKind::Workspaces,
                revision: self.bus.next_revision(),
                data: Some(json!([{
                    "id": self.core.workspace_id().as_str(),
                    "trusted": self.core.workspace_trusted(),
                }])),
                artifact_id: None,
            },
            SnapshotSection {
                kind: SnapshotSectionKind::SessionTree,
                revision: self.bus.next_revision(),
                data: Some(Value::Array(
                    sessions
                        .iter()
                        .map(|record| {
                            json!({
                                "session_id": record.session_id,
                                "title": record.title,
                                "created_at_ms": record.created_at_ms,
                                "updated_at_ms": record.updated_at_ms,
                                "active_branch": record.active_branch,
                                "archived": record.archived,
                            })
                        })
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
                data: Some(json!([])),
                artifact_id: None,
            },
            SnapshotSection {
                kind: SnapshotSectionKind::ProviderStatus,
                revision: self.bus.next_revision(),
                data: Some(json!([{
                    "provider_id": self.core.provider_id().as_str(),
                    "model": self.core.model().as_str(),
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
        let head = self
            .core
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
        let store = self.core.store().map_err(Self::app_error)?;
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
            AppQuery::WorkspaceList => Ok(AppResponse::Data(json!([{
                "id": self.core.workspace_id().as_str(),
                "trusted": self.core.workspace_trusted(),
            }]))),
            AppQuery::SessionGet {
                session_id,
                timeline_after_sequence,
                timeline_limit,
            } => {
                let record = self
                    .core
                    .get_session(session_id)
                    .await
                    .map_err(Self::app_error)?;
                let mut data = json!({
                    "session_id": record.session_id,
                    "title": record.title,
                    "created_at_ms": record.created_at_ms,
                    "updated_at_ms": record.updated_at_ms,
                    "active_branch": record.active_branch,
                });
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
                let catalog = self.core.model_catalog().await;
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
        match &envelope.command {
            AppCommand::SessionCreate { title, .. } => {
                self.core
                    .create_session(title.clone().unwrap_or_else(|| "New session".into()))
                    .await
                    .map_err(Self::app_error)?;
                Ok(AppResponse::Accepted {
                    command_id: envelope.command_id.clone(),
                    run_id: None,
                })
            }
            AppCommand::SessionOpen { session_id } => {
                self.core
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
                if let Some(model) = model {
                    if model.as_str() != self.core.model().as_str() {
                        return Err(Self::host_error(
                            "unsupported",
                            "GUI model switching arrives in wave C; this run would not use the requested model",
                        ));
                    }
                }
                self.core
                    .get_session(session_id)
                    .await
                    .map_err(Self::app_error)?;
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
                    let _ = core
                        .chat_turn_with_run_id(run.clone(), &session, vec![message], &sink, token)
                        .await;
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
            other => Err(Self::host_error(
                "unsupported",
                format!(
                    "command {} is not part of the S7 wave A slice",
                    command_name(other)
                ),
            )),
        }
    }

    fn subscribe_events(&self) -> tokio::sync::broadcast::Receiver<AppEventEnvelope> {
        self.bus.subscribe()
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
        AgentEvent::ToolApprovalRequested {
            tool_call_id,
            reason,
        } => AppEvent::ToolApprovalRequired {
            run_id: run,
            tool_call_id: tool_call_id.clone(),
            reason: reason.clone(),
        },
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
    use pawork_domain::{CommandId, MessageMetadata, Timestamp, WorkspaceId};
    use pawork_protocol::{ActorIdentity, CommandSource};
    use pawork_testkit::{MockProvider, MockScript};

    struct NoopSink;

    #[async_trait]
    impl AgentEventSink for NoopSink {
        async fn emit(&self, _envelope: AgentEventEnvelope) -> Result<(), EngineError> {
            Ok(())
        }
    }

    fn command_envelope(command: AppCommand) -> AppCommandEnvelope {
        AppCommandEnvelope {
            api_version: API_VERSION,
            command_id: CommandId::from("cmd-test-1"),
            source: CommandSource::Automation,
            identity: ActorIdentity::System,
            expected_revision: None,
            idempotency_key: None,
            issued_at: Timestamp::from_unix_millis(1),
            command,
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
        let core = Arc::new(AppCore::from_parts(
            Arc::new(provider),
            None,
            pawork_domain::ModelId::from("model-1"),
            pawork_domain::ProviderId::from("mock"),
            Some(store),
        ));
        let session = core.create_session("gui-cancel").await.expect("session");
        let adapter = GuiHostAdapter::new(Arc::clone(&core));
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
    async fn run_start_with_foreign_model_fails_closed() {
        let (core, _dir, session) = core_with_turn().await;
        let host: Arc<dyn GuiHost> = Arc::new(GuiHostAdapter::new(core));
        let error = host
            .command(&command_envelope(AppCommand::RunStart {
                session_id: session,
                user_message: "hi".into(),
                model: Some(pawork_domain::ModelId::from("other-model")),
                profile: None,
            }))
            .await
            .expect_err("foreign model must fail closed");
        assert_eq!(error.code, "unsupported");
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
    }
}
