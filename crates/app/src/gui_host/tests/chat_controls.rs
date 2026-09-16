use super::*;

#[tokio::test]
async fn chat_browser_tool_waits_for_approval_and_persists_actual_reply() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = pawork_storage::session::SessionStore::open(dir.path().join("session.db"))
        .await
        .unwrap();
    let provider = MockProvider::sequence(vec![
        MockScript::new()
            .tool_call("browser", json!({"action":"read"}))
            .complete(),
        MockScript::new().text("page received").complete(),
    ]);
    let mut core = AppCore::from_parts(
        Arc::new(provider),
        None,
        "model-1".into(),
        "mock".into(),
        Some(store.clone()),
    );
    core.configure_approval(
        pawork_policy::ApprovalMode::AskForDangerous,
        true,
        Arc::new(crate::DenyAllApprovals),
    );
    core.attach_workspace(dir.path()).unwrap();
    let session = core.create_session("browser test").await.unwrap();
    let adapter = GuiHostAdapter::new(Arc::new(core));
    let source = CommandSource::LocalGui {
        client_id: "browser-client".into(),
    };
    let mut request = command_envelope(AppCommand::RunStart {
        session_id: session.clone(),
        user_message: "read browser".into(),
        model: None,
        provider: None,
        profile: None,
    });
    request.source = source.clone();
    let mut events = adapter.subscribe_events();
    let AppResponse::Accepted {
        run_id: Some(run), ..
    } = adapter.command(&request).await.unwrap()
    else {
        panic!("run not accepted");
    };
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let event = events.recv().await.unwrap();
            if let AppEvent::ToolApprovalRequired { tool_call_id, .. } = event.payload {
                assert!(
                    adapter.browser.claim(&run, &source).is_null(),
                    "no request before approval"
                );
                adapter
                    .approvals
                    .resolve(&run, &tool_call_id, ApprovalDecision::ApprovedOnce)
                    .unwrap();
                break;
            }
        }
    })
    .await
    .unwrap();
    let claimed = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let result = adapter.browser.claim(&run, &source);
            if !result.is_null() {
                break result;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    adapter
        .browser
        .respond(
            claimed["request_id"].as_str().unwrap(),
            &source,
            json!({"ok":true,"data":{"text":"BROWSER-REAL-REPLY"}}),
        )
        .unwrap();
    wait_run_completed(&mut events, &run).await;
    let persisted = store.replay_events(&session, 1, 100).await.unwrap();
    let serialized = serde_json::to_string(&persisted).unwrap();
    assert!(serialized.contains("BROWSER-REAL-REPLY"));
    let core = adapter.core.read().await;
    assert!(core.tool_names().contains(&"terminal"));
    assert!(core.tool_names().contains(&"browser"));
    core.resume_messages_keep_pending(&session).await.unwrap();
    assert!(
        adapter.browser.claim(&run, &source).is_null(),
        "history must not dispatch"
    );
}
