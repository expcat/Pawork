use super::*;
use pawork_testkit::{MockProvider, MockScript};
use serde_json::Value;

// ------------------------------------------------------------------
// ADR-045：terminal_close 命令与 TerminalExited live 事件。

pub(super) async fn terminal_adapter() -> (GuiHostAdapter, tempfile::TempDir) {
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

pub(super) async fn create_terminal(adapter: &GuiHostAdapter) -> String {
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

pub(super) async fn wait_terminal_exited(
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
