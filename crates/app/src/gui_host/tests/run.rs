use super::*;
use pawork_testkit::{MockProvider, MockScript};

fn naming_mock_provider(scripts: Vec<MockScript>) -> MockProvider {
    MockProvider::sequence(scripts).with_models(vec![pawork_domain::ModelDefinition {
        id: pawork_domain::ModelId::from("model-1"),
        display_name: "model-1".into(),
        context_window_tokens: 0,
        max_output_tokens: 0,
        capabilities: pawork_domain::ModelCapabilities::default(),
    }])
}

async fn assert_session_title(adapter: &GuiHostAdapter, session: &pawork_domain::SessionId, expected: &str) {
    let response = adapter
        .command(&command_envelope(AppCommand::SessionOpen {
            session_id: session.clone(),
        }))
        .await
        .expect("session open");
    let AppResponse::Data(data) = response else {
        panic!("SessionOpen must return data: {response:?}");
    };
    assert_eq!(data["title"].as_str(), Some(expected), "{data}");
}

#[tokio::test]
async fn run_success_auto_titles_placeholder_session_and_broadcasts() {
    let dir = tempfile::tempdir().expect("store");
    let (store, _) = pawork_storage::session::SessionStore::open(dir.path().join("session.db"))
        .await
        .expect("store");
    let provider = naming_mock_provider(vec![
        MockScript::new().text("ok").complete(),
        // 命名输出带前后空白与第二行：必须 trim 取首个非空行。
        MockScript::new().text("  帮我修登录 bug\n多余的第二行\n").complete(),
    ]);
    let mut core = AppCore::from_parts(
        Arc::new(provider),
        None,
        pawork_domain::ModelId::from("model-1"),
        pawork_domain::ProviderId::from("mock"),
        Some(store),
    );
    core.config.naming_provider = Some("mock".into());
    core.config.naming_model = Some("model-1".into());
    let session = core.create_session("New session").await.expect("session");
    let adapter = GuiHostAdapter::new(Arc::new(core));
    let mut events = adapter.subscribe_events();
    let response = adapter
        .command(&command_envelope(AppCommand::RunStart {
            session_id: session.clone(),
            user_message: "登录页 500 了，帮我看看".into(),
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
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let envelope = tokio::time::timeout_at(deadline, events.recv())
            .await
            .expect("naming should broadcast SessionMetaChanged")
            .expect("event channel");
        if let AppEvent::SessionMetaChanged {
            session_id,
            title,
            archived,
        } = &envelope.payload
        {
            assert_eq!(session_id, &session);
            assert_eq!(title, "帮我修登录 bug");
            assert!(!archived);
            break;
        }
    }
    assert_session_title(&adapter, &session, "帮我修登录 bug").await;
}

#[tokio::test]
async fn auto_title_without_naming_config_skips_provider_call() {
    let dir = tempfile::tempdir().expect("store");
    let (store, _) = pawork_storage::session::SessionStore::open(dir.path().join("session.db"))
        .await
        .expect("store");
    let provider = naming_mock_provider(vec![MockScript::new().text("ok").complete()]);
    let core = AppCore::from_parts(
        Arc::new(provider.clone()),
        None,
        pawork_domain::ModelId::from("model-1"),
        pawork_domain::ProviderId::from("mock"),
        Some(store),
    );
    // 不配置 naming_provider / naming_model：不得调用命名模型。
    let session = core.create_session("New session").await.expect("session");
    let adapter = GuiHostAdapter::new(Arc::new(core));
    let mut events = adapter.subscribe_events();
    let response = adapter
        .command(&command_envelope(AppCommand::RunStart {
            session_id: session.clone(),
            user_message: "hello".into(),
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
    wait_run_registry_drains(&adapter.runs(), &run_id).await;
    // 给错误的命名触发留出观察窗口后再断言。
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    assert_eq!(
        provider.calls().len(),
        1,
        "naming must not call the provider without naming config"
    );
    assert_session_title(&adapter, &session, "New session").await;
}

#[tokio::test]
async fn auto_title_failure_keeps_placeholder_title() {
    let dir = tempfile::tempdir().expect("store");
    let (store, _) = pawork_storage::session::SessionStore::open(dir.path().join("session.db"))
        .await
        .expect("store");
    let provider = naming_mock_provider(vec![
        MockScript::new().text("ok").complete(),
        MockScript::new().fail(pawork_domain::ProviderError::new(
            pawork_domain::ProviderErrorKind::Timeout,
            "naming provider down",
        )),
    ]);
    let mut core = AppCore::from_parts(
        Arc::new(provider.clone()),
        None,
        pawork_domain::ModelId::from("model-1"),
        pawork_domain::ProviderId::from("mock"),
        Some(store),
    );
    core.config.naming_provider = Some("mock".into());
    core.config.naming_model = Some("model-1".into());
    let session = core.create_session("New session").await.expect("session");
    let adapter = GuiHostAdapter::new(Arc::new(core));
    let mut events = adapter.subscribe_events();
    let response = adapter
        .command(&command_envelope(AppCommand::RunStart {
            session_id: session.clone(),
            user_message: "hello".into(),
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
    // 等命名尝试真实发生（第二次 provider 调用）再断言保留占位名。
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    while provider.calls().len() < 2 {
        assert!(
            tokio::time::Instant::now() < deadline,
            "naming attempt must reach the provider"
        );
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    while let Ok(envelope) = events.try_recv() {
        assert!(
            !matches!(&envelope.payload, AppEvent::SessionMetaChanged { .. }),
            "failed naming must not broadcast: {:?}",
            envelope.payload
        );
    }
    assert_session_title(&adapter, &session, "New session").await;
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
        super::super::handlers::run_start::seal_run_without_terminal(
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

/// ADR-055 D4：生效模型或显式请求模型被禁用时 RunStart 结构化
/// fail-closed（model_disabled），不启动 Run、不登记 ActiveGuiRun。
#[tokio::test]
async fn run_start_fails_closed_when_model_disabled() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (store, _) = pawork_storage::session::SessionStore::open(dir.path().join("session.db"))
        .await
        .expect("store");
    let provider =
        MockProvider::sequence(Vec::new()).with_models(vec![pawork_domain::ModelDefinition {
            id: pawork_domain::ModelId::from("model-2"),
            display_name: "Model 2".into(),
            context_window_tokens: 8_000,
            max_output_tokens: 1_024,
            capabilities: Default::default(),
        }]);
    let mut core = AppCore::from_parts(
        Arc::new(provider),
        None,
        pawork_domain::ModelId::from("model-1"),
        pawork_domain::ProviderId::from("mock"),
        Some(store),
    );
    core.config
        .providers
        .push(pawork_workspace::config::ProviderConfig {
            id: "mock".into(),
            disabled_models: vec!["model-1".into(), "model-2".into()],
            ..Default::default()
        });
    let session = core.create_session("disabled").await.expect("session");
    let adapter = GuiHostAdapter::new(Arc::new(core));

    // 无切换请求：会话当前生效模型（model-1）被禁用 → fail-closed。
    let error = adapter
        .command(&command_envelope(AppCommand::RunStart {
            session_id: session.clone(),
            user_message: "hi".into(),
            model: None,
            provider: None,
            profile: None,
        }))
        .await
        .expect_err("disabled effective model must fail closed");
    assert_eq!(error.code, "model_disabled", "error: {error:?}");
    assert!(
        adapter.runs.active().is_empty(),
        "failed RunStart must not register an active run"
    );

    // 显式切换到禁用模型（model-2）：switch_model 同闸 fail-closed。
    let error = adapter
        .command(&command_envelope(AppCommand::RunStart {
            session_id: session,
            user_message: "hi".into(),
            model: Some(pawork_domain::ModelId::from("model-2")),
            provider: None,
            profile: None,
        }))
        .await
        .expect_err("disabled requested model must fail closed");
    assert_eq!(error.code, "model_disabled", "error: {error:?}");
    assert!(adapter.runs.active().is_empty());
}
