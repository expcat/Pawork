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
            workspace_id: Some(WorkspaceId::from("ws-default")),
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

/// ADR-054 D1：缺省 workspace_id 落盘 NULL，snapshot 不带归属字段。
#[tokio::test]
async fn session_create_without_workspace_lands_unassigned() {
    let (core, _dir, _session) = core_with_turn().await;
    let host: Arc<dyn GuiHost> = Arc::new(GuiHostAdapter::new(core));
    let response = host
        .command(&command_envelope(AppCommand::SessionCreate {
            workspace_id: None,
            title: None,
        }))
        .await
        .expect("accepted");
    let AppResponse::Data(data) = &response else {
        panic!("SessionCreate must return Data: {response:?}");
    };
    assert_eq!(data["title"], json!("New session"));
    assert!(data.get("workspace_id").is_none(), "{data}");
    let session_id = data["session_id"].as_str().expect("session_id").to_string();
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
            entry.get("session_id").and_then(Value::as_str) == Some(session_id.as_str())
                && entry.get("workspace_id").is_none()
        }),
        "unassigned SessionCreate must omit workspace_id in snapshot: {sessions:?}"
    );
}

/// ADR-054 D2/D3/D5：改名 / 归档写盘、回执写后状态并广播
/// SessionMetaChanged；空 title 结构化拒绝且不写盘。
#[tokio::test]
async fn session_rename_and_archive_update_meta_and_broadcast() {
    let (core, _dir, _session) = core_with_turn().await;
    let session = core
        .create_session_with_workspace("before", WorkspaceId::from("ws-default"))
        .await
        .expect("seed session");
    let adapter = GuiHostAdapter::new(core);
    let mut events = adapter.subscribe_events();

    let blank = adapter
        .command(&command_envelope(AppCommand::SessionRename {
            session_id: session.clone(),
            title: "   ".into(),
        }))
        .await
        .expect_err("blank title is rejected");
    assert_eq!(blank.code, "invalid_title");
    assert!(matches!(events.try_recv(), Err(_)));

    let response = adapter
        .command(&command_envelope(AppCommand::SessionRename {
            session_id: session.clone(),
            title: "renamed".into(),
        }))
        .await
        .expect("rename");
    let AppResponse::Data(data) = response else {
        panic!("SessionRename must return Data");
    };
    assert_eq!(data.get("title").and_then(Value::as_str), Some("renamed"));
    assert_eq!(data.get("archived").and_then(Value::as_bool), Some(false));
    let event = events.try_recv().expect("SessionMetaChanged event");
    assert!(matches!(
        event.payload,
        AppEvent::SessionMetaChanged {
            ref title,
            archived: false,
            ..
        } if title == "renamed"
    ));

    let response = adapter
        .command(&command_envelope(AppCommand::SessionArchive {
            session_id: session.clone(),
            archived: true,
        }))
        .await
        .expect("archive");
    let AppResponse::Data(data) = response else {
        panic!("SessionArchive must return Data");
    };
    assert_eq!(data.get("archived").and_then(Value::as_bool), Some(true));
    let event = events.try_recv().expect("archive SessionMetaChanged");
    assert!(matches!(
        event.payload,
        AppEvent::SessionMetaChanged { archived: true, .. }
    ));

    let listed = adapter
        .core
        .read()
        .await
        .list_sessions()
        .await
        .expect("list");
    assert!(
        !listed
            .iter()
            .any(|record| record.session_id == session.as_str()),
        "archived session must be hidden from list_sessions"
    );

    let missing = adapter
        .command(&command_envelope(AppCommand::SessionArchive {
            session_id: SessionId::from("ses-missing"),
            archived: true,
        }))
        .await
        .expect_err("missing session is fail-closed");
    assert_eq!(missing.code, "not_found");
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
