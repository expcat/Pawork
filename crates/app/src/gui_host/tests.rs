    use super::*;
    use crate::approval::ApprovalPromptHost;
    use pawork_domain::{
        ApprovalDecision, CancellationToken, CommandId, ContentPart, Message, MessageId,
        MessageMetadata, MessageRole, QueryId, RunId, TenantId, TextContent, Timestamp, WorkspaceId,
    };
    use pawork_protocol::{ActorIdentity, AppEvent, AppQuery, AppResponseEnvelope, CommandSource, EventStream, RunState, TimelineItemKind, DEFAULT_CONTROL_PLANE_TENANT};
    use pawork_protocol::app::registry::{command_entries, query_entries};
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
        wire.iter().any(|event| {
            matches!(event, AppEvent::Diagnostic { code, .. } if code == "run.failed")
        })
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
        assert_eq!(adapter.runs().active().len(), 0, "failed expand must not leave a ghost run");
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
        let session = core.create_session("gui-fail-terminal").await.expect("session");
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
        let session = core.create_session("gui-cancel-terminal").await.expect("session");
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
            .command(&command_envelope(AppCommand::RunCancel { run_id: run.clone() }))
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
        let provider =
            MockProvider::sequence(vec![MockScript::new().text("unreachable").complete()]);
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
        let provider =
            MockProvider::sequence(vec![MockScript::new().text("unreachable").complete()]);
        let core = AppCore::from_parts(
            Arc::new(provider),
            None,
            pawork_domain::ModelId::from("model-1"),
            pawork_domain::ProviderId::from("mock"),
            Some(store),
        );
        let session = core.create_session("gate-durable-seal").await.expect("session");
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
            AppEvent::Diagnostic { level, code, message } => {
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

    #[tokio::test]
    async fn command_idempotency_survives_restart() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = dir.path().join("session.db");
        let (store, _) = pawork_storage::session::SessionStore::open(&db)
            .await
            .expect("store");
        let core = AppCore::from_parts(
            Arc::new(MockProvider::sequence(vec![MockScript::new().text("ok").complete()])),
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
            Arc::new(MockProvider::sequence(vec![MockScript::new().text("ok").complete()])),
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
            Arc::new(MockProvider::sequence(vec![MockScript::new().text("idle").complete()])),
            None,
            pawork_domain::ModelId::from("model-1"),
            pawork_domain::ProviderId::from("mock"),
            Some(store),
        );
        let adapter = GuiHostAdapter::new(Arc::new(core));
        let mut ledger = crate::IdempotencyStore::for_store(
            adapter.session_store().await.expect("store").command_ledger(),
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
            ledger.check(&tenant, &conflict_id, None).await.expect("reserve"),
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
        adapter.persist_command_response(
            &ledger,
            &tenant,
            &conflict_id,
            Some("shared-key"),
            conflict,
        )
        .await;
        let captured = capture.events();
        capture.dismiss();
        assert_eq!(adapter.command_record_failure_count().await, 1);
        assert!(
            matches!(events.try_recv(), Err(tokio::sync::broadcast::error::TryRecvError::Empty)),
            "IdempotencyConflict must not send a client frame"
        );
        let emitted = captured.iter().find(|event| {
            event.fields.get("code").map(String::as_str) == Some("degrade.idempotency_conflict")
        }).unwrap_or_else(|| panic!("record failure must emit tracing: {captured:?}"));
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
            Arc::new(MockProvider::sequence(vec![MockScript::new().text("idle").complete()])),
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
            Arc::new(MockProvider::sequence(vec![MockScript::new().text("idle").complete()])),
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
            Arc::new(MockProvider::sequence(vec![MockScript::new().text("idle").complete()])),
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
                AgentEvent::MessageCommitted { message }
                    if message.role == MessageRole::Tool =>
                {
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
                AgentEvent::ToolApprovalResponded { decision, comment, .. } => {
                    Some((decision.clone(), comment.clone()))
                }
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
        let session = core.create_session("queued-broadcast").await.expect("session");
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
            Arc::new(MockProvider::sequence(vec![MockScript::new().text("ok").complete()])),
            None,
            pawork_domain::ModelId::from("model-1"),
            pawork_domain::ProviderId::from("mock"),
            Some(store),
        );
        let adapter = Arc::new(GuiHostAdapter::new(Arc::new(core)));
        let mut ledger = crate::IdempotencyStore::for_store(
            adapter.session_store().await.expect("store").command_ledger(),
        )
        .with_scope("automation");
        ledger.share_waiters_from(&adapter.waiters);
        let tenant = TenantId::new(DEFAULT_CONTROL_PLANE_TENANT);
        let holder = CommandId::from("cmd-inflight-holder");
        assert!(matches!(
            ledger.check(&tenant, &holder, Some("shared-hang")).await.expect("reserve"),
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
            Arc::new(MockProvider::sequence(vec![MockScript::new().text("ok").complete()])),
            None,
            pawork_domain::ModelId::from("model-1"),
            pawork_domain::ProviderId::from("mock"),
            Some(store),
        );
        let adapter = GuiHostAdapter::new(Arc::new(core));
        let mut ledger = crate::IdempotencyStore::for_store(
            adapter.session_store().await.expect("store").command_ledger(),
        )
        .with_scope("automation");
        ledger.share_waiters_from(&adapter.waiters);
        let tenant = TenantId::new(DEFAULT_CONTROL_PLANE_TENANT);
        let holder = CommandId::from("cmd-drop-wakeup-holder");
        let waiter_id = CommandId::from("cmd-drop-wakeup-waiter");
        assert!(matches!(
            ledger.check(&tenant, &holder, Some("drop-wakeup")).await.expect("reserve"),
            crate::IdempotencyCheck::New
        ));
        assert!(matches!(
            ledger.check(&tenant, &waiter_id, Some("drop-wakeup")).await.expect("inflight"),
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
            Arc::new(MockProvider::sequence(vec![MockScript::new().text("idle").complete()])),
            None,
            pawork_domain::ModelId::from("model-1"),
            pawork_domain::ProviderId::from("mock"),
            Some(store),
        );
        let adapter = GuiHostAdapter::new(Arc::new(core));
        let mut ledger = crate::IdempotencyStore::for_store(
            adapter.session_store().await.expect("store").command_ledger(),
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
            ledger.check(&tenant, &conflict_id, None).await.expect("reserve"),
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
        adapter.persist_command_response(
            &ledger,
            &tenant,
            &conflict_id,
            Some("shared-key"),
            conflict,
        )
        .await;
        assert_eq!(adapter.command_record_failure_count().await, 1);
        match ledger
            .check(&tenant, &conflict_id, Some("shared-key"))
            .await
            .expect("keyed retry after release")
        {
            crate::IdempotencyCheck::Replay(replay) => {
                assert_eq!(replay.response, primed.response, "keyed retry must Replay the primed holder, not re-execute");
            }
            other => panic!("expected Replay of primed key holder, got {other:?}"),
        }
        match ledger
            .check(&tenant, &conflict_id, None)
            .await
            .expect("reenter after record failure")
        {
            crate::IdempotencyCheck::InFlight(_) => panic!(
                "record failure must release inflight so the same command_id is not stuck"
            ),
            crate::IdempotencyCheck::New | crate::IdempotencyCheck::Replay(_) => {}
        }
    }
