//! Approval 领域服务：审批模式、workspace 信任与决策宿主的自持状态。

use std::sync::Arc;

use pawork_policy::ApprovalMode;

use crate::approval::{ApprovalPromptHost, DenyAllApprovals};

/// Approval 领域服务（R4 波 A：三字段从 AppCore 平移，行为零变化）。
pub(crate) struct ApprovalService {
    mode: ApprovalMode,
    workspace_trusted: bool,
    host: Arc<dyn ApprovalPromptHost>,
}

impl ApprovalService {
    pub(crate) fn new() -> Self {
        Self {
            mode: ApprovalMode::ReadOnly,
            workspace_trusted: false,
            host: Arc::new(DenyAllApprovals),
        }
    }

    /// 设置审批模式、workspace 信任与决策宿主。须在 attach_workspace 之前调用。
    pub(crate) fn configure(
        &mut self,
        mode: ApprovalMode,
        workspace_trusted: bool,
        host: Arc<dyn ApprovalPromptHost>,
    ) {
        self.mode = mode;
        self.workspace_trusted = workspace_trusted;
        self.host = host;
    }

    /// ADR-048 D2：运行时切换审批模式（GUI Settings 专用）。只改内存态，
    /// 对之后启动的 run 生效（run 启动时快照）；不持久化，不触碰进行中
    /// run。启动装配仍走 Self::configure（一次性设三字段）。
    pub(crate) fn set_mode(&mut self, mode: ApprovalMode) {
        self.mode = mode;
    }

    /// ADR-048 D3：运行时切换当前会话的 workspace 信任（内存态，不写盘，
    /// 重启后跟随 Global 配置）。生效边界同 set_mode。
    pub(crate) fn set_workspace_trusted(&mut self, workspace_trusted: bool) {
        self.workspace_trusted = workspace_trusted;
    }

    pub(crate) fn mode(&self) -> ApprovalMode {
        self.mode
    }

    pub(crate) fn workspace_trusted(&self) -> bool {
        self.workspace_trusted
    }

    pub(crate) fn host(&self) -> Arc<dyn ApprovalPromptHost> {
        Arc::clone(&self.host)
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;
    use std::sync::Mutex;

    use async_trait::async_trait;
    use pawork_domain::{ApprovalDecision, CancellationToken, MessageRole};
    use pawork_storage::session::SessionStore;
    use pawork_testkit::{MockProvider, MockScript};

    use crate::gui_server::GuiHost;
    use crate::testsupport::{user_hello, RecordingEvents};
    use crate::{AppCore, ApprovalAsk, ApprovalMode, ApprovalPromptHost, DenyAllApprovals};

    struct ScriptedHost {
        queue: Mutex<Vec<ApprovalDecision>>,
        asked: AtomicU64,
    }

    impl ScriptedHost {
        fn new(queue: Vec<ApprovalDecision>) -> Arc<Self> {
            Arc::new(Self {
                queue: Mutex::new(queue),
                asked: AtomicU64::new(0),
            })
        }
    }

    #[async_trait]
    impl ApprovalPromptHost for ScriptedHost {
        async fn decide(&self, _ask: &ApprovalAsk, _cancel: CancellationToken) -> ApprovalDecision {
            self.asked.fetch_add(1, Ordering::SeqCst);
            self.queue.lock().expect("queue").remove(0)
        }
    }

    struct PanicHost;

    #[async_trait]
    impl ApprovalPromptHost for PanicHost {
        async fn decide(&self, ask: &ApprovalAsk, _cancel: CancellationToken) -> ApprovalDecision {
            panic!("approval host should not be asked for {}", ask.tool_name);
        }
    }

    async fn write_ready_core(
        mode: ApprovalMode,
        trusted: bool,
        host: Arc<dyn ApprovalPromptHost>,
        workspace: &Path,
    ) -> (AppCore, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("store");
        let path = dir.path().join("session.db");
        let (store, _) = SessionStore::open(&path).await.expect("store");
        let provider = MockProvider::sequence(vec![
            MockScript::new()
                .tool_call(
                    "write_file",
                    serde_json::json!({"path": "notes.txt", "content": "hello-write"}),
                )
                .complete_with(pawork_domain::StopReason::ToolUse),
            MockScript::new().text("wrote notes").complete(),
        ]);
        let mut core = AppCore::from_parts(
            Arc::new(provider),
            None,
            pawork_domain::ModelId::from("model-1"),
            pawork_domain::ProviderId::from("mock"),
            Some(store),
        );
        core.open_checkpoints(dir.path().join("artifacts"))
            .await
            .expect("artifacts");
        core.configure_approval(mode, trusted, host);
        core.attach_workspace(workspace).expect("attach");
        (core, dir)
    }

    #[tokio::test]
    async fn ask_for_writes_approved_once_persists_file_and_event_pair() {
        let workspace = tempfile::tempdir().expect("workspace");
        let host = ScriptedHost::new(vec![ApprovalDecision::ApprovedOnce]);
        let (core, _dir) = write_ready_core(
            ApprovalMode::AskForWrites,
            true,
            host.clone(),
            workspace.path(),
        )
        .await;
        let session = core.create_session("write").await.expect("create");
        let sink = RecordingEvents::default();
        core.chat_turn(
            &session,
            vec![user_hello()],
            &sink,
            CancellationToken::new(),
        )
        .await
        .expect("turn");

        let types = sink.types();
        let requested = types
            .iter()
            .position(|name| *name == "ToolApprovalRequested")
            .expect("requested");
        let responded = types
            .iter()
            .position(|name| *name == "ToolApprovalResponded")
            .expect("responded");
        let started = types
            .iter()
            .position(|name| *name == "ToolExecutionStarted")
            .expect("started");
        assert!(requested < responded);
        assert!(responded < started);
        assert_eq!(host.asked.load(Ordering::SeqCst), 1);
        let written = std::fs::read_to_string(workspace.path().join("notes.txt")).expect("file");
        assert_eq!(written, "hello-write");
        assert!(types.contains(&"CheckpointCreated"));
        let created = types
            .iter()
            .position(|name| *name == "CheckpointCreated")
            .expect("created");
        assert!(responded < created);
        assert!(created < started);
        core.shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    async fn write_checkpoint_rollback_restores_and_appends() {
        let workspace = tempfile::tempdir().expect("workspace");
        let host = ScriptedHost::new(vec![ApprovalDecision::ApprovedOnce]);
        let (core, _dir) =
            write_ready_core(ApprovalMode::AskForWrites, true, host, workspace.path()).await;
        let session = core.create_session("rollback").await.expect("create");
        let sink = RecordingEvents::default();
        core.chat_turn(
            &session,
            vec![user_hello()],
            &sink,
            CancellationToken::new(),
        )
        .await
        .expect("turn");
        assert_eq!(
            std::fs::read_to_string(workspace.path().join("notes.txt")).expect("file"),
            "hello-write"
        );

        let listed = core.list_checkpoints(&session).await.expect("list");
        assert!(!listed.is_empty());
        let spec = listed
            .iter()
            .find(|item| item.tool_call_id.is_none())
            .or_else(|| listed.first())
            .expect("checkpoint")
            .checkpoint_id
            .clone();
        let before_rollback = core
            .store()
            .expect("store")
            .replay_events(&session, 1, usize::MAX)
            .await
            .expect("replay");
        let last_seq = before_rollback.last().expect("tail").sequence.value();

        let outcome = core.rollback(&session, &spec).await.expect("rollback");
        assert!(!workspace.path().join("notes.txt").exists());
        assert!(outcome.restored.iter().any(|path| path == "notes.txt"));

        let after = core
            .store()
            .expect("store")
            .replay_events(&session, 1, usize::MAX)
            .await
            .expect("replay after");
        assert_eq!(after.last().expect("last").sequence.value(), last_seq + 1);
        assert!(matches!(
            after.last().expect("last").payload,
            pawork_domain::AgentEvent::CheckpointRolledBack { .. }
        ));
        let diff = core.session_diff(&session).await.expect("diff");
        assert!(diff.files.is_empty());
        core.shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    async fn deny_all_emits_approval_pair_and_does_not_write() {
        let workspace = tempfile::tempdir().expect("workspace");
        let (core, _dir) = write_ready_core(
            ApprovalMode::AskForWrites,
            true,
            Arc::new(DenyAllApprovals),
            workspace.path(),
        )
        .await;
        let session = core.create_session("deny").await.expect("create");
        let sink = RecordingEvents::default();
        core.chat_turn(
            &session,
            vec![user_hello()],
            &sink,
            CancellationToken::new(),
        )
        .await
        .expect("turn");

        let types = sink.types();
        assert!(types.contains(&"ToolApprovalRequested"));
        assert!(types.contains(&"ToolApprovalResponded"));
        assert!(!types.contains(&"ToolExecutionStarted"));
        assert!(!workspace.path().join("notes.txt").exists());
        core.shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    async fn read_only_trusted_write_is_denied_without_asking() {
        let workspace = tempfile::tempdir().expect("workspace");
        let (core, _dir) = write_ready_core(
            ApprovalMode::ReadOnly,
            true,
            Arc::new(PanicHost),
            workspace.path(),
        )
        .await;
        let session = core.create_session("readonly").await.expect("create");
        let sink = RecordingEvents::default();
        core.chat_turn(
            &session,
            vec![user_hello()],
            &sink,
            CancellationToken::new(),
        )
        .await
        .expect("turn");

        assert!(!sink.types().contains(&"ToolApprovalRequested"));
        assert!(!workspace.path().join("notes.txt").exists());
        core.shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    async fn untrusted_never_ask_denies_write_without_asking() {
        let workspace = tempfile::tempdir().expect("workspace");
        let (core, _dir) = write_ready_core(
            ApprovalMode::NeverAsk,
            false,
            Arc::new(PanicHost),
            workspace.path(),
        )
        .await;
        let session = core.create_session("untrusted").await.expect("create");
        let sink = RecordingEvents::default();
        core.chat_turn(
            &session,
            vec![user_hello()],
            &sink,
            CancellationToken::new(),
        )
        .await
        .expect("turn");

        assert!(!sink.types().contains(&"ToolApprovalRequested"));
        assert!(!workspace.path().join("notes.txt").exists());
        core.shutdown().await.expect("shutdown");
    }

    struct HangHost;

    #[async_trait]
    impl ApprovalPromptHost for HangHost {
        async fn decide(&self, _ask: &ApprovalAsk, cancel: CancellationToken) -> ApprovalDecision {
            cancel.cancelled().await;
            ApprovalDecision::Cancelled
        }
    }

    #[tokio::test]
    async fn k02_crash_keeps_pending_and_detached_resolve_does_not_rerun() {
        let workspace = tempfile::tempdir().expect("workspace");
        let dir = tempfile::tempdir().expect("store");
        let db = dir.path().join("session.db");
        let (store, _) = SessionStore::open(&db).await.expect("store");
        let probe = store.clone();
        let provider = MockProvider::sequence(vec![
            MockScript::new()
                .tool_call(
                    "write_file",
                    serde_json::json!({"path": "notes.txt", "content": "hello-write"}),
                )
                .complete_with(pawork_domain::StopReason::ToolUse),
            MockScript::new().text("wrote notes").complete(),
        ]);
        let mut core = AppCore::from_parts(
            Arc::new(provider),
            None,
            pawork_domain::ModelId::from("model-1"),
            pawork_domain::ProviderId::from("mock"),
            Some(store),
        );
        core.open_checkpoints(dir.path().join("artifacts"))
            .await
            .expect("artifacts");
        core.configure_approval(ApprovalMode::AskForWrites, true, Arc::new(HangHost));
        core.attach_workspace(workspace.path()).expect("attach");
        let session = core.create_session("crash").await.expect("create");
        let sink = RecordingEvents::default();
        let cancel = CancellationToken::new();
        let turn_session = session.clone();
        let handle = tokio::spawn(async move {
            core.chat_turn(&turn_session, vec![user_hello()], &sink, cancel)
                .await
        });
        let mut waiting = None;
        for _ in 0..100 {
            if let Ok(snap) = probe.projection_snapshot(&session).await {
                if snap
                    .tool_calls
                    .iter()
                    .any(|call| call.state == "waiting_for_approval")
                {
                    waiting = Some(snap);
                    break;
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        let waiting = waiting.expect("requested should persist before wait");
        assert_eq!(waiting.tool_calls[0].state, "waiting_for_approval");
        handle.abort();
        let _ = handle.await;
        drop(probe);

        let (store, _) = SessionStore::open(&db).await.expect("reopen after crash");
        let mut core = AppCore::from_parts(
            Arc::new(MockProvider::sequence(vec![MockScript::new()
                .text("idle")
                .complete()])),
            None,
            pawork_domain::ModelId::from("model-1"),
            pawork_domain::ProviderId::from("mock"),
            Some(store),
        );
        core.configure_approval(ApprovalMode::AskForWrites, true, Arc::new(HangHost));
        core.attach_workspace(workspace.path()).expect("attach");
        let messages = core
            .resume_messages_keep_pending(&session)
            .await
            .expect("keep pending");
        assert!(messages
            .iter()
            .all(|message| message.role != MessageRole::Tool));
        let snap = core
            .store()
            .expect("store")
            .projection_snapshot(&session)
            .await
            .expect("still waiting");
        assert_eq!(snap.tool_calls[0].state, "waiting_for_approval");
        let run_id = snap.tool_calls[0].run_id.clone();
        let tool_call_id = snap.tool_calls[0].tool_call_id.clone();
        let adapter = crate::GuiHostAdapter::new(std::sync::Arc::new(core));
        adapter
            .command(&pawork_protocol::AppCommandEnvelope {
                api_version: pawork_protocol::API_VERSION,
                command_id: pawork_domain::CommandId::from("cmd-k02-deny"),
                source: pawork_protocol::CommandSource::Automation,
                identity: pawork_protocol::ActorIdentity::System,
                expected_revision: None,
                idempotency_key: None,
                issued_at: pawork_domain::Timestamp::from_unix_millis(1),
                command: pawork_protocol::AppCommand::ToolApprove {
                    run_id,
                    tool_call_id,
                    decision: pawork_protocol::ApprovalDecision::Deny,
                },
            })
            .await
            .expect("tool approve deny after crash");
        let after = adapter
            .session_store()
            .await
            .expect("store")
            .replay_events(&session, 1, usize::MAX)
            .await
            .expect("after");
        let types: Vec<_> = after
            .iter()
            .map(|envelope| match &envelope.payload {
                pawork_domain::AgentEvent::ToolApprovalRequested { .. } => "ToolApprovalRequested",
                pawork_domain::AgentEvent::ToolApprovalResponded { .. } => "ToolApprovalResponded",
                pawork_domain::AgentEvent::ToolExecutionStarted { .. } => "ToolExecutionStarted",
                pawork_domain::AgentEvent::ToolExecutionCompleted { .. } => {
                    "ToolExecutionCompleted"
                }
                pawork_domain::AgentEvent::MessageCommitted { message }
                    if message.role == MessageRole::Tool =>
                {
                    "MessageCommitted.tool"
                }
                _ => "other",
            })
            .collect();
        assert!(types.contains(&"ToolApprovalRequested"));
        assert!(types.contains(&"ToolApprovalResponded"));
        assert!(types.contains(&"ToolExecutionCompleted"));
        assert!(types.contains(&"MessageCommitted.tool"));
        assert!(!types.contains(&"ToolExecutionStarted"));
        let responded = after
            .iter()
            .find_map(|envelope| match &envelope.payload {
                pawork_domain::AgentEvent::ToolApprovalResponded {
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
        let completed_err = after.iter().find_map(|envelope| match &envelope.payload {
            pawork_domain::AgentEvent::ToolExecutionCompleted { result, .. } => {
                Some(result.is_error)
            }
            _ => None,
        });
        assert_eq!(completed_err, Some(true));
        assert!(after.iter().any(|envelope| matches!(
            envelope.payload,
            pawork_domain::AgentEvent::MessageCommitted { .. }
        )));
        assert!(!workspace.path().join("notes.txt").exists());
        let sequences: Vec<_> = after
            .iter()
            .map(|envelope| envelope.sequence.value())
            .collect();
        assert_eq!(sequences, (1..=after.len() as u64).collect::<Vec<_>>());
        let store = adapter.session_store().await.expect("store clone");
        let core = AppCore::from_parts(
            std::sync::Arc::new(MockProvider::sequence(vec![MockScript::new()
                .text("idle")
                .complete()])),
            None,
            pawork_domain::ModelId::from("model-1"),
            pawork_domain::ProviderId::from("mock"),
            Some(store),
        );
        let again_len = core
            .resume_messages_keep_pending(&session)
            .await
            .expect("second resume")
            .len();
        let replayed = core
            .store()
            .expect("store")
            .replay_events(&session, 1, usize::MAX)
            .await
            .expect("replay2");
        assert_eq!(replayed.len(), after.len());
        assert_eq!(again_len, messages.len() + 1);
        drop(core);
        adapter.shutdown().await.expect("shutdown adapter");
    }

    #[tokio::test]
    async fn write_snapshot_failure_emits_diagnostic_persisted_and_broadcast() {
        let workspace = tempfile::tempdir().expect("workspace");
        let dir = tempfile::tempdir().expect("store");
        let (store, _) = SessionStore::open(dir.path().join("session.db"))
            .await
            .expect("store");
        // ../ 路径逃出 workspace roots：snapshot_before_write 判 PathEscape，
        // 快照失败发诊断，写工具仍照常执行（工具层同样拒绝越界路径）。
        let provider = MockProvider::sequence(vec![
            MockScript::new()
                .tool_call(
                    "write_file",
                    serde_json::json!({"path": "../escape.txt", "content": "nope"}),
                )
                .complete_with(pawork_domain::StopReason::ToolUse),
            MockScript::new().text("write attempted").complete(),
        ]);
        let mut core = AppCore::from_parts(
            Arc::new(provider),
            None,
            pawork_domain::ModelId::from("model-1"),
            pawork_domain::ProviderId::from("mock"),
            Some(store),
        );
        core.open_checkpoints(dir.path().join("artifacts"))
            .await
            .expect("artifacts");
        core.configure_approval(
            ApprovalMode::AskForWrites,
            true,
            ScriptedHost::new(vec![ApprovalDecision::ApprovedOnce]),
        );
        core.attach_workspace(workspace.path()).expect("attach");
        let session = core
            .create_session("snapshot-failed")
            .await
            .expect("create");

        let bus = Arc::new(crate::GuiEventBus::new(512));
        let mut subscription = bus.subscribe();
        let broadcast =
            crate::GuiBroadcastSink::new(bus, pawork_domain::CoreInstanceId::from("instance-test"));
        core.chat_turn(
            &session,
            vec![user_hello()],
            &broadcast,
            CancellationToken::new(),
        )
        .await
        .expect("turn");

        let replayed = core
            .store()
            .expect("store")
            .replay_events(&session, 1, usize::MAX)
            .await
            .expect("replay");
        let diagnostic = replayed
            .iter()
            .find(|envelope| {
                matches!(
                    &envelope.payload,
                    pawork_domain::AgentEvent::Diagnostic { code, .. }
                        if code == "checkpoint.snapshot_failed"
                )
            })
            .expect("snapshot failure diagnostic persisted");
        let pawork_domain::AgentEvent::Diagnostic { details, .. } = &diagnostic.payload else {
            unreachable!();
        };
        assert_eq!(
            details.get("path").and_then(serde_json::Value::as_str),
            Some("../escape.txt")
        );
        assert!(details
            .get("error")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .contains("parent traversal"));
        assert!(replayed.iter().any(|envelope| matches!(
            &envelope.payload,
            pawork_domain::AgentEvent::RunCompleted { .. }
        )));

        let mut broadcast_found = false;
        while let Ok(event) = subscription.try_recv() {
            if let pawork_protocol::AppEvent::Diagnostic { code, message, .. } = &event.payload {
                if code == "checkpoint.snapshot_failed" {
                    broadcast_found = true;
                    let parsed: serde_json::Value =
                        serde_json::from_str(message).expect("details json");
                    assert_eq!(
                        parsed.get("path").and_then(serde_json::Value::as_str),
                        Some("../escape.txt")
                    );
                    assert_eq!(
                        parsed.get("message").and_then(serde_json::Value::as_str),
                        Some("checkpoint snapshot failed — write proceeded without rollback point")
                    );
                }
            }
        }
        assert!(
            broadcast_found,
            "diagnostic should broadcast to GUI clients"
        );
        core.shutdown().await.expect("shutdown");
    }
}
