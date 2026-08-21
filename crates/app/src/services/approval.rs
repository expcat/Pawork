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
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Mutex;

    use async_trait::async_trait;
    use pawork_domain::{ApprovalDecision, CancellationToken};
    use pawork_storage::session::SessionStore;
    use pawork_testkit::{MockProvider, MockScript};

    use crate::testsupport::{RecordingEvents, user_hello};
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
        let (core, _dir) =
            write_ready_core(ApprovalMode::AskForWrites, true, host.clone(), workspace.path())
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
}
