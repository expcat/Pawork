    use super::*;
    use crate::approval::ApprovalPromptHost;
    use pawork_domain::{
        ApprovalDecision, CancellationToken, CommandId, ContentPart, Message, MessageId,
        MessageMetadata, MessageRole, RunId, TextContent, Timestamp, WorkspaceId,
    };
    use pawork_protocol::{ActorIdentity, AppQuery, CommandSource, RunState, TimelineItemKind};
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
