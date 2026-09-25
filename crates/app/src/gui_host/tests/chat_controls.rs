use super::*;

#[tokio::test]
async fn video_reference_survives_run_resume_and_rejects_old_clients() {
    use pawork_domain::{ModelCapabilities, ModelDefinition, VideoContent};
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = pawork_storage::session::SessionStore::open(dir.path().join("video.db"))
        .await
        .unwrap();
    let provider = MockProvider::new(MockScript::new().text("video received").complete());
    let mut core = AppCore::from_parts(
        Arc::new(provider.clone()),
        None,
        "model-1".into(),
        "mock".into(),
        Some(store.clone()),
    );
    Arc::make_mut(&mut core.registry).merge_provider_models(
        &"mock".into(),
        &[ModelDefinition {
            id: "model-1".into(),
            display_name: "video model".into(),
            context_window_tokens: 0,
            max_output_tokens: 0,
            capabilities: ModelCapabilities {
                text: true,
                video_input: true,
                ..Default::default()
            },
        }],
    );
    let session = core.create_session("Video replay").await.unwrap();
    let video = VideoContent {
        url: "https://example.test/clip.mp4".into(),
        media_type: "video/mp4".into(),
    };
    let adapter = GuiHostAdapter::new(Arc::new(core));
    let mut request = command_envelope(AppCommand::RunStart {
        session_id: session.clone(),
        user_message: String::new(),
        model: None,
        provider: None,
        profile: None,
        effort: None,
        attachment_ids: Vec::new(),
        web_search: None,
        video_urls: vec![video.clone()],
    });
    request.api_version.minor = 23;
    assert!(adapter.command(&request).await.is_err());
    assert!(provider.calls().is_empty());
    request.api_version = API_VERSION;
    request.command_id = "video-current".into();
    request.idempotency_key = None;
    let mut events = adapter.subscribe_events();
    let AppResponse::Accepted {
        run_id: Some(run), ..
    } = adapter.command(&request).await.unwrap()
    else {
        panic!("run not accepted")
    };
    wait_host_run_task_settled(&adapter, &mut events, &run).await;
    assert_eq!(provider.calls().len(), 1);
    let resumed = adapter
        .core
        .read()
        .await
        .resume_messages_keep_pending(&session)
        .await
        .unwrap();
    assert!(resumed
        .iter()
        .flat_map(|m| &m.content)
        .any(|p| p == &ContentPart::Video(video.clone())));
    let timeline = adapter.timeline(&session, None, Some(100)).await.unwrap();
    assert!(serde_json::to_string(&timeline)
        .unwrap()
        .contains(&video.url));
    assert_eq!(
        provider.calls().len(),
        1,
        "replay must not send the reference again"
    );
}

#[tokio::test]
async fn computer_approval_image_persistence_and_resume_do_not_repeat_input() {
    use pawork_domain::{AgentTool, ContentPart, ImageContent, ImageSource, ToolResult};
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = pawork_storage::session::SessionStore::open(dir.path().join("computer.db"))
        .await
        .unwrap();
    let provider = MockProvider::sequence(vec![
        MockScript::new()
            .tool_call("computer", json!({"action":"screenshot"}))
            .complete(),
        MockScript::new().text("observation received").complete(),
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
    assert!(core.tool_names().contains(&"computer"));
    let image = ContentPart::Image(ImageContent {
        source: ImageSource::Base64("/9j/2Q==".into()),
        media_type: "image/jpeg".into(),
        alt_text: None,
    });
    let tool = Arc::new(
        pawork_testkit::MockTool::new("computer", ToolResult::success(vec![image]))
            .with_descriptor(pawork_tools::ComputerTool::default().descriptor()),
    );
    core.scheduler = Arc::new(
        core.scheduler
            .with_tools([tool.clone() as Arc<dyn AgentTool>])
            .unwrap(),
    );
    let session = core.create_session("computer replay").await.unwrap();
    let adapter = GuiHostAdapter::new(Arc::new(core));
    let mut events = adapter.subscribe_events();
    let request = command_envelope(AppCommand::RunStart {
        session_id: session.clone(),
        user_message: "observe desktop".into(),
        model: None,
        provider: None,
        profile: None,
        effort: None,
        attachment_ids: Vec::new(),
        web_search: None,
        video_urls: Vec::new(),
    });
    let AppResponse::Accepted {
        run_id: Some(run), ..
    } = adapter.command(&request).await.unwrap()
    else {
        panic!("run not accepted")
    };
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if let AppEvent::ToolApprovalRequired { tool_call_id, .. } =
                events.recv().await.unwrap().payload
            {
                assert!(tool.calls().is_empty());
                let pending = adapter.approvals.pending();
                assert!(pending.iter().any(|p| p
                    .preview
                    .as_deref()
                    .is_some_and(|s| s.contains("screenshot"))));
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
    wait_run_completed(&mut events, &run).await;
    assert_eq!(tool.calls().len(), 1);
    let persisted = store.replay_events(&session, 1, 100).await.unwrap();
    let json = serde_json::to_string(&persisted).unwrap();
    assert!(json.contains("/9j/2Q==") && json.contains("image/jpeg"));
    adapter
        .core
        .read()
        .await
        .resume_messages_keep_pending(&session)
        .await
        .unwrap();
    assert_eq!(
        tool.calls().len(),
        1,
        "replaying observations must not re-execute computer actions"
    );
}

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
        effort: None,
        attachment_ids: Vec::new(),
        web_search: None,
        video_urls: Vec::new(),
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
