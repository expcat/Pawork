use super::*;
use pawork_testkit::{MockProvider, MockScript};

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

pub(super) fn gui_command_envelope(
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

