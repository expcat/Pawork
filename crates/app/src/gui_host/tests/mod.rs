pub(super) use super::*;
use crate::approval::ApprovalPromptHost;
use pawork_domain::{
    ApprovalDecision, CancellationToken, CommandId, ContentPart, Message, MessageId,
    MessageMetadata, MessageRole, RunId, TextContent, Timestamp,
};
use pawork_protocol::app::registry::{command_entries, query_entries};
use pawork_protocol::{
    ActorIdentity, AppEvent, AppQuery, CommandSource, EventStream, RunState, TimelineItemKind,
};
use pawork_testkit::{MockProvider, MockScript};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicU64, Ordering};


mod approval;
mod idempotency;
mod run;
mod session;
mod settings;
mod terminal;

pub(super) struct NoopSink;

#[async_trait]
impl AgentEventSink for NoopSink {
    async fn emit(&self, _envelope: AgentEventEnvelope) -> Result<(), EngineError> {
        Ok(())
    }
}

pub(super) fn next_test_command_id() -> CommandId {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    CommandId::from(format!("cmd-test-{}", NEXT.fetch_add(1, Ordering::Relaxed)))
}

pub(super) fn command_envelope(command: AppCommand) -> AppCommandEnvelope {
    command_envelope_with(next_test_command_id(), None, command)
}

pub(super) fn command_envelope_with(
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

pub(super) fn session_titles(snapshot: &Snapshot, title: &str) -> usize {
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

pub(super) fn query_envelope(query: AppQuery) -> AppQueryEnvelope {
    AppQueryEnvelope {
        api_version: API_VERSION,
        request_id: pawork_domain::QueryId::from("query-test-1"),
        source: CommandSource::Automation,
        identity: ActorIdentity::System,
        issued_at: Timestamp::from_unix_millis(1),
        query,
    }
}

#[test]
fn dispatch_tables_match_gui_available_registry_entries() {
    let dispatch_commands: Vec<_> = COMMAND_HANDLERS
        .iter()
        .map(|(wire_name, _)| *wire_name)
        .collect();
    let registry_commands: Vec<_> = command_entries()
        .iter()
        .filter(|entry| entry.gui.available)
        .map(|entry| entry.wire_name)
        .collect();
    assert_eq!(dispatch_commands, registry_commands);

    let dispatch_queries: Vec<_> = QUERY_HANDLERS
        .iter()
        .map(|(wire_name, _)| *wire_name)
        .collect();
    let registry_queries: Vec<_> = query_entries()
        .iter()
        .filter(|entry| entry.gui.available)
        .map(|entry| entry.wire_name)
        .collect();
    assert_eq!(dispatch_queries, registry_queries);
}

pub(super) async fn core_with_turn() -> (Arc<AppCore>, tempfile::TempDir, SessionId) {
    let dir = tempfile::tempdir().expect("tempdir");
    let (store, _) = pawork_storage::session::SessionStore::open(dir.path().join("session.db"))
        .await
        .expect("store");
    let provider =
        MockProvider::sequence(vec![MockScript::new().text("hello from mock").complete()]);
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
        content: vec![ContentPart::Text(TextContent { text: "hi".into() })],
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
    let page = adapter
        .timeline(&session, None, Some(500))
        .await
        .expect("page");
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
        .query(&query_envelope(AppQuery::ModelList {
            provider_id: None,
            include_disabled: false,
        }))
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
    for expected in [
        "xai",
        "glm-coding",
        "opencode-go",
        "qwen-token-plan",
        "deepseek",
    ] {
        assert!(
            providers.contains(expected),
            "ModelList must include {expected}: {providers:?}"
        );
    }
}

#[tokio::test]
async fn mcp_list_returns_servers_array_shape() {
    let (core, _dir, _session) = core_with_turn().await;
    let host: Arc<dyn GuiHost> = Arc::new(GuiHostAdapter::new(core));
    let response = host
        .query(&query_envelope(AppQuery::McpList))
        .await
        .expect("mcp list");
    let AppResponse::Data(data) = response else {
        panic!("mcp list must return data");
    };
    let servers = data
        .get("servers")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("mcp list must carry a servers array: {data:?}"));
    assert!(
        servers.is_empty(),
        "harness assembles no MCP servers: {servers:?}"
    );
}

pub(super) async fn wait_run_completed(
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
            state: RunState::Completed | RunState::Failed | RunState::Cancelled,
        } = &envelope.payload
        {
            if id == run_id {
                return;
            }
        }
    }
}

pub(super) async fn wait_run_registry_drains(runs: &GuiRunRegistry, run: &RunId) {
    for _ in 0..500 {
        if !runs.contains(run) {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("registry must drain after the run finishes");
}

pub(super) async fn wait_host_run_task_settled(
    adapter: &GuiHostAdapter,
    events: &mut tokio::sync::broadcast::Receiver<AppEventEnvelope>,
    run: &RunId,
) -> Vec<AppEvent> {
    // 先等到该 run 的终态上流（RunCancel 不经过 registry 摘除语义，
    // 只能靠事件观察），再等宿主 task 末尾的 terminal 登记清理 ——
    // 清理发生在所有兜底 publish 之后，读到 false 即 task 已收尾。
    // 已消费的事件要带回给断言（终态本身就在其中）。
    let mut seen = Vec::new();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let envelope = tokio::time::timeout_at(deadline, events.recv())
            .await
            .expect("run should reach a terminal state")
            .expect("event channel");
        let terminal_for_run = matches!(
            &envelope.payload,
            AppEvent::RunChanged {
                run_id: id,
                state: RunState::Completed | RunState::Failed | RunState::Cancelled,
            } if id == run
        );
        seen.push(envelope.payload);
        if terminal_for_run {
            break;
        }
    }
    for _ in 0..500 {
        if !adapter.bus.terminal_reported(run.as_str()) {
            return seen;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("host run task must settle after the terminal event");
}

pub(super) fn drain_wire_events(
    events: &mut tokio::sync::broadcast::Receiver<AppEventEnvelope>,
) -> Vec<AppEvent> {
    drain_wire_envelopes(events)
        .into_iter()
        .map(|envelope| envelope.payload)
        .collect()
}

pub(super) fn drain_wire_envelopes(
    events: &mut tokio::sync::broadcast::Receiver<AppEventEnvelope>,
) -> Vec<AppEventEnvelope> {
    let mut wire = Vec::new();
    loop {
        match events.try_recv() {
            Ok(envelope) => wire.push(envelope),
            Err(tokio::sync::broadcast::error::TryRecvError::Lagged(missed)) => {
                panic!("subscriber lagged: {missed}");
            }
            Err(
                tokio::sync::broadcast::error::TryRecvError::Empty
                | tokio::sync::broadcast::error::TryRecvError::Closed,
            ) => break,
        }
    }
    wire
}

pub(super) fn terminal_states_for(wire: &[AppEvent], run: &RunId) -> Vec<RunState> {
    wire.iter()
        .filter_map(|event| match event {
            AppEvent::RunChanged { run_id, state }
                if run_id == run
                    && matches!(
                        state,
                        RunState::Completed
                            | RunState::Cancelled
                            | RunState::Failed
                            | RunState::Interrupted
                    ) =>
            {
                Some(state.clone())
            }
            _ => None,
        })
        .collect()
}

pub(super) fn has_run_failed_diagnostic(wire: &[AppEvent]) -> bool {
    wire.iter()
        .any(|event| matches!(event, AppEvent::Diagnostic { code, .. } if code == "run.failed"))
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

#[test]
fn gui_event_bus_publishes_lagged_degrade_frame() {
    let bus = GuiEventBus::new(4);
    let instance = pawork_domain::CoreInstanceId::from("instance-1");
    let mut subscription = bus.subscribe();
    bus.publish_event_stream_lagged(instance, Some(3), Some("gui-1"));
    let event = subscription.try_recv().expect("lagged degrade");
    match event.payload {
        AppEvent::Diagnostic {
            level,
            code,
            message,
        } => {
            assert_eq!(level, pawork_protocol::DiagnosticLevel::Warning);
            assert_eq!(code, "degrade.event_stream_lagged");
            assert_eq!(message, "event stream subscriber lagged");
        }
        other => panic!("expected lagged diagnostic, got {other:?}"),
    }
    assert_eq!(event.stream, EventStream::Global);
    assert_eq!(bus.current_sequence(), 1);
}
