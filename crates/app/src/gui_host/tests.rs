use super::*;
use crate::approval::ApprovalPromptHost;
use pawork_domain::{
    ApprovalDecision, CancellationToken, CommandId, ContentPart, Message, MessageId,
    MessageMetadata, MessageRole, QueryId, RunId, TenantId, TextContent, Timestamp, WorkspaceId,
};
use pawork_protocol::app::registry::{command_entries, query_entries};
use pawork_protocol::{
    ActorIdentity, AppEvent, AppQuery, AppResponseEnvelope, CommandSource, EventStream, RunState,
    TimelineItemKind, DEFAULT_CONTROL_PLANE_TENANT,
};
use pawork_testkit::{MockProvider, MockScript};
use std::sync::atomic::{AtomicU64, Ordering};

struct NoopSink;

#[async_trait]
impl AgentEventSink for NoopSink {
    async fn emit(&self, _envelope: AgentEventEnvelope) -> Result<(), EngineError> {
        Ok(())
    }
}

fn next_test_command_id() -> CommandId {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    CommandId::from(format!("cmd-test-{}", NEXT.fetch_add(1, Ordering::Relaxed)))
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

async fn core_with_turn() -> (Arc<AppCore>, tempfile::TempDir, SessionId) {
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

async fn wait_run_completed(
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

async fn wait_run_registry_drains(runs: &GuiRunRegistry, run: &RunId) {
    for _ in 0..500 {
        if !runs.contains(run) {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("registry must drain after the run finishes");
}

async fn wait_host_run_task_settled(
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

fn drain_wire_events(
    events: &mut tokio::sync::broadcast::Receiver<AppEventEnvelope>,
) -> Vec<AppEvent> {
    drain_wire_envelopes(events)
        .into_iter()
        .map(|envelope| envelope.payload)
        .collect()
}

fn drain_wire_envelopes(
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

fn terminal_states_for(wire: &[AppEvent], run: &RunId) -> Vec<RunState> {
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

fn has_run_failed_diagnostic(wire: &[AppEvent]) -> bool {
    wire.iter()
        .any(|event| matches!(event, AppEvent::Diagnostic { code, .. } if code == "run.failed"))
}

#[tokio::test]
async fn run_start_expands_at_refs_into_separate_parts() {
    let workspace = tempfile::tempdir().expect("workspace");
    std::fs::write(
        workspace.path().join("ROADMAP.md"),
        "phase R8 wave D wiring\n",
    )
    .expect("roadmap");
    let dir = tempfile::tempdir().expect("store");
    let (store, _) = pawork_storage::session::SessionStore::open(dir.path().join("session.db"))
        .await
        .expect("store");
    let provider = MockProvider::sequence(vec![MockScript::new().text("ok").complete()]);
    let mut core = AppCore::from_parts(
        Arc::new(provider),
        None,
        pawork_domain::ModelId::from("model-1"),
        pawork_domain::ProviderId::from("mock"),
        Some(store),
    );
    core.attach_workspace(workspace.path())
        .expect("attach workspace");
    core.prime_extensions().await.expect("prime extensions");
    let session = core.create_session("at-ref").await.expect("session");
    let adapter = GuiHostAdapter::new(Arc::new(core));
    let mut events = adapter.subscribe_events();
    let response = adapter
        .command(&command_envelope(AppCommand::RunStart {
            session_id: session.clone(),
            user_message: "请展开 @ROADMAP 后回答".into(),
            model: None,
            provider: None,
            profile: None,
        }))
        .await
        .expect("run accepted");
    let AppResponse::Accepted {
        run_id: Some(run_id),
        ..
    } = response
    else {
        panic!("RunStart must be accepted: {response:?}");
    };
    wait_run_completed(&mut events, &run_id).await;
    let (store, _) = pawork_storage::session::SessionStore::open(dir.path().join("session.db"))
        .await
        .expect("reopen store");
    let messages = store
        .projection_snapshot(&session)
        .await
        .expect("projection snapshot")
        .messages;
    let user = messages
        .iter()
        .rev()
        .find(|message| message.role == MessageRole::User)
        .expect("user message persisted");
    assert_eq!(user.content.len(), 2, "{:?}", user.content);
    match &user.content[0] {
        ContentPart::Text(text) => assert_eq!(text.text, "请展开 @ROADMAP 后回答"),
        other => panic!("first part must be the user text: {other:?}"),
    }
    match &user.content[1] {
        ContentPart::Text(text) => {
            assert!(text.text.contains("ROADMAP.md"), "{:?}", text.text);
            assert!(
                text.text.contains("phase R8 wave D wiring"),
                "{:?}",
                text.text
            );
        }
        other => panic!("second part must be the attachment: {other:?}"),
    }
}

#[tokio::test]
async fn run_start_expand_at_refs_failure_does_not_leave_active_run() {
    let workspace = tempfile::tempdir().expect("workspace");
    std::fs::write(
        workspace.path().join("ROADMAP.md"),
        "phase R8 wave D wiring\n",
    )
    .expect("roadmap");
    let dir = tempfile::tempdir().expect("store");
    let (store, _) = pawork_storage::session::SessionStore::open(dir.path().join("session.db"))
        .await
        .expect("store");
    let provider = MockProvider::sequence(vec![MockScript::new().text("ok").complete()]);
    let mut core = AppCore::from_parts(
        Arc::new(provider),
        None,
        pawork_domain::ModelId::from("model-1"),
        pawork_domain::ProviderId::from("mock"),
        Some(store),
    );
    core.attach_workspace(workspace.path())
        .expect("attach workspace");
    core.prime_extensions().await.expect("prime extensions");
    std::fs::remove_file(workspace.path().join("ROADMAP.md")).expect("remove indexed file");
    std::fs::create_dir(workspace.path().join("ROADMAP.md")).expect("replace with directory");
    let session = core.create_session("at-ref-fail").await.expect("session");
    let adapter = GuiHostAdapter::new(Arc::new(core));
    let error = adapter
        .command(&command_envelope(AppCommand::RunStart {
            session_id: session.clone(),
            user_message: "请展开 @ROADMAP 后回答".into(),
            model: None,
            provider: None,
            profile: None,
        }))
        .await
        .expect_err("stale @file must fail closed");
    assert_eq!(error.code, "app_error", "{error:?}");
    assert_eq!(
        adapter.runs().active().len(),
        0,
        "failed expand must not leave a ghost run"
    );
}

#[tokio::test]
async fn run_start_without_at_token_passes_single_text_part() {
    let dir = tempfile::tempdir().expect("store");
    let (store, _) = pawork_storage::session::SessionStore::open(dir.path().join("session.db"))
        .await
        .expect("store");
    let provider = MockProvider::sequence(vec![MockScript::new().text("ok").complete()]);
    let core = AppCore::from_parts(
        Arc::new(provider),
        None,
        pawork_domain::ModelId::from("model-1"),
        pawork_domain::ProviderId::from("mock"),
        Some(store),
    );
    let session = core.create_session("plain-turn").await.expect("session");
    let adapter = GuiHostAdapter::new(Arc::new(core));
    let mut events = adapter.subscribe_events();
    let response = adapter
        .command(&command_envelope(AppCommand::RunStart {
            session_id: session.clone(),
            user_message: "plain turn without refs".into(),
            model: None,
            provider: None,
            profile: None,
        }))
        .await
        .expect("run accepted");
    let AppResponse::Accepted {
        run_id: Some(run_id),
        ..
    } = response
    else {
        panic!("RunStart must be accepted: {response:?}");
    };
    wait_run_completed(&mut events, &run_id).await;
    let (store, _) = pawork_storage::session::SessionStore::open(dir.path().join("session.db"))
        .await
        .expect("reopen store");
    let messages = store
        .projection_snapshot(&session)
        .await
        .expect("projection snapshot")
        .messages;
    let user = messages
        .iter()
        .rev()
        .find(|message| message.role == MessageRole::User)
        .expect("user message persisted");
    assert_eq!(user.content.len(), 1, "{:?}", user.content);
    match &user.content[0] {
        ContentPart::Text(text) => assert_eq!(text.text, "plain turn without refs"),
        other => panic!("single part must be the user text: {other:?}"),
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
    let expected = dir
        .path()
        .canonicalize()
        .unwrap_or_else(|_| dir.path().to_path_buf());
    let listed_path = std::path::PathBuf::from(listed);
    let listed_canon = listed_path.canonicalize().unwrap_or(listed_path);
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
    let provider = MockProvider::sequence(vec![MockScript::new().wait_for_cancellation()]);
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
    let AppResponse::Accepted {
        run_id: Some(run), ..
    } = response
    else {
        panic!("run start must report the run id");
    };
    assert!(runs.contains(&run));

    let cancel_response = host
        .command(&command_envelope(AppCommand::RunCancel {
            run_id: run.clone(),
        }))
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
async fn run_start_provider_failure_broadcasts_single_terminal_without_synthetic() {
    // engine fail 路径：Err 返回前已经 sink 广播 RunChanged{Failed}，
    // 宿主不得再补发合成终态对（幽灵 "Run failed" + run.failed）。
    let dir = tempfile::tempdir().expect("tempdir");
    let (store, _) = pawork_storage::session::SessionStore::open(dir.path().join("session.db"))
        .await
        .expect("store");
    let provider = MockProvider::sequence(vec![MockScript::new().fail(
        pawork_domain::ProviderError::new(
            pawork_domain::ProviderErrorKind::Timeout,
            "scripted timeout",
        ),
    )]);
    let core = AppCore::from_parts(
        Arc::new(provider),
        None,
        pawork_domain::ModelId::from("model-1"),
        pawork_domain::ProviderId::from("mock"),
        Some(store),
    );
    let session = core
        .create_session("gui-fail-terminal")
        .await
        .expect("session");
    let adapter = GuiHostAdapter::new(Arc::new(core));
    let runs = adapter.runs();
    let mut events = adapter.subscribe_events();
    let response = adapter
        .command(&command_envelope(AppCommand::RunStart {
            session_id: session.clone(),
            user_message: "fail this turn".into(),
            model: None,
            provider: None,
            profile: None,
        }))
        .await
        .expect("run accepted");
    let AppResponse::Accepted {
        run_id: Some(run), ..
    } = response
    else {
        panic!("RunStart must be accepted: {response:?}");
    };
    wait_run_registry_drains(&runs, &run).await;
    let wire = drain_wire_events(&mut events);
    assert_eq!(
        terminal_states_for(&wire, &run),
        vec![RunState::Failed],
        "engine failure must broadcast exactly one terminal RunChanged: {wire:?}"
    );
    assert!(
        !has_run_failed_diagnostic(&wire),
        "host must not synthesize a duplicate run.failed after the engine terminal: {wire:?}"
    );
}

#[tokio::test]
async fn run_start_cancel_broadcasts_cancelled_without_synthetic_failed() {
    // cancel 路径：engine 广播 RunChanged{Cancelled} 后以 Err 收尾，
    // 宿主不得谎报合成 RunChanged{Failed}。
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
    let session = core
        .create_session("gui-cancel-terminal")
        .await
        .expect("session");
    let adapter = GuiHostAdapter::new(Arc::new(core));
    let mut events = adapter.subscribe_events();
    let response = adapter
        .command(&command_envelope(AppCommand::RunStart {
            session_id: session.clone(),
            user_message: "cancel this turn".into(),
            model: None,
            provider: None,
            profile: None,
        }))
        .await
        .expect("run accepted");
    let AppResponse::Accepted {
        run_id: Some(run), ..
    } = response
    else {
        panic!("RunStart must be accepted: {response:?}");
    };
    let cancel_response = adapter
        .command(&command_envelope(AppCommand::RunCancel {
            run_id: run.clone(),
        }))
        .await
        .expect("cancel accepted");
    assert!(matches!(cancel_response, AppResponse::Accepted { .. }));
    let mut wire = wait_host_run_task_settled(&adapter, &mut events, &run).await;
    wire.extend(drain_wire_events(&mut events));
    assert_eq!(
        terminal_states_for(&wire, &run),
        vec![RunState::Cancelled],
        "cancel must broadcast exactly one terminal RunChanged{{Cancelled}}: {wire:?}"
    );
    assert!(
        !has_run_failed_diagnostic(&wire),
        "host must not misreport a cancelled run as synthetic failed: {wire:?}"
    );
}

#[tokio::test]
async fn run_start_early_death_without_terminal_still_synthesizes_failed() {
    // 无终态早死路径：Draft plan 使 chat_turn 在 run_session 之前被闸门拒绝，
    // engine 未报任何终态 —— 宿主合成 RunChanged{Failed} + run.failed 兜底不丢。
    let dir = tempfile::tempdir().expect("tempdir");
    let (store, _) = pawork_storage::session::SessionStore::open(dir.path().join("session.db"))
        .await
        .expect("store");
    let provider = MockProvider::sequence(vec![MockScript::new().text("unreachable").complete()]);
    let core = AppCore::from_parts(
        Arc::new(provider),
        None,
        pawork_domain::ModelId::from("model-1"),
        pawork_domain::ProviderId::from("mock"),
        Some(store),
    );
    let session = core.create_session("gui-plan-gate").await.expect("session");
    core.store()
        .expect("store")
        .append_event(
            pawork_storage::session::DEFAULT_BRANCH_ID,
            AgentEventEnvelope::new(
                pawork_domain::EventId::from("evt-plan-gate-1"),
                session.clone(),
                RunId::from("run-plan-seed"),
                pawork_domain::EventSequence::new(1),
                now_timestamp(),
                AgentEvent::Plan(pawork_domain::PlanEvent::Created {
                    plan_id: pawork_domain::PlanId::from("plan-gate"),
                    version: pawork_domain::PlanVersionId::from("plan-gate-v1"),
                    title: "draft plan".into(),
                    steps: vec![pawork_domain::PlanStepSnapshot {
                        step_id: pawork_domain::PlanStepId::from("plan-gate-step-1"),
                        text: "draft step".into(),
                        status: pawork_domain::PlanStepStatus::Pending,
                    }],
                }),
            ),
        )
        .await
        .expect("seed draft plan");
    let adapter = GuiHostAdapter::new(Arc::new(core));
    let runs = adapter.runs();
    let mut events = adapter.subscribe_events();
    let response = adapter
        .command(&command_envelope(AppCommand::RunStart {
            session_id: session.clone(),
            user_message: "blocked by plan gate".into(),
            model: None,
            provider: None,
            profile: None,
        }))
        .await
        .expect("run accepted");
    let AppResponse::Accepted {
        run_id: Some(run), ..
    } = response
    else {
        panic!("RunStart must be accepted: {response:?}");
    };
    wait_run_registry_drains(&runs, &run).await;
    let envelopes = drain_wire_envelopes(&mut events);
    let wire: Vec<AppEvent> = envelopes
        .iter()
        .map(|envelope| envelope.payload.clone())
        .collect();
    assert_eq!(
        terminal_states_for(&wire, &run),
        vec![RunState::Failed],
        "early death without an engine terminal must still synthesize exactly one RunChanged{{Failed}}: {wire:?}"
    );
    assert!(
        has_run_failed_diagnostic(&wire),
        "fallback run.failed diagnostic must survive for early-death paths: {wire:?}"
    );
    // 合成兜底不占真实持久化号段：序号从 SYNTHETIC_SEQUENCE_BASE 递增自取，
    // 有序插入落在既有时间线内容（含用户消息乐观回显）之后而非 seq-0 顶端。
    let synthetic_sequences: Vec<u64> = envelopes
        .iter()
        .filter(|envelope| {
            matches!(
                envelope.payload,
                AppEvent::RunChanged {
                    state: RunState::Failed,
                    ..
                } | AppEvent::Diagnostic { .. }
            )
        })
        .map(|envelope| envelope.stream_sequence)
        .collect();
    assert_eq!(
        synthetic_sequences.len(),
        2,
        "early death must emit exactly the synthetic terminal pair: {wire:?}"
    );
    assert!(
        synthetic_sequences
            .iter()
            .all(|sequence| *sequence >= super::bus::SYNTHETIC_SEQUENCE_BASE),
        "synthetic envelopes must not occupy the persisted sequence space: {synthetic_sequences:?}"
    );
    assert!(
        synthetic_sequences[0] < synthetic_sequences[1],
        "synthetic sequences must follow arrival order: {synthetic_sequences:?}"
    );
}

#[tokio::test]
async fn run_terminal_gate_persists_run_failed_before_broadcast() {
    // 合成闸硬化主路径：engine 未报终态即死且 run 行已存在时，宿主先
    // 持久化真实 RunFailed（persist-first），再经正常映射补广播——
    // RunChanged{Failed} 携带真实持久化 sequence，不占合成号段；
    // run.failed 诊断保留。
    let dir = tempfile::tempdir().expect("tempdir");
    let (store, _) = pawork_storage::session::SessionStore::open(dir.path().join("session.db"))
        .await
        .expect("store");
    let provider = MockProvider::sequence(vec![MockScript::new().text("unreachable").complete()]);
    let core = AppCore::from_parts(
        Arc::new(provider),
        None,
        pawork_domain::ModelId::from("model-1"),
        pawork_domain::ProviderId::from("mock"),
        Some(store),
    );
    let session = core
        .create_session("gate-durable-seal")
        .await
        .expect("session");
    let run = RunId::from("run-gate-seal");
    core.store()
        .expect("store")
        .append_event(
            pawork_storage::session::DEFAULT_BRANCH_ID,
            AgentEventEnvelope::new(
                pawork_domain::EventId::from("evt-gate-seed-1"),
                session.clone(),
                run.clone(),
                pawork_domain::EventSequence::new(1),
                now_timestamp(),
                AgentEvent::RunStarted {
                    trigger_message_id: MessageId::from("msg-gate-seal"),
                },
            ),
        )
        .await
        .expect("seed run without terminal");

    let adapter = GuiHostAdapter::new(Arc::new(core));
    let mut events = adapter.subscribe_events();
    let error = crate::AppError::EmptyTurn;
    {
        let core = adapter.core.read().await;
        super::handlers::run_start::seal_run_without_terminal(
            &core,
            &adapter.bus,
            adapter.instance.clone(),
            &session,
            &run,
            &error,
        )
        .await;
    }

    let persisted_sequence;
    {
        let core = adapter.core.read().await;
        let sealed = core
            .store()
            .expect("store")
            .replay_events(&session, 1, 100)
            .await
            .expect("replay");
        let last = sealed.last().expect("durable seal appended");
        persisted_sequence = last.sequence.value();
        match &last.payload {
            AgentEvent::RunFailed { error: context, .. } => {
                assert_eq!(context.category, pawork_domain::ErrorCategory::Internal);
                assert_eq!(context.message, error.to_string());
            }
            other => panic!("durable seal must append RunFailed: {other:?}"),
        }
    }

    let envelopes = drain_wire_envelopes(&mut events);
    let wire: Vec<AppEvent> = envelopes
        .iter()
        .map(|envelope| envelope.payload.clone())
        .collect();
    assert_eq!(
        terminal_states_for(&wire, &run),
        vec![RunState::Failed],
        "durable seal must broadcast exactly one terminal RunChanged{{Failed}}: {wire:?}"
    );
    let terminal_envelope = envelopes
        .iter()
        .find(|envelope| {
            matches!(
                &envelope.payload,
                AppEvent::RunChanged {
                    run_id: id,
                    state: RunState::Failed,
                } if id == &run
            )
        })
        .expect("terminal envelope");
    assert_eq!(
        terminal_envelope.stream_sequence, persisted_sequence,
        "terminal RunChanged must carry the persisted sequence: {envelopes:?}"
    );
    assert!(
        terminal_envelope.stream_sequence < super::bus::SYNTHETIC_SEQUENCE_BASE,
        "durable seal must not occupy the synthetic sequence space: {envelopes:?}"
    );
    assert!(
        has_run_failed_diagnostic(&wire),
        "run.failed diagnostic must survive the durable seal path: {wire:?}"
    );
}

#[tokio::test]
async fn run_start_switches_same_registry_model_and_unknown_fails_closed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (store, _) = pawork_storage::session::SessionStore::open(dir.path().join("session.db"))
        .await
        .expect("store");
    let provider =
        MockProvider::sequence(vec![MockScript::new().text("hello from other").complete()])
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
        Arc::new(MockProvider::sequence(vec![MockScript::new()
            .text("idle")
            .complete()])),
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
            workspace_id: WorkspaceId::from("ws-default"),
            workspace_roots: Vec::new(),
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

    let replay_start_id = adapter
        .command(&start)
        .await
        .expect("replay run command_id");
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
            .find(|event| {
                matches!(
                    &event.payload,
                    pawork_domain::AgentEvent::RunCompleted { .. }
                )
            })
            .expect("run completed boundary")
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
    assert_eq!(
        data.get("session_id").and_then(Value::as_str),
        Some(session.as_str())
    );
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
    assert_eq!(
        prepared.len(),
        2,
        "two turns should prepare context twice: {prepared:?}"
    );
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
        Arc::new(MockProvider::sequence(vec![MockScript::new()
            .text("ok")
            .complete()])),
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

#[tokio::test]
async fn command_idempotency_survives_restart() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = dir.path().join("session.db");
    let (store, _) = pawork_storage::session::SessionStore::open(&db)
        .await
        .expect("store");
    let core = AppCore::from_parts(
        Arc::new(MockProvider::sequence(vec![MockScript::new()
            .text("ok")
            .complete()])),
        None,
        pawork_domain::ModelId::from("model-1"),
        pawork_domain::ProviderId::from("mock"),
        Some(store),
    );
    let adapter = GuiHostAdapter::new(Arc::new(core));
    let create = command_envelope_with(
        CommandId::from("cmd-create-1"),
        Some("create-once"),
        AppCommand::SessionCreate {
            workspace_id: WorkspaceId::from("ws-default"),
            title: Some("once".into()),
        },
    );
    let first = adapter.command(&create).await.expect("create");
    drop(adapter);

    let (store, _) = pawork_storage::session::SessionStore::open(&db)
        .await
        .expect("reopen store");
    let core = AppCore::from_parts(
        Arc::new(MockProvider::sequence(vec![MockScript::new()
            .text("ok")
            .complete()])),
        None,
        pawork_domain::ModelId::from("model-1"),
        pawork_domain::ProviderId::from("mock"),
        Some(store),
    );
    let adapter = GuiHostAdapter::new(Arc::new(core));
    let replay = adapter.command(&create).await.expect("restart replay");
    assert_eq!(replay, first);
    let snapshot = adapter.snapshot().await.expect("snapshot");
    assert_eq!(session_titles(&snapshot, "once"), 1);
}

#[tokio::test]
async fn command_record_failure_is_counted_not_swallowed() {
    // command() looks up idempotency_key in check() before record(), so a
    // pre-bound key Replays and never reaches persist. Inject the failure
    // through persist_command_response — the helper command() actually
    // calls — with a reserved inflight command_id and a key already bound
    // to another command. Closing the db is unstable.
    let dir = tempfile::tempdir().expect("tempdir");
    let (store, _) = pawork_storage::session::SessionStore::open(dir.path().join("session.db"))
        .await
        .expect("store");
    let core = AppCore::from_parts(
        Arc::new(MockProvider::sequence(vec![MockScript::new()
            .text("idle")
            .complete()])),
        None,
        pawork_domain::ModelId::from("model-1"),
        pawork_domain::ProviderId::from("mock"),
        Some(store),
    );
    let adapter = GuiHostAdapter::new(Arc::new(core));
    let mut ledger = crate::IdempotencyStore::for_store(
        adapter
            .session_store()
            .await
            .expect("store")
            .command_ledger(),
    )
    .with_scope("automation");
    ledger.share_waiters_from(&adapter.waiters);
    let tenant = TenantId::new(DEFAULT_CONTROL_PLANE_TENANT);
    let primed_id = CommandId::from("cmd-create-shared");
    let conflict_id = CommandId::from("cmd-create-conflict");
    let primed = AppResponseEnvelope {
        api_version: API_VERSION,
        request_id: QueryId::from(primed_id.as_str()),
        responded_at: Timestamp::from_unix_millis(1),
        response: AppResponse::Accepted {
            command_id: primed_id.clone(),
            run_id: None,
        },
    };
    ledger
        .record(&tenant, &primed_id, Some("shared-key"), primed)
        .await
        .expect("prime key");
    assert!(matches!(
        ledger
            .check(&tenant, &conflict_id, None)
            .await
            .expect("reserve"),
        crate::IdempotencyCheck::New
    ));
    let conflict = AppResponseEnvelope {
        api_version: API_VERSION,
        request_id: QueryId::from(conflict_id.as_str()),
        responded_at: Timestamp::from_unix_millis(2),
        response: AppResponse::Accepted {
            command_id: conflict_id.clone(),
            run_id: None,
        },
    };
    // RecordingCapture 双注册钉住 interest 缓存：与无 subscriber 的兄弟测试共享
    // degrade.idempotency_conflict callsite，裸 set_default 会因 never 缓存间歇丢事件。
    let capture = crate::testsupport::RecordingCapture::install();
    let mut events = adapter.subscribe_events();
    adapter
        .persist_command_response(&ledger, &tenant, &conflict_id, Some("shared-key"), conflict)
        .await;
    let captured = capture.events();
    capture.dismiss();
    assert_eq!(adapter.command_record_failure_count().await, 1);
    assert!(
        matches!(
            events.try_recv(),
            Err(tokio::sync::broadcast::error::TryRecvError::Empty)
        ),
        "IdempotencyConflict must not send a client frame"
    );
    let emitted = captured
        .iter()
        .find(|event| {
            event.fields.get("code").map(String::as_str) == Some("degrade.idempotency_conflict")
        })
        .unwrap_or_else(|| panic!("record failure must emit tracing: {captured:?}"));
    assert_eq!(emitted.level, "ERROR");
    assert_eq!(
        emitted.fields.get("command_id").map(String::as_str),
        Some(conflict_id.as_str()),
        "{emitted:?}"
    );
    let created = adapter
        .command(&command_envelope_with(
            CommandId::from("cmd-create-live"),
            None,
            AppCommand::SessionCreate {
                workspace_id: WorkspaceId::from("ws-default"),
                title: Some("live".into()),
            },
        ))
        .await
        .expect("command still returns");
    assert!(
        matches!(created, AppResponse::Data(_)),
        "expected Data after dispatch, got {created:?}"
    );
}

#[tokio::test]
async fn snapshot_rebuilds_pending_approvals_after_restart() {
    // Pin: PendingToolApprovals is global (same as host.pending()), so a
    // waiting projection from any session is merged without a session filter.
    let dir = tempfile::tempdir().expect("tempdir");
    let db = dir.path().join("session.db");
    let (store, _) = pawork_storage::session::SessionStore::open(&db)
        .await
        .expect("store");
    let core = AppCore::from_parts(
        Arc::new(MockProvider::sequence(vec![MockScript::new()
            .text("idle")
            .complete()])),
        None,
        pawork_domain::ModelId::from("model-1"),
        pawork_domain::ProviderId::from("mock"),
        Some(store),
    );
    let session = core.create_session("wait").await.expect("session");
    let tool_call_id = pawork_domain::ToolCallId::from("call-wait");
    let run_id = RunId::from("run-wait");
    append_waiting_write(&core, &session, &run_id, &tool_call_id, "evt", 1).await;
    drop(core);

    let (store, _) = pawork_storage::session::SessionStore::open(&db)
        .await
        .expect("reopen");
    let core = AppCore::from_parts(
        Arc::new(MockProvider::sequence(vec![MockScript::new()
            .text("idle")
            .complete()])),
        None,
        pawork_domain::ModelId::from("model-1"),
        pawork_domain::ProviderId::from("mock"),
        Some(store),
    );
    let adapter = GuiHostAdapter::new(Arc::new(core));
    let snapshot = adapter.snapshot().await.expect("snapshot");
    let pending = snapshot
        .sections
        .iter()
        .find(|section| section.kind == SnapshotSectionKind::PendingToolApprovals)
        .and_then(|section| section.data.clone())
        .expect("pending section");
    let items = pending.as_array().expect("array");
    assert_eq!(items.len(), 1);
    assert_eq!(
        items[0].get("message").and_then(Value::as_str),
        Some("approval pending across restart")
    );
    assert_eq!(
        items[0].get("relative_path").and_then(Value::as_str),
        Some("notes.txt")
    );
}

async fn append_waiting_write(
    core: &AppCore,
    session: &SessionId,
    run_id: &RunId,
    tool_call_id: &pawork_domain::ToolCallId,
    event_prefix: &str,
    start_sequence: u64,
) {
    let ts = now_timestamp();
    core.store()
        .expect("store")
        .append_event(
            pawork_storage::session::DEFAULT_BRANCH_ID,
            AgentEventEnvelope::new(
                pawork_domain::EventId::from(format!("{event_prefix}-1")),
                session.clone(),
                run_id.clone(),
                pawork_domain::EventSequence::new(start_sequence),
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
            pawork_storage::session::DEFAULT_BRANCH_ID,
            AgentEventEnvelope::new(
                pawork_domain::EventId::from(format!("{event_prefix}-2")),
                session.clone(),
                run_id.clone(),
                pawork_domain::EventSequence::new(start_sequence + 1),
                ts,
                AgentEvent::ToolCallArgumentsDelta {
                    tool_call_id: tool_call_id.clone(),
                    json_delta: r#"{"path":"notes.txt","content":"secret"}"#.into(),
                },
            ),
        )
        .await
        .expect("args");
    core.store()
        .expect("store")
        .append_event(
            pawork_storage::session::DEFAULT_BRANCH_ID,
            AgentEventEnvelope::new(
                pawork_domain::EventId::from(format!("{event_prefix}-3")),
                session.clone(),
                run_id.clone(),
                pawork_domain::EventSequence::new(start_sequence + 2),
                ts,
                AgentEvent::ToolApprovalRequested {
                    tool_call_id: tool_call_id.clone(),
                    reason: "tool `write_file` requires approval".into(),
                },
            ),
        )
        .await
        .expect("requested");
}

fn idle_core(store: pawork_storage::session::SessionStore) -> AppCore {
    AppCore::from_parts(
        Arc::new(MockProvider::sequence(vec![MockScript::new()
            .text("idle")
            .complete()])),
        None,
        pawork_domain::ModelId::from("model-1"),
        pawork_domain::ProviderId::from("mock"),
        Some(store),
    )
}

fn replay_types(events: &[AgentEventEnvelope]) -> Vec<&'static str> {
    events
        .iter()
        .map(|envelope| match &envelope.payload {
            AgentEvent::ToolApprovalRequested { .. } => "ToolApprovalRequested",
            AgentEvent::ToolApprovalResponded { .. } => "ToolApprovalResponded",
            AgentEvent::ToolExecutionStarted { .. } => "ToolExecutionStarted",
            AgentEvent::ToolExecutionCompleted { .. } => "ToolExecutionCompleted",
            AgentEvent::MessageCommitted { message } if message.role == MessageRole::Tool => {
                "MessageCommitted.tool"
            }
            _ => "other",
        })
        .collect()
}

#[tokio::test]
async fn tool_approve_live_waiting_does_not_durable_seal() {
    let host = Arc::new(GuiApprovalHost::new());
    let ask = crate::ApprovalAsk {
        run_id: RunId::from("run-live"),
        session_id: Some(SessionId::from("ses-live")),
        tool_name: "write_file".into(),
        tool_call_id: pawork_domain::ToolCallId::from("call-live"),
        relative_path: Some("notes.txt".into()),
        message: "Approve workspace file write".into(),
        risk: pawork_policy::RiskLevel::Moderate,
        preview: None,
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
    let core = idle_core(store);
    let session = core.create_session("live").await.expect("session");
    let run_id = RunId::from("run-live");
    let tool_call_id = pawork_domain::ToolCallId::from("call-live");
    append_waiting_write(&core, &session, &run_id, &tool_call_id, "evt-live", 1).await;
    let adapter = GuiHostAdapter::with_approvals(Arc::new(core), Arc::clone(&host));
    adapter.runs().register(
        ActiveGuiRun {
            run_id: run_id.clone(),
            session_id: session.clone(),
            workspace_id: WorkspaceId::from("ws-default"),
            workspace_roots: Vec::new(),
            started_at_ms: 1,
        },
        CancellationToken::new(),
    );

    let response = adapter
        .command(&command_envelope(AppCommand::ToolApprove {
            run_id: run_id.clone(),
            tool_call_id: tool_call_id.clone(),
            decision: pawork_protocol::ApprovalDecision::Deny,
        }))
        .await
        .expect("live approve");
    assert!(matches!(response, AppResponse::Accepted { .. }));
    let decision = waiter.await.expect("join");
    assert_eq!(decision, ApprovalDecision::Denied);

    let events = adapter
        .session_store()
        .await
        .expect("store")
        .replay_events(&session, 1, usize::MAX)
        .await
        .expect("replay");
    let types = replay_types(&events);
    assert!(!types.contains(&"ToolApprovalResponded"));
    assert!(!types.contains(&"ToolExecutionCompleted"));
    assert!(!types.contains(&"MessageCommitted.tool"));
    let snap = adapter
        .session_store()
        .await
        .expect("store")
        .projection_snapshot(&session)
        .await
        .expect("proj");
    assert_eq!(snap.tool_calls[0].state, "waiting_for_approval");
}

#[tokio::test]
async fn tool_approve_non_live_waiting_projection_is_durable() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (store, _) = pawork_storage::session::SessionStore::open(dir.path().join("session.db"))
        .await
        .expect("store");
    let core = idle_core(store);
    let session = core.create_session("durable").await.expect("session");
    let run_id = RunId::from("run-detached");
    let tool_call_id = pawork_domain::ToolCallId::from("call-detached");
    append_waiting_write(&core, &session, &run_id, &tool_call_id, "evt-d", 1).await;
    let adapter = GuiHostAdapter::new(Arc::new(core));
    assert!(!adapter.runs().contains(&run_id));

    adapter
        .command(&command_envelope(AppCommand::ToolApprove {
            run_id: run_id.clone(),
            tool_call_id: tool_call_id.clone(),
            decision: pawork_protocol::ApprovalDecision::Deny,
        }))
        .await
        .expect("durable deny");

    let events = adapter
        .session_store()
        .await
        .expect("store")
        .replay_events(&session, 1, usize::MAX)
        .await
        .expect("replay");
    let types = replay_types(&events);
    assert!(types.contains(&"ToolApprovalResponded"));
    assert!(types.contains(&"ToolExecutionCompleted"));
    assert!(types.contains(&"MessageCommitted.tool"));
    assert!(!types.contains(&"ToolExecutionStarted"));
    let responded = events
        .iter()
        .find_map(|envelope| match &envelope.payload {
            AgentEvent::ToolApprovalResponded {
                decision, comment, ..
            } => Some((decision.clone(), comment.clone())),
            _ => None,
        })
        .expect("responded payload");
    assert_eq!(responded.0, ApprovalDecision::Denied);
    assert_eq!(
        responded.1.as_deref(),
        Some("approval resolved after restart; tool not executed")
    );
    let completed = events.iter().find_map(|envelope| match &envelope.payload {
        AgentEvent::ToolExecutionCompleted { result, .. } => Some(result.is_error),
        _ => None,
    });
    assert_eq!(completed, Some(true));
}

#[tokio::test]
async fn tool_approve_non_live_waiting_broadcasts_tool_completed() {
    // 重启后 queued 决议：persist-first 落库之外还必须补实时广播，
    // 否则 GUI 的 clear_pending_for_tool 永不触发、审批卡永驻。
    let dir = tempfile::tempdir().expect("tempdir");
    let (store, _) = pawork_storage::session::SessionStore::open(dir.path().join("session.db"))
        .await
        .expect("store");
    let core = idle_core(store);
    let session = core
        .create_session("queued-broadcast")
        .await
        .expect("session");
    let run_id = RunId::from("run-queued-broadcast");
    let tool_call_id = pawork_domain::ToolCallId::from("call-queued-broadcast");
    append_waiting_write(&core, &session, &run_id, &tool_call_id, "evt-qb", 1).await;
    let adapter = GuiHostAdapter::new(Arc::new(core));
    assert!(!adapter.runs().contains(&run_id));
    let mut events = adapter.subscribe_events();

    adapter
        .command(&command_envelope(AppCommand::ToolApprove {
            run_id: run_id.clone(),
            tool_call_id: tool_call_id.clone(),
            decision: pawork_protocol::ApprovalDecision::ApproveOnce,
        }))
        .await
        .expect("queued approve broadcast");

    // 广播在 command 返回前同步完成，订阅缓冲此刻已含全部 wire 事件。
    let mut wire = Vec::new();
    loop {
        match events.try_recv() {
            Ok(envelope) => wire.push(envelope.payload),
            Err(tokio::sync::broadcast::error::TryRecvError::Lagged(missed)) => {
                panic!("subscriber lagged: {missed}");
            }
            Err(
                tokio::sync::broadcast::error::TryRecvError::Empty
                | tokio::sync::broadcast::error::TryRecvError::Closed,
            ) => break,
        }
    }
    let completed: Vec<_> = wire
        .iter()
        .filter_map(|event| match event {
            AppEvent::ToolCompleted {
                run_id,
                tool_call_id,
                success,
            } => Some((run_id.clone(), tool_call_id.clone(), *success)),
            _ => None,
        })
        .collect();
    assert_eq!(
        completed,
        vec![(run_id.clone(), tool_call_id.clone(), false)],
        "queued approval closure must broadcast exactly one failed ToolCompleted"
    );
    // 钉住当前 wire 契约：Responded/Committed 不进实时流，
    // 实时流除 ToolCompleted 外不得出现任何 approval 类事件。
    assert_eq!(wire.len(), 1, "unexpected wire events: {wire:?}");
    assert!(
        !wire
            .iter()
            .any(|event| matches!(event, AppEvent::ToolApprovalRequired { .. })),
        "approval-required must not leak into the closure broadcast"
    );

    // 持久化先于广播：库内三事件（Responded/Completed/Committed.tool）仍在。
    let store_events = adapter
        .session_store()
        .await
        .expect("store")
        .replay_events(&session, 1, usize::MAX)
        .await
        .expect("replay");
    let types = replay_types(&store_events);
    assert!(types.contains(&"ToolApprovalResponded"));
    assert!(types.contains(&"ToolExecutionCompleted"));
    assert!(types.contains(&"MessageCommitted.tool"));
}

#[tokio::test]
async fn tool_approve_non_live_without_waiting_projection_stays_queued() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (store, _) = pawork_storage::session::SessionStore::open(dir.path().join("session.db"))
        .await
        .expect("store");
    let core = idle_core(store);
    let session = core.create_session("queued").await.expect("session");
    let adapter = GuiHostAdapter::new(Arc::new(core));
    let run_id = RunId::from("run-missing");
    let tool_call_id = pawork_domain::ToolCallId::from("call-missing");
    assert!(!adapter.runs().contains(&run_id));

    adapter
        .command(&command_envelope(AppCommand::ToolApprove {
            run_id,
            tool_call_id,
            decision: pawork_protocol::ApprovalDecision::Deny,
        }))
        .await
        .expect("queued");

    let events = adapter
        .session_store()
        .await
        .expect("store")
        .replay_events(&session, 1, usize::MAX)
        .await
        .expect("replay");
    let types = replay_types(&events);
    assert!(!types.contains(&"ToolApprovalResponded"));
    assert!(!types.contains(&"ToolExecutionCompleted"));
    assert_eq!(adapter.approvals().pending().len(), 0);
}

#[tokio::test]
async fn inflight_shared_key_different_command_id_does_not_hang() {
    // Hazard 1: same idempotency_key with a different command_id returns
    // InFlight, but Notify is keyed by the waiter command_id. Bounded wait
    // must recheck SQLite instead of hanging forever.
    let dir = tempfile::tempdir().expect("tempdir");
    let (store, _) = pawork_storage::session::SessionStore::open(dir.path().join("session.db"))
        .await
        .expect("store");
    let core = AppCore::from_parts(
        Arc::new(MockProvider::sequence(vec![MockScript::new()
            .text("ok")
            .complete()])),
        None,
        pawork_domain::ModelId::from("model-1"),
        pawork_domain::ProviderId::from("mock"),
        Some(store),
    );
    let adapter = Arc::new(GuiHostAdapter::new(Arc::new(core)));
    let mut ledger = crate::IdempotencyStore::for_store(
        adapter
            .session_store()
            .await
            .expect("store")
            .command_ledger(),
    )
    .with_scope("automation");
    ledger.share_waiters_from(&adapter.waiters);
    let tenant = TenantId::new(DEFAULT_CONTROL_PLANE_TENANT);
    let holder = CommandId::from("cmd-inflight-holder");
    assert!(matches!(
        ledger
            .check(&tenant, &holder, Some("shared-hang"))
            .await
            .expect("reserve"),
        crate::IdempotencyCheck::New
    ));
    let waiter = tokio::spawn({
        let adapter = Arc::clone(&adapter);
        async move {
            adapter
                .command(&command_envelope_with(
                    CommandId::from("cmd-inflight-waiter"),
                    Some("shared-hang"),
                    AppCommand::SessionCreate {
                        workspace_id: WorkspaceId::from("ws-default"),
                        title: Some("waiter".into()),
                    },
                ))
                .await
        }
    });
    tokio::time::sleep(std::time::Duration::from_millis(80)).await;
    ledger
        .record(
            &tenant,
            &holder,
            Some("shared-hang"),
            AppResponseEnvelope {
                api_version: API_VERSION,
                request_id: QueryId::from(holder.as_str()),
                responded_at: Timestamp::from_unix_millis(1),
                response: AppResponse::Accepted {
                    command_id: holder.clone(),
                    run_id: None,
                },
            },
        )
        .await
        .expect("record holder");
    let result = tokio::time::timeout(std::time::Duration::from_secs(2), waiter)
        .await
        .expect("waiter join timed out")
        .expect("waiter task");
    assert!(
        result.is_ok(),
        "shared-key InFlight waiter must finish with Replay or an explicit result, got {result:?}"
    );
}

#[tokio::test]
async fn inflight_dropped_wakeup_still_converges_via_bounded_poll() {
    // Hazard 2: notify_waiters does not store a permit. If record completes
    // after check returns InFlight but before notified() is created, the
    // wakeup can be lost. Bounded poll must still recheck SQLite.
    let dir = tempfile::tempdir().expect("tempdir");
    let (store, _) = pawork_storage::session::SessionStore::open(dir.path().join("session.db"))
        .await
        .expect("store");
    let core = AppCore::from_parts(
        Arc::new(MockProvider::sequence(vec![MockScript::new()
            .text("ok")
            .complete()])),
        None,
        pawork_domain::ModelId::from("model-1"),
        pawork_domain::ProviderId::from("mock"),
        Some(store),
    );
    let adapter = GuiHostAdapter::new(Arc::new(core));
    let mut ledger = crate::IdempotencyStore::for_store(
        adapter
            .session_store()
            .await
            .expect("store")
            .command_ledger(),
    )
    .with_scope("automation");
    ledger.share_waiters_from(&adapter.waiters);
    let tenant = TenantId::new(DEFAULT_CONTROL_PLANE_TENANT);
    let holder = CommandId::from("cmd-drop-wakeup-holder");
    let waiter_id = CommandId::from("cmd-drop-wakeup-waiter");
    assert!(matches!(
        ledger
            .check(&tenant, &holder, Some("drop-wakeup"))
            .await
            .expect("reserve"),
        crate::IdempotencyCheck::New
    ));
    assert!(matches!(
        ledger
            .check(&tenant, &waiter_id, Some("drop-wakeup"))
            .await
            .expect("inflight"),
        crate::IdempotencyCheck::InFlight(_)
    ));
    ledger
        .record(
            &tenant,
            &holder,
            Some("drop-wakeup"),
            AppResponseEnvelope {
                api_version: API_VERSION,
                request_id: QueryId::from(holder.as_str()),
                responded_at: Timestamp::from_unix_millis(1),
                response: AppResponse::Accepted {
                    command_id: holder.clone(),
                    run_id: None,
                },
            },
        )
        .await
        .expect("record before waiter notified()");
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        adapter.command(&command_envelope_with(
            waiter_id,
            Some("drop-wakeup"),
            AppCommand::SessionCreate {
                workspace_id: WorkspaceId::from("ws-default"),
                title: Some("drop-wakeup".into()),
            },
        )),
    )
    .await
    .expect("dropped wakeup waiter timed out");
    assert!(
        result.is_ok(),
        "bounded poll must converge after a lost notify, got {result:?}"
    );
}

#[tokio::test]
async fn record_failure_releases_inflight_so_same_command_id_can_reenter() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (store, _) = pawork_storage::session::SessionStore::open(dir.path().join("session.db"))
        .await
        .expect("store");
    let core = AppCore::from_parts(
        Arc::new(MockProvider::sequence(vec![MockScript::new()
            .text("idle")
            .complete()])),
        None,
        pawork_domain::ModelId::from("model-1"),
        pawork_domain::ProviderId::from("mock"),
        Some(store),
    );
    let adapter = GuiHostAdapter::new(Arc::new(core));
    let mut ledger = crate::IdempotencyStore::for_store(
        adapter
            .session_store()
            .await
            .expect("store")
            .command_ledger(),
    )
    .with_scope("automation");
    ledger.share_waiters_from(&adapter.waiters);
    let tenant = TenantId::new(DEFAULT_CONTROL_PLANE_TENANT);
    let primed_id = CommandId::from("cmd-create-shared");
    let conflict_id = CommandId::from("cmd-create-conflict-retry");
    let primed = AppResponseEnvelope {
        api_version: API_VERSION,
        request_id: QueryId::from(primed_id.as_str()),
        responded_at: Timestamp::from_unix_millis(1),
        response: AppResponse::Accepted {
            command_id: primed_id.clone(),
            run_id: None,
        },
    };
    ledger
        .record(&tenant, &primed_id, Some("shared-key"), primed.clone())
        .await
        .expect("prime key");
    assert!(matches!(
        ledger
            .check(&tenant, &conflict_id, None)
            .await
            .expect("reserve"),
        crate::IdempotencyCheck::New
    ));
    let conflict = AppResponseEnvelope {
        api_version: API_VERSION,
        request_id: QueryId::from(conflict_id.as_str()),
        responded_at: Timestamp::from_unix_millis(2),
        response: AppResponse::Accepted {
            command_id: conflict_id.clone(),
            run_id: None,
        },
    };
    adapter
        .persist_command_response(&ledger, &tenant, &conflict_id, Some("shared-key"), conflict)
        .await;
    assert_eq!(adapter.command_record_failure_count().await, 1);
    match ledger
        .check(&tenant, &conflict_id, Some("shared-key"))
        .await
        .expect("keyed retry after release")
    {
        crate::IdempotencyCheck::Replay(replay) => {
            assert_eq!(
                replay.response, primed.response,
                "keyed retry must Replay the primed holder, not re-execute"
            );
        }
        other => panic!("expected Replay of primed key holder, got {other:?}"),
    }
    match ledger
        .check(&tenant, &conflict_id, None)
        .await
        .expect("reenter after record failure")
    {
        crate::IdempotencyCheck::InFlight(_) => {
            panic!("record failure must release inflight so the same command_id is not stuck")
        }
        crate::IdempotencyCheck::New | crate::IdempotencyCheck::Replay(_) => {}
    }
}

// ------------------------------------------------------------------
// ADR-045：terminal_close 命令与 TerminalExited live 事件。

async fn terminal_adapter() -> (GuiHostAdapter, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let (store, _) = pawork_storage::session::SessionStore::open(dir.path().join("session.db"))
        .await
        .expect("store");
    let provider = MockProvider::sequence(vec![MockScript::new().text("ok").complete()]);
    let mut core = AppCore::from_parts(
        Arc::new(provider),
        None,
        pawork_domain::ModelId::from("model-1"),
        pawork_domain::ProviderId::from("mock"),
        Some(store),
    );
    core.attach_workspace(dir.path()).expect("attach workspace");
    core.configure_approval(
        crate::ApprovalMode::AskForDangerous,
        true,
        Arc::new(crate::DenyAllApprovals),
    );
    (GuiHostAdapter::new(Arc::new(core)), dir)
}

async fn create_terminal(adapter: &GuiHostAdapter) -> String {
    let created = adapter
        .command(&command_envelope(AppCommand::TerminalCreate {
            workspace_id: WorkspaceId::from("ws-default"),
            working_directory: None,
        }))
        .await
        .expect("terminal_create");
    let AppResponse::Data(payload) = created else {
        panic!("terminal_create must return Data: {created:?}");
    };
    payload["terminal_session_id"]
        .as_str()
        .expect("terminal_session_id")
        .to_string()
}

async fn wait_terminal_exited(
    events: &mut tokio::sync::broadcast::Receiver<AppEventEnvelope>,
    terminal_id: &str,
) -> (
    Option<i32>,
    Option<String>,
    pawork_protocol::TerminalExitReason,
) {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let envelope = tokio::time::timeout_at(deadline, events.recv())
            .await
            .expect("TerminalExited within deadline")
            .expect("event stream open");
        if let AppEvent::TerminalExited {
            terminal_session_id,
            exit_code,
            signal,
            reason,
        } = envelope.payload
        {
            if terminal_session_id == terminal_id {
                return (exit_code, signal, reason);
            }
        }
    }
}

#[tokio::test]
async fn terminal_close_kills_broadcasts_killed_and_unregisters() {
    let (adapter, _dir) = terminal_adapter().await;
    let mut events = adapter.subscribe_events();
    let terminal_id = create_terminal(&adapter).await;
    let closed = adapter
        .command(&command_envelope(AppCommand::TerminalClose {
            terminal_session_id: terminal_id.clone(),
        }))
        .await
        .expect("terminal_close");
    assert!(matches!(closed, AppResponse::Accepted { .. }));
    let (_, _, reason) = wait_terminal_exited(&mut events, &terminal_id).await;
    assert_eq!(reason, pawork_protocol::TerminalExitReason::Killed);
    assert_eq!(
        adapter.pty.session_count(),
        0,
        "terminal_close must remove the PTY service entry and buffered state"
    );
    assert!(
        adapter.terminal_snapshots().iter().all(|entry| {
            entry.get("terminal_session_id").and_then(Value::as_str) != Some(terminal_id.as_str())
        }),
        "closed terminal must leave the snapshot section"
    );
    // 幂等边界（ADR-045 D1）：重复 close 同一已注销 id 报 not_found，不伪造成功。
    let again = adapter
        .command(&command_envelope(AppCommand::TerminalClose {
            terminal_session_id: terminal_id.clone(),
        }))
        .await
        .expect_err("repeat close must fail");
    assert_eq!(again.code, "not_found");
}

#[tokio::test]
async fn terminal_natural_exit_broadcasts_exited_with_code() {
    let (adapter, _dir) = terminal_adapter().await;
    let mut events = adapter.subscribe_events();
    let terminal_id = create_terminal(&adapter).await;
    adapter
        .command(&command_envelope(AppCommand::TerminalWrite {
            terminal_session_id: terminal_id.clone(),
            data: "exit 0\n".into(),
        }))
        .await
        .expect("terminal_write");
    let (exit_code, _, reason) = wait_terminal_exited(&mut events, &terminal_id).await;
    assert_eq!(reason, pawork_protocol::TerminalExitReason::Exited);
    assert_eq!(exit_code, Some(0));
    assert_eq!(
        adapter.pty.session_count(),
        1,
        "natural exit remains as a reconnectable tombstone until close"
    );

    adapter
        .command(&command_envelope(AppCommand::TerminalClose {
            terminal_session_id: terminal_id,
        }))
        .await
        .expect("close exited terminal");
    assert_eq!(
        adapter.pty.session_count(),
        0,
        "closing an exited tombstone must remove its PTY service entry"
    );
}
// ---- SET-2 Host Settings 门面（ADR-046）----

async fn settings_adapter(
    base_url: String,
    backend: Arc<pawork_auth::MemoryBackend>,
) -> (GuiHostAdapter, tempfile::TempDir) {
    settings_adapter_for_channel("glm-coding", "glm-5.2", base_url, backend).await
}

async fn settings_adapter_for_channel(
    provider_id: &str,
    model_id: &str,
    base_url: String,
    backend: Arc<pawork_auth::MemoryBackend>,
) -> (GuiHostAdapter, tempfile::TempDir) {
    settings_adapter_with_default(provider_id, model_id, base_url, backend, None).await
}

/// 可选在生效配置中注入 default_provider/default_model（SET-5 持久化
/// 默认项）；None 表示未配置默认项。
async fn settings_adapter_with_default(
    provider_id: &str,
    model_id: &str,
    base_url: String,
    backend: Arc<pawork_auth::MemoryBackend>,
    default: Option<(&str, &str)>,
) -> (GuiHostAdapter, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let (store, _) = pawork_storage::session::SessionStore::open(dir.path().join("session.db"))
        .await
        .expect("store");
    let mut config = pawork_workspace::config::PaworkConfig::default();
    config
        .providers
        .push(pawork_workspace::config::ProviderConfig {
            id: provider_id.into(),
            base_url: Some(base_url),
            default: None,
        });
    if let Some((default_provider, default_model)) = default {
        config.default_provider = Some(default_provider.into());
        config.default_model = Some(default_model.into());
    }
    let core = AppCore::from_parts(
        Arc::new(MockProvider::sequence(Vec::new())),
        None,
        pawork_domain::ModelId::from(model_id),
        pawork_domain::ProviderId::from(provider_id),
        Some(store),
    )
    .with_state(config, backend as Arc<dyn pawork_auth::SecretBackend>);
    (GuiHostAdapter::new(Arc::new(core)), dir)
}

/// 将 HOME 重定向到临时目录，并在 Drop 时恢复原值（含 panic 路径）。
/// 本文件仅此一处改进程环境；directories 在 Unix 上优先读 HOME，
/// 必须先恢复再删临时目录，避免其它测试读到已释放路径。
/// HOME 重定向测试互斥：libtest 并行会交叉改进程环境。
static HOME_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

struct RestoreHome(Option<std::ffi::OsString>);

impl Drop for RestoreHome {
    fn drop(&mut self) {
        #[allow(unused_unsafe)]
        unsafe {
            match self.0.take() {
                Some(value) => std::env::set_var("HOME", value),
                None => std::env::remove_var("HOME"),
            }
        }
    }
}

#[tokio::test]
async fn provider_auth_status_reports_persisted_default_pair() {
    let backend = Arc::new(pawork_auth::MemoryBackend::new());
    // 默认项取自生效配置而非 core 当前选中的 provider/model，
    // 因此注入与 core 选中值不同的默认对。
    let (adapter, _dir) = settings_adapter_with_default(
        "glm-coding",
        "glm-5.2",
        "http://127.0.0.1:1".into(),
        backend,
        Some(("deepseek", "deepseek-chat")),
    )
    .await;
    let status = adapter
        .query(&query_envelope(AppQuery::ProviderAuthStatus {
            provider_id: Some(pawork_domain::ProviderId::from("glm-coding")),
        }))
        .await
        .expect("provider auth status");
    let AppResponse::Data(status) = status else {
        panic!("ProviderAuthStatus must return Data: {status:?}")
    };
    assert_eq!(
        status["default"],
        serde_json::json!({
            "provider_id": "deepseek",
            "model_id": "deepseek-chat",
        })
    );
}

#[tokio::test]
async fn provider_auth_status_reports_null_default_when_unconfigured() {
    let backend = Arc::new(pawork_auth::MemoryBackend::new());
    let (adapter, _dir) = settings_adapter("http://127.0.0.1:1".into(), backend).await;
    let status = adapter
        .query(&query_envelope(AppQuery::ProviderAuthStatus {
            provider_id: Some(pawork_domain::ProviderId::from("glm-coding")),
        }))
        .await
        .expect("provider auth status");
    let AppResponse::Data(status) = status else {
        panic!("ProviderAuthStatus must return Data: {status:?}")
    };
    assert!(
        status["default"].is_null(),
        "default must be null: {status}"
    );
}

#[tokio::test]
async fn set_default_model_updates_status_default_within_same_session() {
    // 写盘目标经 HOME 重定向到临时目录，避免污染真实全局配置。
    // RestoreHome 必须在 tempfile 之后声明：Drop 先恢复 HOME，再删临时目录。
    let _home_env = HOME_ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let home = tempfile::tempdir().expect("home tempdir");
    let _restore_home = RestoreHome(std::env::var_os("HOME"));
    crate::testsupport::set_env("HOME", home.path().to_str().expect("utf-8 home"));
    let backend = Arc::new(pawork_auth::MemoryBackend::new());
    let (adapter, _dir) = settings_adapter("http://127.0.0.1:1".into(), backend).await;
    let provider = pawork_domain::ProviderId::from("glm-coding");

    let before = adapter
        .query(&query_envelope(AppQuery::ProviderAuthStatus {
            provider_id: Some(provider.clone()),
        }))
        .await
        .expect("status before set");
    let AppResponse::Data(before) = before else {
        panic!("ProviderAuthStatus must return Data: {before:?}")
    };
    assert!(before["default"].is_null(), "fresh config has no default");

    let response = adapter
        .command(&command_envelope(AppCommand::SetDefaultModel {
            provider_id: provider.clone(),
            model_id: "glm-5.2".into(),
        }))
        .await
        .expect("set default model");
    let AppResponse::Data(data) = response else {
        panic!("SetDefaultModel must return Data: {response:?}")
    };
    assert_eq!(data["provider_id"], "glm-coding");
    assert_eq!(data["model_id"], "glm-5.2");

    // 同会话重查：内存生效配置已同步为新 pair，无需 Host 重启。
    let after = adapter
        .query(&query_envelope(AppQuery::ProviderAuthStatus {
            provider_id: Some(provider),
        }))
        .await
        .expect("status after set");
    let AppResponse::Data(after) = after else {
        panic!("ProviderAuthStatus must return Data: {after:?}")
    };
    assert_eq!(
        after["default"],
        serde_json::json!({ "provider_id": "glm-coding", "model_id": "glm-5.2" })
    );

    // 写盘确实落在重定向后的全局配置文件。
    let config_path = pawork_workspace::config::global_config_path().expect("global path");
    let persisted = std::fs::read_to_string(&config_path).expect("persisted config");
    assert!(
        persisted.contains("default_provider = \"glm-coding\""),
        "persisted config misses default pair: {persisted}"
    );
}

#[tokio::test]
async fn set_proxy_url_updates_general_settings_within_same_session() {
    let _home_env = HOME_ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let home = tempfile::tempdir().expect("home tempdir");
    let _restore_home = RestoreHome(std::env::var_os("HOME"));
    crate::testsupport::set_env("HOME", home.path().to_str().expect("utf-8 home"));
    let backend = Arc::new(pawork_auth::MemoryBackend::new());
    let (adapter, _dir) = settings_adapter("http://127.0.0.1:1".into(), backend).await;
    let proxy = "http://127.0.0.1:7890";

    let before = adapter
        .query(&query_envelope(AppQuery::GeneralSettings))
        .await
        .expect("general settings before set");
    let AppResponse::Data(before) = before else {
        panic!("GeneralSettings must return Data: {before:?}")
    };
    assert!(
        before["proxy_url"].is_null(),
        "fresh config has no proxy_url"
    );

    let response = adapter
        .command(&command_envelope(AppCommand::SetProxyUrl {
            proxy_url: Some(proxy.into()),
        }))
        .await
        .expect("set proxy url");
    let AppResponse::Data(data) = response else {
        panic!("SetProxyUrl must return Data: {response:?}")
    };
    assert_eq!(data["proxy_url"], proxy);

    let after = adapter
        .query(&query_envelope(AppQuery::GeneralSettings))
        .await
        .expect("general settings after set");
    let AppResponse::Data(after) = after else {
        panic!("GeneralSettings must return Data: {after:?}")
    };
    assert_eq!(after["proxy_url"], proxy);

    let config_path = pawork_workspace::config::global_config_path().expect("global path");
    let persisted = std::fs::read_to_string(&config_path).expect("persisted config");
    assert!(
        persisted.contains("proxy_url = \"http://127.0.0.1:7890\""),
        "persisted config misses proxy_url: {persisted}"
    );
}

#[tokio::test]
async fn clear_proxy_url_updates_general_settings_to_null() {
    let _home_env = HOME_ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let home = tempfile::tempdir().expect("home tempdir");
    let _restore_home = RestoreHome(std::env::var_os("HOME"));
    crate::testsupport::set_env("HOME", home.path().to_str().expect("utf-8 home"));
    let backend = Arc::new(pawork_auth::MemoryBackend::new());
    let (adapter, _dir) = settings_adapter("http://127.0.0.1:1".into(), backend).await;

    adapter
        .command(&command_envelope(AppCommand::SetProxyUrl {
            proxy_url: Some("http://127.0.0.1:7890".into()),
        }))
        .await
        .expect("seed proxy url");

    let response = adapter
        .command(&command_envelope(AppCommand::SetProxyUrl {
            proxy_url: None,
        }))
        .await
        .expect("clear proxy url");
    let AppResponse::Data(data) = response else {
        panic!("SetProxyUrl clear must return Data: {response:?}")
    };
    assert!(
        data["proxy_url"].is_null(),
        "clear receipt must be null: {data}"
    );

    let after = adapter
        .query(&query_envelope(AppQuery::GeneralSettings))
        .await
        .expect("general settings after clear");
    let AppResponse::Data(after) = after else {
        panic!("GeneralSettings must return Data: {after:?}")
    };
    assert!(
        after["proxy_url"].is_null(),
        "requery after clear must be null: {after}"
    );

    let config_path = pawork_workspace::config::global_config_path().expect("global path");
    let persisted = std::fs::read_to_string(&config_path).expect("persisted config");
    assert!(
        !persisted.contains("proxy_url"),
        "cleared config still has proxy_url: {persisted}"
    );
}

#[tokio::test]
async fn set_proxy_url_rejects_invalid_url_and_keeps_old_value() {
    let _home_env = HOME_ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let home = tempfile::tempdir().expect("home tempdir");
    let _restore_home = RestoreHome(std::env::var_os("HOME"));
    crate::testsupport::set_env("HOME", home.path().to_str().expect("utf-8 home"));
    let backend = Arc::new(pawork_auth::MemoryBackend::new());
    let (adapter, _dir) = settings_adapter("http://127.0.0.1:1".into(), backend).await;
    let old = "http://127.0.0.1:7890";
    adapter
        .command(&command_envelope(AppCommand::SetProxyUrl {
            proxy_url: Some(old.into()),
        }))
        .await
        .expect("seed proxy url");
    let config_path = pawork_workspace::config::global_config_path().expect("global path");
    let seeded = std::fs::read_to_string(&config_path).expect("seed persisted");
    assert!(
        seeded.contains("proxy_url"),
        "seed did not persist: {seeded}"
    );

    let bad = "http://user:s3cret-proxy@not a url";
    let error = adapter
        .command(&command_envelope(AppCommand::SetProxyUrl {
            proxy_url: Some(bad.into()),
        }))
        .await
        .expect_err("invalid proxy must fail closed");
    assert_eq!(error.code, "invalid_proxy_url");
    assert!(
        !error.message.contains(bad) && !error.message.contains("s3cret-proxy"),
        "error leaks proxy URL: {}",
        error.message
    );

    let after = adapter
        .query(&query_envelope(AppQuery::GeneralSettings))
        .await
        .expect("general settings after invalid set");
    let AppResponse::Data(after) = after else {
        panic!("GeneralSettings must return Data: {after:?}")
    };
    assert_eq!(
        after["proxy_url"], old,
        "invalid set must keep old proxy_url"
    );

    let persisted = std::fs::read_to_string(&config_path).expect("persisted config");
    assert_eq!(persisted, seeded, "invalid set must not rewrite disk");
}

#[tokio::test]
async fn set_terminal_settings_updates_and_clears_shell_within_same_session() {
    let _home_env = HOME_ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let home = tempfile::tempdir().expect("home tempdir");
    let _restore_home = RestoreHome(std::env::var_os("HOME"));
    crate::testsupport::set_env("HOME", home.path().to_str().expect("utf-8 home"));
    let backend = Arc::new(pawork_auth::MemoryBackend::new());
    let (adapter, _dir) = settings_adapter("http://127.0.0.1:1".into(), backend).await;

    let before = adapter
        .query(&query_envelope(AppQuery::TerminalSettings))
        .await
        .expect("terminal settings before set");
    let AppResponse::Data(before) = before else {
        panic!("TerminalSettings must return Data: {before:?}")
    };
    assert_eq!(
        before,
        serde_json::json!({ "shell": None::<String>, "columns": 80, "rows": 24 }),
        "fresh config must report platform defaults"
    );

    #[cfg(unix)]
    let shell = "/bin/sh";
    #[cfg(windows)]
    let shell = "cmd.exe";
    let response = adapter
        .command(&command_envelope(AppCommand::SetTerminalSettings {
            shell: Some(shell.into()),
            columns: 120,
            rows: 40,
        }))
        .await
        .expect("set terminal settings");
    let AppResponse::Data(data) = response else {
        panic!("SetTerminalSettings must return Data: {response:?}")
    };
    assert_eq!(data["shell"], shell);
    assert_eq!(data["columns"], 120);
    assert_eq!(data["rows"], 40);

    let after = adapter
        .query(&query_envelope(AppQuery::TerminalSettings))
        .await
        .expect("terminal settings after set");
    let AppResponse::Data(after) = after else {
        panic!("TerminalSettings must return Data: {after:?}")
    };
    assert_eq!(after["shell"], shell);
    assert_eq!(after["columns"], 120);
    assert_eq!(after["rows"], 40);

    let config_path = pawork_workspace::config::global_config_path().expect("global path");
    let persisted = std::fs::read_to_string(&config_path).expect("persisted config");
    assert!(persisted.contains("[terminal]"), "missing [terminal]: {persisted}");
    assert!(
        persisted.contains(format!("shell = \"{shell}\"").as_str()),
        "persisted config misses shell: {persisted}"
    );

    // ADR-050 D3：shell=null 显式清除回平台默认，columns/rows 保持全态值。
    let response = adapter
        .command(&command_envelope(AppCommand::SetTerminalSettings {
            shell: None,
            columns: 120,
            rows: 40,
        }))
        .await
        .expect("clear terminal shell");
    let AppResponse::Data(data) = response else {
        panic!("SetTerminalSettings clear must return Data: {response:?}")
    };
    assert!(data["shell"].is_null(), "clear receipt must be null: {data}");

    let after = adapter
        .query(&query_envelope(AppQuery::TerminalSettings))
        .await
        .expect("terminal settings after clear");
    let AppResponse::Data(after) = after else {
        panic!("TerminalSettings must return Data: {after:?}")
    };
    assert!(after["shell"].is_null(), "requery after clear: {after}");
    assert_eq!(after["columns"], 120);
    assert_eq!(after["rows"], 40);

    let persisted = std::fs::read_to_string(&config_path).expect("persisted config");
    assert!(
        !persisted.contains("shell"),
        "cleared config still has shell key: {persisted}"
    );
    assert!(persisted.contains("columns = 120"), "{persisted}");
}

#[tokio::test]
async fn set_terminal_settings_rejects_invalid_values_and_keeps_old() {
    let _home_env = HOME_ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let home = tempfile::tempdir().expect("home tempdir");
    let _restore_home = RestoreHome(std::env::var_os("HOME"));
    crate::testsupport::set_env("HOME", home.path().to_str().expect("utf-8 home"));
    let backend = Arc::new(pawork_auth::MemoryBackend::new());
    let (adapter, _dir) = settings_adapter("http://127.0.0.1:1".into(), backend).await;

    #[cfg(unix)]
    let seeded_shell = "/bin/sh";
    #[cfg(windows)]
    let seeded_shell = "cmd.exe";
    adapter
        .command(&command_envelope(AppCommand::SetTerminalSettings {
            shell: Some(seeded_shell.into()),
            columns: 120,
            rows: 40,
        }))
        .await
        .expect("seed terminal settings");
    let config_path = pawork_workspace::config::global_config_path().expect("global path");
    let seeded = std::fs::read_to_string(&config_path).expect("seed persisted");

    for bad in [
        AppCommand::SetTerminalSettings {
            shell: Some("/definitely/missing/pawork-shell".into()),
            columns: 120,
            rows: 40,
        },
        AppCommand::SetTerminalSettings {
            shell: Some(seeded_shell.into()),
            columns: 1,
            rows: 40,
        },
        AppCommand::SetTerminalSettings {
            shell: Some(seeded_shell.into()),
            columns: 120,
            rows: 2000,
        },
    ] {
        let error = adapter
            .command(&command_envelope(bad))
            .await
            .expect_err("invalid terminal settings must fail closed");
        assert_eq!(error.code, "invalid_terminal_settings");
    }

    let after = adapter
        .query(&query_envelope(AppQuery::TerminalSettings))
        .await
        .expect("terminal settings after invalid set");
    let AppResponse::Data(after) = after else {
        panic!("TerminalSettings must return Data: {after:?}")
    };
    assert_eq!(after["shell"], seeded_shell);
    assert_eq!(after["columns"], 120);
    assert_eq!(after["rows"], 40);

    let persisted = std::fs::read_to_string(&config_path).expect("persisted config");
    assert_eq!(persisted, seeded, "invalid set must not rewrite disk");
}

#[tokio::test]
async fn terminal_create_applies_configured_shell_and_size() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (store, _) = pawork_storage::session::SessionStore::open(dir.path().join("session.db"))
        .await
        .expect("store");
    #[cfg(unix)]
    let configured_shell = "/usr/bin/true";
    #[cfg(windows)]
    let configured_shell = "cmd.exe";
    let mut config = pawork_workspace::config::PaworkConfig::default();
    config.terminal = Some(pawork_workspace::config::TerminalConfig {
        shell: Some(configured_shell.into()),
        columns: Some(97),
        rows: Some(31),
    });
    let mut core = AppCore::from_parts(
        Arc::new(MockProvider::sequence(Vec::new())),
        None,
        pawork_domain::ModelId::from("model-1"),
        pawork_domain::ProviderId::from("mock"),
        Some(store),
    )
    .with_state(
        config,
        Arc::new(pawork_auth::MemoryBackend::new()) as Arc<dyn pawork_auth::SecretBackend>,
    );
    core.attach_workspace(dir.path()).expect("attach workspace");
    core.configure_approval(
        crate::ApprovalMode::AskForDangerous,
        true,
        Arc::new(crate::DenyAllApprovals),
    );
    let adapter = GuiHostAdapter::new(Arc::new(core));

    let created = adapter
        .command(&command_envelope(AppCommand::TerminalCreate {
            workspace_id: WorkspaceId::from("ws-default"),
            working_directory: None,
        }))
        .await
        .expect("terminal_create");
    let AppResponse::Data(payload) = created else {
        panic!("terminal_create must return Data: {created:?}")
    };
    let terminal_id = payload["terminal_session_id"]
        .as_str()
        .expect("terminal_session_id")
        .to_string();

    // ADR-050 D4：size 生效值来自配置（pixel 0 由 PtyWindowSize::default 保持）。
    let owner = pawork_exec::OwnerSessionId::new("ws-default");
    let snapshot = adapter
        .pty
        .snapshot(&pawork_exec::TerminalId::new(&terminal_id), &owner)
        .expect("snapshot");
    assert_eq!(snapshot.size.cols, 97);
    assert_eq!(snapshot.size.rows, 31);
    assert_eq!(snapshot.size.pixel_width, 0);
    assert_eq!(snapshot.size.pixel_height, 0);

    // 配置 shell 真被用于 spawn：/usr/bin/true 立即以 exit_code=0 退出
    //（默认交互 shell 不会立即退出）。
    #[cfg(unix)]
    {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            let snapshot = adapter
                .pty
                .snapshot(&pawork_exec::TerminalId::new(&terminal_id), &owner)
                .expect("snapshot");
            if snapshot.state == pawork_exec::PtySessionState::Exited {
                assert_eq!(snapshot.exit_code, Some(0));
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "configured shell /usr/bin/true must exit promptly"
            );
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }
}

#[tokio::test]
async fn set_approval_mode_updates_permissions_settings_within_same_session() {
    let backend = Arc::new(pawork_auth::MemoryBackend::new());
    let (adapter, _dir) = settings_adapter("http://127.0.0.1:1".into(), backend).await;

    let before = adapter
        .query(&query_envelope(AppQuery::PermissionsSettings))
        .await
        .expect("permissions settings before set");
    let AppResponse::Data(before) = before else {
        panic!("PermissionsSettings must return Data: {before:?}")
    };
    assert_eq!(before["approval_mode"], "read_only");
    assert_eq!(before["workspace_trusted"], false);
    assert!(before["trust_workspaces_global"].is_null());
    // ADR-048 D1（实现期修订）：透出 Host 权威 attached workspace_id。
    let attached = adapter.core.read().await.workspace_id().to_string();
    assert_eq!(before["workspace_id"], attached.as_str());

    let response = adapter
        .command(&command_envelope(AppCommand::SetApprovalMode {
            mode: "ask_for_writes".into(),
        }))
        .await
        .expect("set approval mode");
    let AppResponse::Data(data) = response else {
        panic!("SetApprovalMode must return Data: {response:?}")
    };
    assert_eq!(data["approval_mode"], "ask_for_writes");

    // 未知值 fail-closed：Error 且旧值保留（ADR-048 D2）。
    let error = adapter
        .command(&command_envelope(AppCommand::SetApprovalMode {
            mode: "yolo".into(),
        }))
        .await
        .expect_err("unknown approval mode must fail closed");
    assert_eq!(error.code, "invalid_approval_mode");

    let after = adapter
        .query(&query_envelope(AppQuery::PermissionsSettings))
        .await
        .expect("permissions settings after set");
    let AppResponse::Data(after) = after else {
        panic!("PermissionsSettings must return Data: {after:?}")
    };
    assert_eq!(after["approval_mode"], "ask_for_writes");
    assert_eq!(after["workspace_trusted"], false);
    // ToolScheduler 必须同步 Arc-swap，否则之后启动的 run 仍走旧 ReadOnly 闸门。
    assert_eq!(
        adapter.core.read().await.scheduler_approval_snapshot(),
        (crate::ApprovalMode::AskForWrites, false)
    );
}

#[tokio::test]
async fn workspace_trust_toggles_session_trust_for_attached_workspace() {
    let backend = Arc::new(pawork_auth::MemoryBackend::new());
    let (adapter, _dir) = settings_adapter("http://127.0.0.1:1".into(), backend).await;
    let workspace_id = adapter.core.read().await.workspace_id().clone();

    let response = adapter
        .command(&command_envelope(AppCommand::WorkspaceTrust {
            workspace_id: workspace_id.clone(),
            trusted: true,
        }))
        .await
        .expect("workspace trust");
    let AppResponse::Data(data) = response else {
        panic!("WorkspaceTrust must return Data: {response:?}")
    };
    assert_eq!(data["workspace_trusted"], true);

    let after = adapter
        .query(&query_envelope(AppQuery::PermissionsSettings))
        .await
        .expect("permissions settings after trust");
    let AppResponse::Data(after) = after else {
        panic!("PermissionsSettings must return Data: {after:?}")
    };
    assert_eq!(after["workspace_trusted"], true);
    // 之后启动的 run 克隆新 scheduler Arc（check_gate 用 config.workspace_trusted）。
    assert!(adapter.core.read().await.workspace_trusted());
    assert_eq!(
        adapter.core.read().await.scheduler_approval_snapshot(),
        (crate::ApprovalMode::ReadOnly, true)
    );
}

#[tokio::test]
async fn workspace_trust_rejects_mismatched_workspace_id_fail_closed() {
    let backend = Arc::new(pawork_auth::MemoryBackend::new());
    let (adapter, _dir) = settings_adapter("http://127.0.0.1:1".into(), backend).await;

    let error = adapter
        .command(&command_envelope(AppCommand::WorkspaceTrust {
            workspace_id: WorkspaceId::from("ws-other"),
            trusted: true,
        }))
        .await
        .expect_err("mismatched workspace must fail closed");
    assert_eq!(error.code, "unknown_workspace");

    let after = adapter
        .query(&query_envelope(AppQuery::PermissionsSettings))
        .await
        .expect("permissions settings after mismatch");
    let AppResponse::Data(after) = after else {
        panic!("PermissionsSettings must return Data: {after:?}")
    };
    assert_eq!(
        after["workspace_trusted"], false,
        "trust must stay old value"
    );
    assert_eq!(
        adapter.core.read().await.scheduler_approval_snapshot(),
        (crate::ApprovalMode::ReadOnly, false),
        "fail-closed must not rebuild scheduler trust"
    );
}

#[tokio::test]
async fn auth_set_api_key_verifies_replaces_and_masks_end_to_end() {
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let secret = "sk-live-plaintext-1234567890abcd";
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/models"))
        .and(header("authorization", &format!("Bearer {secret}")))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({ "data": [{ "id": "glm-5.2" }] })),
        )
        // hyper 对幂等 GET 在连接被对端关闭时会自动重发一次，
        // 计数只要求「至少一次携带候选 key 的已认证请求」。
        .expect(1..)
        .mount(&server)
        .await;

    let backend = Arc::new(pawork_auth::MemoryBackend::new());
    let (adapter, dir) = settings_adapter(server.uri(), backend).await;
    let mut events = adapter.subscribe_events();

    let response = adapter
        .command(&command_envelope(AppCommand::AuthSetApiKey {
            provider_id: pawork_domain::ProviderId::from("glm-coding"),
            api_key: pawork_protocol::ApiKeySecret::new(secret),
        }))
        .await
        .expect("verify-then-replace succeeds");
    let AppResponse::Data(data) = response else {
        panic!("AuthSetApiKey must return Data: {response:?}")
    };
    assert_eq!(data["provider_id"], "glm-coding");
    assert_eq!(data["method"], "api_key");
    assert!(data["verified_at"].as_str().is_some());
    let response_wire = serde_json::to_string(&data).expect("serialize response");
    assert!(!response_wire.contains(secret), "response leaks plaintext");

    let event = events.try_recv().expect("AuthChanged::Succeeded event");
    let event_wire = serde_json::to_string(&event).expect("serialize event");
    assert!(
        event_wire.contains("\"succeeded\""),
        "missing succeeded state"
    );
    assert!(!event_wire.contains(secret), "event leaks plaintext");

    server.verify().await;

    let status = adapter
        .query(&query_envelope(AppQuery::ProviderAuthStatus {
            provider_id: Some(pawork_domain::ProviderId::from("glm-coding")),
        }))
        .await
        .expect("provider auth status");
    let AppResponse::Data(status) = status else {
        panic!("ProviderAuthStatus must return Data: {status:?}")
    };
    let entry = &status["providers"][0];
    assert_eq!(entry["provider_id"], "glm-coding");
    assert_eq!(entry["display_name"], "GLM Coding");
    assert_eq!(entry["auth_methods"], serde_json::json!(["api_key"]));
    assert_eq!(entry["auth"]["type"], "connected");
    assert_eq!(entry["auth"]["method"], "api_key");
    let masked = entry["auth"]["masked_credential"].as_str().expect("masked");
    assert!(!masked.contains(secret), "status leaks plaintext: {masked}");

    // ADR-046 D6 Secret 负断言：命令完成后，临时目录内任何持久化文件
    //（command ledger / session.db 及其 -wal/-shm）都不得含明文——
    // ledger 只缓存脱敏响应信封，请求 payload 不落盘。
    for entry in std::fs::read_dir(dir.path()).expect("read tempdir") {
        let path = entry.expect("tempdir entry").path();
        let bytes = std::fs::read(&path).expect("read persisted file");
        let persisted = String::from_utf8_lossy(&bytes);
        assert!(
            !persisted.contains(secret),
            "persisted file {} leaks plaintext",
            path.display()
        );
    }
}

#[tokio::test]
async fn auth_set_api_key_verify_failure_keeps_old_credential() {
    use pawork_auth::SecretBackend as _;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let old_secret = "sk-old-secret-00000000000000";
    let new_secret = "sk-new-invalid-1234567890ab";
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/models"))
        .respond_with(ResponseTemplate::new(401).set_body_string("invalid api key"))
        .expect(1)
        .mount(&server)
        .await;

    let backend = Arc::new(pawork_auth::MemoryBackend::new());
    backend
        .store("pawork.glm-coding", "default", old_secret)
        .expect("seed old credential");
    let (adapter, _dir) = settings_adapter(server.uri(), backend.clone()).await;
    let mut events = adapter.subscribe_events();

    let error = adapter
        .command(&command_envelope(AppCommand::AuthSetApiKey {
            provider_id: pawork_domain::ProviderId::from("glm-coding"),
            api_key: pawork_protocol::ApiKeySecret::new(new_secret),
        }))
        .await
        .expect_err("verification must fail closed");
    assert_eq!(error.code, "auth_verify");
    assert!(!error.message.contains(new_secret), "error leaks plaintext");

    assert_eq!(
        backend
            .get("pawork.glm-coding", "default")
            .expect("old credential retained"),
        old_secret,
        "failed verification must not replace the stored key"
    );
    let event = events.try_recv().expect("AuthChanged::Failed event");
    let event_wire = serde_json::to_string(&event).expect("serialize event");
    assert!(event_wire.contains("\"failed\""), "missing failed state");
    assert!(!event_wire.contains(new_secret), "event leaks plaintext");
    server.verify().await;
}

// ---- SET-6c 工具与 MCP（ADR-049）----

/// 构造带 MCP 段生效配置的 adapter：merged 视图经 extra 注入（模拟
/// loader 已发现 Global 层），盘上内容由测试自行播种保持一致。
async fn mcp_settings_adapter(mcp: serde_json::Value) -> (GuiHostAdapter, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let (store, _) = pawork_storage::session::SessionStore::open(dir.path().join("session.db"))
        .await
        .expect("store");
    let mut config = pawork_workspace::config::PaworkConfig::default();
    config
        .providers
        .push(pawork_workspace::config::ProviderConfig {
            id: "glm-coding".into(),
            base_url: Some("http://127.0.0.1:1".into()),
            default: None,
        });
    config.extra.insert("mcp".into(), mcp);
    let backend = Arc::new(pawork_auth::MemoryBackend::new());
    let core = AppCore::from_parts(
        Arc::new(MockProvider::sequence(Vec::new())),
        None,
        pawork_domain::ModelId::from("glm-5.2"),
        pawork_domain::ProviderId::from("glm-coding"),
        Some(store),
    )
    .with_state(config, backend as Arc<dyn pawork_auth::SecretBackend>);
    (GuiHostAdapter::new(Arc::new(core)), dir)
}

#[tokio::test]
async fn mcp_server_remove_clears_disk_secret_and_memory() {
    // 写盘与 mcp-auth.json 目标均经 HOME 重定向到临时目录。
    let _home_env = HOME_ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let home = tempfile::tempdir().expect("home tempdir");
    let _restore_home = RestoreHome(std::env::var_os("HOME"));
    crate::testsupport::set_env("HOME", home.path().to_str().expect("utf-8 home"));

    // Global 盘上播种 demo + keep（demo 带一个 SecretRef header）。
    let config_path = pawork_workspace::config::global_config_path().expect("global path");
    std::fs::create_dir_all(config_path.parent().expect("config parent"))
        .expect("create config dir");
    let seeded = r#"trust_workspaces = true

[mcp.servers.demo]
transport = { kind = "http", url = "https://mcp.example.com/mcp", headers = { Authorization = { service = "pawork.mcp.demo", account = "cred-1" } } }

[mcp.servers.keep]
transport = { kind = "http", url = "https://keep.example.com/mcp" }
"#;
    std::fs::write(&config_path, seeded).expect("seed global config");

    let (adapter, _dir) = mcp_settings_adapter(serde_json::json!({
        "servers": {
            "demo": {
                "transport": {
                    "kind": "http",
                    "url": "https://mcp.example.com/mcp",
                    "headers": {
                        "Authorization": {
                            "service": "pawork.mcp.demo",
                            "account": "cred-1"
                        }
                    }
                }
            },
            "keep": {
                "transport": { "kind": "http", "url": "https://keep.example.com/mcp" }
            }
        }
    }))
    .await;

    let secret_backend = crate::extensions::mcp_secret_backend();
    secret_backend
        .store("pawork.mcp.demo", "cred-1", "sk-mcp-value")
        .expect("store mcp secret");

    let response = adapter
        .command(&command_envelope(AppCommand::McpServerRemove {
            name: "demo".into(),
        }))
        .await
        .expect("mcp_server_remove");
    let AppResponse::Data(data) = response else {
        panic!("McpServerRemove must return Data: {response:?}")
    };
    let names: Vec<&str> = data["servers"]
        .as_array()
        .expect("servers array")
        .iter()
        .map(|server| server["name"].as_str().expect("server name"))
        .collect();
    assert_eq!(names, vec!["keep"]);

    // 盘：demo 条目消失；未知字段与其它 server 原样保留。
    let persisted = std::fs::read_to_string(&config_path).expect("persisted config");
    assert!(!persisted.contains("demo"), "demo must be gone: {persisted}");
    assert!(persisted.contains("trust_workspaces = true"));
    // toml 序列化会把单键子表折叠为 [mcp.servers.keep.transport] 形态的
    // header，按前缀断言，不写死 header 形态。
    assert!(
        persisted.contains("mcp.servers.keep"),
        "keep must be preserved: {persisted}"
    );

    // 密：pawork.mcp.demo 下的 SecretRef 已清理。
    assert!(matches!(
        secret_backend.get("pawork.mcp.demo", "cred-1"),
        Err(pawork_auth::AuthError::NotFound)
    ));

    // 内存：同会话重查 mcp_list 不再含 demo。
    let list = adapter
        .query(&query_envelope(AppQuery::McpList))
        .await
        .expect("mcp list after remove");
    let AppResponse::Data(list) = list else {
        panic!("McpList must return Data: {list:?}")
    };
    let list_wire = serde_json::to_string(&list).expect("serialize list");
    assert!(
        !list_wire.contains("demo"),
        "mcp_list must not contain demo: {list_wire}"
    );
    assert!(list_wire.contains("keep"));
}

#[tokio::test]
async fn mcp_server_remove_unknown_name_fails_closed() {
    let _home_env = HOME_ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let home = tempfile::tempdir().expect("home tempdir");
    let _restore_home = RestoreHome(std::env::var_os("HOME"));
    crate::testsupport::set_env("HOME", home.path().to_str().expect("utf-8 home"));

    let config_path = pawork_workspace::config::global_config_path().expect("global path");
    std::fs::create_dir_all(config_path.parent().expect("config parent"))
        .expect("create config dir");
    let seeded = r#"[mcp.servers.demo]
transport = { kind = "http", url = "https://mcp.example.com/mcp", headers = { Authorization = { service = "pawork.mcp.demo", account = "cred-1" } } }
"#;
    std::fs::write(&config_path, seeded).expect("seed global config");

    let (adapter, _dir) = mcp_settings_adapter(serde_json::json!({
        "servers": {
            "demo": {
                "transport": {
                    "kind": "http",
                    "url": "https://mcp.example.com/mcp",
                    "headers": {
                        "Authorization": {
                            "service": "pawork.mcp.demo",
                            "account": "cred-1"
                        }
                    }
                }
            }
        }
    }))
    .await;

    let secret_backend = crate::extensions::mcp_secret_backend();
    secret_backend
        .store("pawork.mcp.demo", "cred-1", "sk-mcp-value")
        .expect("store mcp secret");

    let error = adapter
        .command(&command_envelope(AppCommand::McpServerRemove {
            name: "ghost".into(),
        }))
        .await
        .expect_err("unknown server must fail closed");
    assert_eq!(error.code, "unknown_mcp_server");

    // 三处皆不动：盘字节一致、SecretRef 保留、内存 mcp_list 仍含 demo。
    let after = std::fs::read_to_string(&config_path).expect("config after");
    assert_eq!(seeded, after);
    assert_eq!(
        secret_backend
            .get("pawork.mcp.demo", "cred-1")
            .expect("secret must be kept"),
        "sk-mcp-value"
    );
    let list = adapter
        .query(&query_envelope(AppQuery::McpList))
        .await
        .expect("mcp list after failure");
    let AppResponse::Data(list) = list else {
        panic!("McpList must return Data: {list:?}")
    };
    assert!(
        serde_json::to_string(&list)
            .expect("serialize list")
            .contains("demo"),
        "demo must remain in mcp_list: {list}"
    );
}

#[tokio::test]
async fn mcp_test_unknown_name_fails_closed_and_keeps_list() {
    let (adapter, _dir) = mcp_settings_adapter(serde_json::json!({
        "servers": {
            "demo": { "transport": { "kind": "http", "url": "http://127.0.0.1:1/mcp" } }
        }
    }))
    .await;

    let before = adapter
        .query(&query_envelope(AppQuery::McpList))
        .await
        .expect("mcp list before");
    let error = adapter
        .command(&command_envelope(AppCommand::McpTest { name: "ghost".into() }))
        .await
        .expect_err("unknown server must fail closed");
    assert_eq!(error.code, "unknown_mcp_server");
    let after = adapter
        .query(&query_envelope(AppQuery::McpList))
        .await
        .expect("mcp list after");
    assert_eq!(
        serde_json::to_string(&before).expect("serialize before"),
        serde_json::to_string(&after).expect("serialize after"),
        "mcp_list must be unchanged by the failed test"
    );
}

#[tokio::test]
async fn mcp_test_unreachable_http_fails_closed_and_keeps_slot_state() {
    let ws = tempfile::tempdir().expect("workspace tempdir");
    let (adapter, _dir) = mcp_settings_adapter(serde_json::json!({
        "servers": {
            "demo": { "transport": { "kind": "http", "url": "http://127.0.0.1:1/mcp" } }
        }
    }))
    .await;
    // test_one_mcp 需要已附加 workspace 才会走到建连；预置 connected slot，
    // 断言失败路径不覆盖既有 slot 状态（fail-closed）。
    {
        let mut core = adapter.core.write().await;
        core.attach_workspace(ws.path()).expect("attach workspace");
        core.extensions.mcp_servers.push(crate::extensions::McpServerSlot {
            name: "demo".into(),
            transport: "http".into(),
            state: "connected".into(),
            last_error: None,
            tools: Vec::new(),
            client: None,
        });
    }

    let before = adapter
        .query(&query_envelope(AppQuery::McpList))
        .await
        .expect("mcp list before");
    let error = adapter
        .command(&command_envelope(AppCommand::McpTest { name: "demo".into() }))
        .await
        .expect_err("unreachable http server must fail closed");
    assert_eq!(error.code, "app_error");
    let after = adapter
        .query(&query_envelope(AppQuery::McpList))
        .await
        .expect("mcp list after");
    assert_eq!(
        serde_json::to_string(&before).expect("serialize before"),
        serde_json::to_string(&after).expect("serialize after"),
        "slot state must be unchanged by the failed test"
    );
    let AppResponse::Data(list) = after else {
        panic!("McpList must return Data: {after:?}")
    };
    let demo = list["servers"]
        .as_array()
        .expect("servers array")
        .iter()
        .find(|server| server["name"] == "demo")
        .expect("demo slot retained");
    assert_eq!(demo["state"], "connected");
    assert_eq!(demo["last_error"], serde_json::Value::Null);
}

#[tokio::test]
async fn mcp_server_remove_same_name_in_workspace_layer_fails_closed() {
    let _home_env = HOME_ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let home = tempfile::tempdir().expect("home tempdir");
    let _restore_home = RestoreHome(std::env::var_os("HOME"));
    crate::testsupport::set_env("HOME", home.path().to_str().expect("utf-8 home"));

    // Global 盘上播种 demo；workspace 层再定义同名 demo（跨层同名）。
    let config_path = pawork_workspace::config::global_config_path().expect("global path");
    std::fs::create_dir_all(config_path.parent().expect("config parent"))
        .expect("create config dir");
    let seeded = r#"[mcp.servers.demo]
transport = { kind = "http", url = "https://mcp.example.com/mcp", headers = { Authorization = { service = "pawork.mcp.demo", account = "cred-1" } } }
"#;
    std::fs::write(&config_path, seeded).expect("seed global config");
    let ws = tempfile::tempdir().expect("workspace tempdir");
    let ws_config = ws.path().join(".pawork").join("config.toml");
    std::fs::create_dir_all(ws_config.parent().expect("ws config parent"))
        .expect("create ws config dir");
    std::fs::write(
        &ws_config,
        "[mcp.servers.demo]\ntransport = { kind = \"http\", url = \"https://workspace.example.com/mcp\" }\n",
    )
    .expect("seed workspace config");

    let (adapter, _dir) = mcp_settings_adapter(serde_json::json!({
        "servers": {
            "demo": {
                "transport": {
                    "kind": "http",
                    "url": "https://mcp.example.com/mcp",
                    "headers": {
                        "Authorization": {
                            "service": "pawork.mcp.demo",
                            "account": "cred-1"
                        }
                    }
                }
            }
        }
    }))
    .await;
    adapter
        .core
        .write()
        .await
        .attach_workspace(ws.path())
        .expect("attach workspace");

    let secret_backend = crate::extensions::mcp_secret_backend();
    secret_backend
        .store("pawork.mcp.demo", "cred-1", "sk-mcp-value")
        .expect("store mcp secret");

    let error = adapter
        .command(&command_envelope(AppCommand::McpServerRemove {
            name: "demo".into(),
        }))
        .await
        .expect_err("cross-layer same name must fail closed");
    assert_eq!(error.code, "mcp_server_defined_in_other_layers");
    assert!(
        error.message.contains("also defined"),
        "message must state the server is also defined elsewhere: {}",
        error.message
    );
    assert!(
        error.message.contains("workspace"),
        "message must name the other layer: {}",
        error.message
    );

    // 三处皆不动：盘字节一致、SecretRef 保留、内存 mcp_list 仍含 demo。
    let after = std::fs::read_to_string(&config_path).expect("config after");
    assert_eq!(seeded, after);
    assert!(ws_config.is_file(), "workspace layer untouched");
    assert_eq!(
        secret_backend
            .get("pawork.mcp.demo", "cred-1")
            .expect("secret must be kept"),
        "sk-mcp-value"
    );
    let list = adapter
        .query(&query_envelope(AppQuery::McpList))
        .await
        .expect("mcp list after failure");
    let AppResponse::Data(list) = list else {
        panic!("McpList must return Data: {list:?}")
    };
    assert!(
        serde_json::to_string(&list)
            .expect("serialize list")
            .contains("demo"),
        "demo must remain in mcp_list: {list}"
    );
}

// ---- SET-4 A3：xAI 双认证（auth_set_api_key 走 verify-then-replace 门）----

#[tokio::test]
async fn xai_auth_set_api_key_main_path_connects_via_api_key() {
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let secret = "xai-live-key-1234567890abcdef";
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/models"))
        .and(header("authorization", &format!("Bearer {secret}")))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({ "data": [{ "id": "grok-4" }] })),
        )
        .expect(1..)
        .mount(&server)
        .await;

    let backend = Arc::new(pawork_auth::MemoryBackend::new());
    let (adapter, _dir) =
        settings_adapter_for_channel("xai", "grok-4", server.uri(), backend).await;

    let response = adapter
        .command(&command_envelope(AppCommand::AuthSetApiKey {
            provider_id: pawork_domain::ProviderId::from("xai"),
            api_key: pawork_protocol::ApiKeySecret::new(secret),
        }))
        .await
        .expect("xai api key verify-then-replace succeeds");
    let AppResponse::Data(data) = response else {
        panic!("AuthSetApiKey must return Data: {response:?}")
    };
    assert_eq!(data["provider_id"], "xai");
    assert_eq!(data["method"], "api_key");
    server.verify().await;

    let status = adapter
        .query(&query_envelope(AppQuery::ProviderAuthStatus {
            provider_id: Some(pawork_domain::ProviderId::from("xai")),
        }))
        .await
        .expect("provider auth status");
    let AppResponse::Data(status) = status else {
        panic!("ProviderAuthStatus must return Data: {status:?}")
    };
    let entry = &status["providers"][0];
    assert_eq!(entry["provider_id"], "xai");
    assert_eq!(
        entry["auth_methods"],
        serde_json::json!(["oauth", "api_key"])
    );
    // 双认证通道按实际存储形态展示：api key 凭证在，显示 method api_key。
    assert_eq!(entry["auth"]["type"], "connected");
    assert_eq!(entry["auth"]["method"], "api_key");
}

#[tokio::test]
async fn xai_auth_set_api_key_replaces_stored_oauth_credential() {
    use pawork_auth::SecretBackend as _;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let secret = "xai-replacement-key-0000000001";
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/models"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({ "data": [{ "id": "grok-4" }] })),
        )
        .expect(1..)
        .mount(&server)
        .await;

    let backend = Arc::new(pawork_auth::MemoryBackend::new());
    let provider = pawork_domain::ProviderId::from("xai");
    pawork_auth::store_default_oauth_token(
        backend.as_ref(),
        provider.clone(),
        &pawork_auth::TokenSet {
            access_token: "xai-old-oauth-access".into(),
            refresh_token: Some("xai-old-oauth-refresh".into()),
            id_token: None,
            expires_in: Some(3600),
            token_type: "Bearer".into(),
            scope: Some("grok-cli:access".into()),
        },
    )
    .expect("seed old oauth credential");

    let (adapter, _dir) =
        settings_adapter_for_channel("xai", "grok-4", server.uri(), backend.clone()).await;
    adapter
        .command(&command_envelope(AppCommand::AuthSetApiKey {
            provider_id: provider.clone(),
            api_key: pawork_protocol::ApiKeySecret::new(secret),
        }))
        .await
        .expect("switching auth method must succeed");

    // 替换语义：一切换认证方式 = 替换连接——旧 OAuth 条目被移除。
    assert!(
        pawork_auth::load_default_oauth_meta(backend.as_ref(), &provider)
            .expect("load meta")
            .is_none(),
        "old oauth meta must be removed"
    );
    assert!(
        pawork_auth::load_default_oauth_credential(backend.as_ref(), &provider)
            .expect("load credential")
            .is_none(),
        "old oauth credential must be removed"
    );
    assert_eq!(
        backend
            .get("pawork.xai", "default")
            .expect("api key stored"),
        secret
    );
    server.verify().await;
}

#[tokio::test]
async fn xai_api_key_verification_flight_rejects_auth_cancel() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let secret = "xai-cancel-guard-key-000000001";
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/models"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_delay(std::time::Duration::from_millis(3000))
                .set_body_json(serde_json::json!({ "data": [{ "id": "grok-4" }] })),
        )
        .expect(1..)
        .mount(&server)
        .await;

    let backend = Arc::new(pawork_auth::MemoryBackend::new());
    let (adapter, _dir) =
        settings_adapter_for_channel("xai", "grok-4", server.uri(), backend).await;
    let mut events = adapter.subscribe_events();

    let set_envelope = command_envelope(AppCommand::AuthSetApiKey {
        provider_id: pawork_domain::ProviderId::from("xai"),
        api_key: pawork_protocol::ApiKeySecret::new(secret),
    });
    let (set_outcome, cancel_outcome) = tokio::join!(adapter.command(&set_envelope), async {
        // 等待 api-key 验证 flight 真正登记（auth_state 报 connecting）再取消，
        // 避免竞态下取消落在 flight 登记之前。
        for _ in 0..150 {
            let status = adapter
                .query(&query_envelope(AppQuery::ProviderAuthStatus {
                    provider_id: Some(pawork_domain::ProviderId::from("xai")),
                }))
                .await
                .expect("auth status poll");
            let AppResponse::Data(status) = status else {
                panic!("ProviderAuthStatus must return Data: {status:?}")
            };
            if status["providers"][0]["auth"]["type"] == "connecting" {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        adapter
            .command(&command_envelope(AppCommand::AuthCancel {
                provider_id: pawork_domain::ProviderId::from("xai"),
            }))
            .await
    },);

    // D3：api-key 验证 flight 不可取消——拒绝取消且登记保留，验证本身完成。
    let cancel_error = cancel_outcome.expect_err("cancel of api-key flight must be rejected");
    assert_eq!(cancel_error.code, "unsupported");
    let response = set_outcome.expect("verification must complete despite rejected cancel attempt");
    let AppResponse::Data(data) = response else {
        panic!("AuthSetApiKey must return Data: {response:?}")
    };
    assert_eq!(data["method"], "api_key");

    // 拒绝取消不发 Cancelled；事件流首个认证事件是验证成功。
    let event = events.try_recv().expect("AuthChanged event");
    let event_wire = serde_json::to_string(&event).expect("serialize event");
    assert!(
        event_wire.contains("\"succeeded\""),
        "expected Succeeded event first: {event_wire}"
    );
    assert!(
        !event_wire.contains("\"cancelled\""),
        "rejected cancel must not emit Cancelled: {event_wire}"
    );
    server.verify().await;
}
