use super::*;
use pawork_testkit::{MockProvider, MockScript};
use serde_json::Value;

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

pub(super) async fn append_waiting_write(
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

pub(super) fn idle_core(store: pawork_storage::session::SessionStore) -> AppCore {
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

pub(super) fn replay_types(events: &[AgentEventEnvelope]) -> Vec<&'static str> {
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

