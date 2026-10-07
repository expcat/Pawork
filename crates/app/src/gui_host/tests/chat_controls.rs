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
    use pawork_domain::{
        AgentTool, ContentPart, ImageContent, ImageSource, ModelCapabilities, ModelDefinition,
        ToolResult,
    };
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
    // CU-11：computer 截图/观测是视觉操作，模型须声明 image_input 才暴露。
    Arc::make_mut(&mut core.registry).merge_provider_models(
        &"mock".into(),
        &[ModelDefinition {
            id: "model-1".into(),
            display_name: "vision model".into(),
            context_window_tokens: 0,
            max_output_tokens: 0,
            capabilities: ModelCapabilities {
                text: true,
                image_input: true,
                ..ModelCapabilities::default()
            },
        }],
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
    // CU-14 复审③：首个动作前先应答一次方言探测（tabs），再领取真实动作。
    let probe = tokio::time::timeout(std::time::Duration::from_secs(5), async {
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
    assert_eq!(probe["action"]["action"], "tabs");
    adapter
        .browser
        .respond(
            probe["request_id"].as_str().unwrap(),
            &source,
            json!({"ok":true,"data":{"tabs":[],"active_tab_id":null,"agent_tab_id":null}}),
        )
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
    assert_eq!(claimed["action"]["action"], "read");
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

#[tokio::test]
async fn run_start_browser_screenshot_gate_falls_back_to_static_default() {
    use pawork_domain::{ModelCapabilities, ModelDefinition};
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = pawork_storage::session::SessionStore::open(dir.path().join("fallback.db"))
        .await
        .unwrap();
    let provider = MockProvider::sequence(vec![
        MockScript::new()
            .tool_call("browser", json!({"action":"screenshot"}))
            .complete(),
        MockScript::new().text("image received").complete(),
    ]);
    let mut core = AppCore::from_parts(
        Arc::new(provider),
        None,
        "glm-5.3-flash".into(),
        "mock".into(),
        Some(store.clone()),
    );
    // CU-13 真实验收发现的门缺陷：目录证据可把模型锚定在另一 provider
    // （glm-5.3-flash ↔ glm-coding），经 opencode-go 使用时被过滤后截图门
    // 直接判 false。证据口径与 text 门一致：provider 不匹配回退静态默认。
    // 这里把错锚证据显式声明为非视觉，证明回退后取的是静态默认。
    Arc::make_mut(&mut core.registry).merge_provider_models(
        &"glm-coding".into(),
        &[ModelDefinition {
            id: "glm-5.3-flash".into(),
            display_name: "stale anchor".into(),
            context_window_tokens: 0,
            max_output_tokens: 0,
            capabilities: ModelCapabilities {
                text: true,
                image_input: false,
                ..Default::default()
            },
        }],
    );
    core.configure_approval(
        pawork_policy::ApprovalMode::AskForDangerous,
        true,
        Arc::new(crate::DenyAllApprovals),
    );
    core.attach_workspace(dir.path()).unwrap();
    let session = core.create_session("browser fallback").await.unwrap();
    let adapter = GuiHostAdapter::new(Arc::new(core));
    let source = CommandSource::LocalGui {
        client_id: "fallback-client".into(),
    };
    let mut request = command_envelope(AppCommand::RunStart {
        session_id: session.clone(),
        user_message: "screenshot browser".into(),
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
            if let AppEvent::ToolApprovalRequired { tool_call_id, .. } =
                events.recv().await.unwrap().payload
            {
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
    // CU-14 复审③：首个动作前先应答一次方言探测（tabs），再领取真实动作。
    let probe = tokio::time::timeout(std::time::Duration::from_secs(5), async {
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
    assert_eq!(probe["action"]["action"], "tabs");
    adapter
        .browser
        .respond(
            probe["request_id"].as_str().unwrap(),
            &source,
            json!({"ok":true,"data":{"tabs":[],"active_tab_id":null,"agent_tab_id":null}}),
        )
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
    .expect("gate must fall back to static default and dispatch screenshot");
    assert_eq!(claimed["action"]["action"], "screenshot");
    adapter
        .browser
        .respond(
            claimed["request_id"].as_str().unwrap(),
            &source,
            json!({"ok":true,"data":{"url":"https://example.com/","width":1024,"height":768,"jpeg_base64":"/9j/2Q=="}}),
        )
        .unwrap();
    wait_run_completed(&mut events, &run).await;
    let persisted = store.replay_events(&session, 1, 100).await.unwrap();
    let serialized = serde_json::to_string(&persisted).unwrap();
    assert!(serialized.contains("/9j/2Q=="));
    assert!(!serialized.contains("vision-capable"));
}
