use super::*;
use pawork_testkit::{MockProvider, MockScript};
use serde_json::{json, Value};

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

