//! ACP 全链路测试（请求翻译 → mock host 执行 → 事件回译）：
//! 握手协商/降级、session 生命周期、prompt 流式回译、权限请求、
//! cancel / $/cancel_request、resume/close、未知方法拒绝。

mod common;

use std::sync::Arc;
use std::time::Duration;

use pawork_cli::channels::acp::wire::{
    ERROR_INVALID_REQUEST, ERROR_RESOURCE_NOT_FOUND, PROTOCOL_VERSION,
};
use pawork_cli::channels::acp::{AcpCommandHost, AcpHost, JsonRpcError};
use pawork_domain::{QueryId, Timestamp};
use pawork_protocol::adapter::{
    ClientSessionId, ClientSessionState, InMemorySessionRegistryStore, SessionRegistry,
};
use pawork_protocol::{
    ActorIdentity, AppQuery, AppQueryEnvelope, AppResponse, CommandSource, API_VERSION,
};
use serde_json::{json, Value};

use common::{collect_outbox, find_outbox, wait_until, MockScript};

fn temp_dir(tag: &str) -> tempfile::TempDir {
    tempfile::TempDir::with_prefix(tag).expect("temp dir")
}

fn spawn_prompt(
    harness: &common::TestHarness,
    id: u64,
    session_id: &str,
    text: &str,
) -> tokio::task::JoinHandle<Result<Value, JsonRpcError>> {
    let host = Arc::clone(&harness.host);
    let session_id = session_id.to_string();
    let text = text.to_string();
    tokio::spawn(async move {
        host.handle_request(
            json!(id),
            "session/prompt",
            Some(json!({
                "sessionId": session_id,
                "prompt": [ { "type": "text", "text": text } ],
            })),
        )
        .await
    })
}

async fn await_prompt(
    harness: &common::TestHarness,
    prompt: tokio::task::JoinHandle<Result<Value, JsonRpcError>>,
    collected: &mut Vec<Value>,
) -> Result<Value, JsonRpcError> {
    let mut prompt = prompt;
    loop {
        collect_outbox(harness, collected);
        match tokio::time::timeout(Duration::from_millis(25), &mut prompt).await {
            Ok(result) => {
                collect_outbox(harness, collected);
                return result.expect("prompt task panicked");
            }
            Err(_) => continue,
        }
    }
}

#[tokio::test]
async fn handshake_negotiates_v1_and_records_degraded_capabilities() {
    let harness = common::TestHarness::new(MockScript::new().complete()).await;
    let result = harness.initialize().await.expect("initialize");
    assert_eq!(result["protocolVersion"], json!(PROTOCOL_VERSION));
    assert_eq!(
        result["agentCapabilities"]["sessionCapabilities"]["resume"],
        json!({})
    );
    assert_eq!(
        result["agentCapabilities"]["sessionCapabilities"]["close"],
        json!({})
    );
    assert_eq!(result["agentInfo"]["name"], json!("pawork-acp"));
    let degraded: Vec<String> = harness
        .degraded_capabilities()
        .into_iter()
        .map(|capability| capability.0)
        .collect();
    assert_eq!(degraded, vec!["fs.read_text_file", "terminal"]);
    let error = harness
        .host
        .handle_request(json!(99), "initialize", Some(common::initialize_params()))
        .await
        .expect_err("重复握手必须拒绝");
    assert_eq!(error.code, ERROR_INVALID_REQUEST);
}

#[tokio::test]
async fn uninitialized_host_rejects_requests() {
    let harness = common::TestHarness::new(MockScript::new().complete()).await;
    let error = harness
        .host
        .handle_request(json!(1), "session/new", Some(json!({ "cwd": "/tmp" })))
        .await
        .expect_err("未初始化必须拒绝");
    assert_eq!(error.code, ERROR_INVALID_REQUEST);
    let error = harness
        .host
        .handle_notification("session/cancel", Some(json!({ "sessionId": "s1" })))
        .await
        .expect_err("未初始化通知必须拒绝");
    assert_eq!(error.code, ERROR_INVALID_REQUEST);
}

#[tokio::test]
async fn session_new_creates_core_session_and_attaches() {
    let harness = common::TestHarness::new(MockScript::new().complete()).await;
    let dir = temp_dir("acp-host-ws-");
    harness.prepare_workspace(dir.path()).await;
    let session_id = harness
        .new_session(dir.path().to_str().expect("path"))
        .await;
    assert!(!session_id.is_empty());

    let record = harness
        .host
        .registry()
        .get(&ClientSessionId::new(&session_id))
        .await
        .expect("registry 记录存在");
    assert_eq!(record.state, ClientSessionState::Subscribed);
    assert_eq!(record.ownership_epoch, 1);
    assert_eq!(record.revision, 1);
    assert!(harness.mock.session_exists(&record.core_session_id));
}

#[tokio::test]
async fn session_new_rejects_cwd_outside_registered_workspaces() {
    let harness = common::TestHarness::new(MockScript::new().complete()).await;
    harness.initialize().await.expect("initialize");
    let error = harness
        .host
        .handle_request(
            json!(2),
            "session/new",
            Some(json!({
                "cwd": std::env::temp_dir().to_string_lossy(),
                "mcpServers": [],
            })),
        )
        .await
        .expect_err("未登记的 cwd 必须失败");
    assert_eq!(error.code, pawork_cli::channels::acp::wire::ERROR_INTERNAL);
    assert!(
        error
            .message
            .contains("not inside any registered workspace"),
        "{}",
        error.message
    );
}

#[tokio::test]
async fn session_new_matches_cwd_across_normalization_aliases() {
    let harness = common::TestHarness::new(MockScript::new().complete()).await;
    let dir = temp_dir("acp-host-alias-");
    harness.prepare_workspace(dir.path()).await;
    let raw_cwd = dir.path().to_string_lossy().into_owned();
    harness.initialize().await.expect("initialize");
    let result = harness
        .host
        .handle_request(
            json!(2),
            "session/new",
            Some(json!({ "cwd": raw_cwd, "mcpServers": [] })),
        )
        .await
        .expect("原始 cwd 也应解析到已登记 workspace");
    let session_id = result
        .get("sessionId")
        .and_then(Value::as_str)
        .expect("sessionId");
    assert!(!session_id.is_empty());
}

#[tokio::test]
async fn prompt_unknown_session_rejected() {
    let harness = common::TestHarness::new(MockScript::new().complete()).await;
    harness.initialize().await.expect("initialize");
    let error = harness
        .host
        .handle_request(
            json!(3),
            "session/prompt",
            Some(json!({
                "sessionId": "no-such-session",
                "prompt": [ { "type": "text", "text": "hi" } ],
            })),
        )
        .await
        .expect_err("未知 session 必须拒绝");
    assert_eq!(error.code, ERROR_RESOURCE_NOT_FOUND);
}

#[tokio::test]
async fn prompt_streams_session_update_notifications_and_ends_turn() {
    let harness = common::TestHarness::new(
        MockScript::new()
            .text("hello ")
            .text("core")
            .thinking("planning")
            .complete(),
    )
    .await;
    let dir = temp_dir("acp-host-prompt-");
    harness.prepare_workspace(dir.path()).await;
    let session_id = harness
        .new_session(dir.path().to_str().expect("path"))
        .await;

    let prompt = spawn_prompt(&harness, 5, &session_id, "write hello");
    let mut collected = Vec::new();
    let result = tokio::time::timeout(
        Duration::from_secs(15),
        await_prompt(&harness, prompt, &mut collected),
    )
    .await
    .expect("prompt 应在超时前完成")
    .expect("prompt 应成功");
    assert_eq!(result["stopReason"], json!("end_turn"));

    let updates: Vec<&Value> = collected
        .iter()
        .filter(|message| message.get("method").and_then(Value::as_str) == Some("session/update"))
        .collect();
    assert!(!updates.is_empty(), "应收到 session/update 通知");
    for update in &updates {
        assert_eq!(update["params"]["sessionId"], json!(session_id));
    }
    assert!(
        updates.iter().any(|update| {
            update["params"]["update"]["sessionUpdate"] == json!("agent_message_chunk")
        }),
        "应收到 agent_message_chunk"
    );
    assert!(
        updates.iter().any(|update| {
            update["params"]["update"]["sessionUpdate"] == json!("agent_thought_chunk")
        }),
        "应收到 agent_thought_chunk"
    );
    let chunk = updates
        .iter()
        .find(|update| update["params"]["update"]["sessionUpdate"] == json!("agent_message_chunk"))
        .expect("chunk");
    assert_eq!(chunk["params"]["update"]["content"]["type"], json!("text"));
    assert!(
        chunk["params"]["update"]["messageId"].is_string(),
        "messageId 应为字符串"
    );
}

#[tokio::test]
async fn prompt_with_tool_emits_permission_request_and_tool_events() {
    let harness = common::TestHarness::new(MockScript::new().tool_then_complete()).await;
    let dir = temp_dir("acp-host-tool-");
    harness.prepare_workspace(dir.path()).await;
    let session_id = harness
        .new_session(dir.path().to_str().expect("path"))
        .await;

    let prompt = spawn_prompt(&harness, 6, &session_id, "run the tool");
    let mut prompt = prompt;
    let mut collected = Vec::new();
    let mut responded = false;
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    let result = loop {
        collect_outbox(&harness, &mut collected);
        if !responded {
            if let Some(request) = find_outbox(&collected, "session/request_permission") {
                let request_id = request["id"].clone();
                harness
                    .host
                    .handle_response(
                        request_id,
                        Ok(json!({
                            "outcome": { "outcome": "selected", "optionId": "allow-once" }
                        })),
                    )
                    .await
                    .expect("权限响应应被接受");
                responded = true;
            }
        }
        match tokio::time::timeout(Duration::from_millis(25), &mut prompt).await {
            Ok(result) => {
                collect_outbox(&harness, &mut collected);
                break result
                    .expect("prompt task panicked")
                    .expect("prompt 应成功");
            }
            Err(_) => {
                assert!(
                    std::time::Instant::now() < deadline,
                    "prompt 未在超时前完成"
                );
            }
        }
    };
    assert_eq!(result["stopReason"], json!("end_turn"));

    let permission = find_outbox(&collected, "session/request_permission").expect("权限请求");
    assert_eq!(permission["method"], json!("session/request_permission"));
    assert_eq!(permission["params"]["sessionId"], json!(session_id));
    assert_eq!(
        permission["params"]["toolCall"]["toolCallId"],
        json!("mock-tool-call-0")
    );
    let options = permission["params"]["options"].as_array().expect("options");
    assert_eq!(options.len(), 2);
    assert_eq!(options[0]["optionId"], json!("allow-once"));
    assert_eq!(options[1]["optionId"], json!("reject-once"));

    let tool_call = find_outbox(&collected, "session/update")
        .map(|update| update["params"]["update"].clone())
        .filter(|update| update["sessionUpdate"] == json!("tool_call"))
        .expect("tool_call 更新");
    assert_eq!(tool_call["toolCallId"], json!("mock-tool-call-0"));
    assert_eq!(tool_call["title"], json!("echo"));
    assert_eq!(tool_call["status"], json!("pending"));
    assert!(
        collected.iter().any(|message| {
            message.get("method").and_then(Value::as_str) == Some("session/update")
                && message["params"]["update"]["sessionUpdate"] == json!("tool_call_update")
                && message["params"]["update"]["toolCallId"] == json!("mock-tool-call-0")
                && (message["params"]["update"]["status"] == json!("failed")
                    || message["params"]["update"]["status"] == json!("completed"))
        }),
        "应收到 tool_call_update 终态（failed/completed）"
    );
}

#[tokio::test]
async fn invalid_permission_option_denies_tool_instead_of_hanging() {
    let harness = common::TestHarness::new(MockScript::new().tool_then_complete()).await;
    let dir = temp_dir("acp-host-permission-invalid-");
    harness.prepare_workspace(dir.path()).await;
    let session_id = harness
        .new_session(dir.path().to_str().expect("path"))
        .await;

    let prompt = spawn_prompt(&harness, 28, &session_id, "run the tool");
    let mut prompt = prompt;
    let mut collected = Vec::new();
    let mut responded = false;
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    let result = loop {
        collect_outbox(&harness, &mut collected);
        if !responded {
            if let Some(request) = find_outbox(&collected, "session/request_permission") {
                let request_id = request["id"].clone();
                let error = harness
                    .host
                    .handle_response(
                        request_id,
                        Ok(json!({
                            "outcome": { "outcome": "selected", "optionId": "bogus-option" }
                        })),
                    )
                    .await
                    .expect_err("未知 optionId 必须回 JSON-RPC 错误");
                assert_eq!(
                    error.code,
                    pawork_cli::channels::acp::wire::ERROR_INVALID_PARAMS
                );
                responded = true;
            }
        }
        match tokio::time::timeout(Duration::from_millis(25), &mut prompt).await {
            Ok(result) => {
                collect_outbox(&harness, &mut collected);
                break result
                    .expect("prompt task panicked")
                    .expect("Deny 补发后 prompt 应以 cancelled 收口");
            }
            Err(_) => {
                assert!(
                    std::time::Instant::now() < deadline,
                    "权限响应无法采用时必须补发 Deny，prompt 不得悬挂"
                );
            }
        }
    };
    assert_eq!(result["stopReason"], json!("cancelled"));
}

#[tokio::test]
async fn concurrent_prompts_across_two_sessions_carry_distinct_causal_run_ids() {
    let harness =
        common::TestHarness::new(MockScript::new().text("started ").wait_for_cancellation()).await;
    let dir = temp_dir("acp-host-two-session-");
    harness.prepare_workspace(dir.path()).await;
    let cwd = std::fs::canonicalize(dir.path())
        .unwrap_or_else(|_| dir.path().to_path_buf())
        .to_string_lossy()
        .into_owned();
    harness.initialize().await.expect("initialize");
    let new1 = harness
        .host
        .handle_request(
            json!(2),
            "session/new",
            Some(json!({ "cwd": cwd, "mcpServers": [] })),
        )
        .await
        .expect("session1 new 应成功");
    let session1 = new1["sessionId"].as_str().expect("sessionId").to_string();
    let new2 = harness
        .host
        .handle_request(
            json!(3),
            "session/new",
            Some(json!({ "cwd": cwd, "mcpServers": [] })),
        )
        .await
        .expect("session2 new 应成功");
    let session2 = new2["sessionId"].as_str().expect("sessionId").to_string();
    assert_ne!(session1, session2, "两个 session 必须不同");

    let prompt1 = spawn_prompt(&harness, 4, &session1, "first prompt");
    let prompt2 = spawn_prompt(&harness, 5, &session2, "second prompt");
    let sid1 = ClientSessionId::new(&session1);
    let sid2 = ClientSessionId::new(&session2);
    assert!(
        wait_until(
            || {
                harness.host.pending_run(&sid1).is_some()
                    && harness.host.pending_run(&sid2).is_some()
            },
            Duration::from_secs(10)
        )
        .await,
        "两个 run 应同时注册"
    );
    let run1 = harness.host.pending_run(&sid1).expect("run1 已注册");
    let run2 = harness.host.pending_run(&sid2).expect("run2 已注册");
    assert_ne!(
        run1, run2,
        "并发 prompt 必须各自绑定自己的 run id（不共享全局 run）"
    );

    harness
        .host
        .handle_notification("session/cancel", Some(json!({ "sessionId": session1 })))
        .await
        .expect("cancel session1 通知应被接受");
    harness
        .host
        .handle_notification("session/cancel", Some(json!({ "sessionId": session2 })))
        .await
        .expect("cancel session2 通知应被接受");

    let mut collected = Vec::new();
    let mut prompt1 = prompt1;
    let mut prompt2 = prompt2;
    let mut done1 = None;
    let mut done2 = None;
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while done1.is_none() || done2.is_none() {
        collect_outbox(&harness, &mut collected);
        if done1.is_none() {
            if let Ok(result) = tokio::time::timeout(Duration::from_millis(25), &mut prompt1).await
            {
                done1 = Some(
                    result
                        .expect("prompt1 task panicked")
                        .expect("prompt1 应成功"),
                );
            }
        }
        if done2.is_none() {
            if let Ok(result) = tokio::time::timeout(Duration::from_millis(25), &mut prompt2).await
            {
                done2 = Some(
                    result
                        .expect("prompt2 task panicked")
                        .expect("prompt2 应成功"),
                );
            }
        }
        assert!(
            std::time::Instant::now() < deadline,
            "并发 prompt 未在超时前完成"
        );
    }
    collect_outbox(&harness, &mut collected);
    assert_eq!(done1.expect("done1")["stopReason"], json!("cancelled"));
    assert_eq!(done2.expect("done2")["stopReason"], json!("cancelled"));

    let mut updates1 = 0usize;
    let mut updates2 = 0usize;
    for frame in &collected {
        if frame.get("method").and_then(Value::as_str) != Some("session/update") {
            continue;
        }
        let session = frame["params"]["sessionId"]
            .as_str()
            .expect("update 必须携带 sessionId");
        if session == session1 {
            updates1 += 1;
        } else if session == session2 {
            updates2 += 1;
        } else {
            panic!("session/update 串流到未知 session `{session}`: {frame}");
        }
    }
    assert!(updates1 > 0, "session1 应收到回译更新");
    assert!(updates2 > 0, "session2 应收到回译更新");

    let status1 = query_run_status(&harness, &run1).await;
    assert_eq!(status1["state"], json!("cancelled"));
    let status2 = query_run_status(&harness, &run2).await;
    assert_eq!(status2["state"], json!("cancelled"));
}

#[tokio::test]
async fn session_cancel_cancels_active_prompt() {
    let harness =
        common::TestHarness::new(MockScript::new().text("started ").wait_for_cancellation()).await;
    let dir = temp_dir("acp-host-cancel-");
    harness.prepare_workspace(dir.path()).await;
    let session_id = harness
        .new_session(dir.path().to_str().expect("path"))
        .await;

    let prompt = spawn_prompt(&harness, 7, &session_id, "long task");
    assert!(
        wait_until(|| harness.host.has_active_runs(), Duration::from_secs(10)).await,
        "run 应已注册"
    );
    let run_id = harness
        .host
        .pending_run(&ClientSessionId::new(&session_id))
        .expect("run 已注册");
    harness
        .host
        .handle_notification("session/cancel", Some(json!({ "sessionId": session_id })))
        .await
        .expect("cancel 通知应被接受");

    let mut collected = Vec::new();
    let mut prompt = prompt;
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    let result = tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            collect_outbox(&harness, &mut collected);
            match tokio::time::timeout(Duration::from_millis(25), &mut prompt).await {
                Ok(result) => return result.expect("prompt task panicked"),
                Err(_) => {
                    assert!(
                        std::time::Instant::now() < deadline,
                        "prompt 未在取消后结束"
                    );
                    harness
                        .host
                        .handle_notification(
                            "session/cancel",
                            Some(json!({ "sessionId": session_id })),
                        )
                        .await
                        .expect("cancel 通知可重发");
                    tokio::time::sleep(Duration::from_millis(25)).await;
                }
            }
        }
    })
    .await
    .expect("prompt 应在超时前结束")
    .expect("prompt 应成功");
    assert_eq!(result["stopReason"], json!("cancelled"));

    let status = query_run_status(&harness, &run_id).await;
    assert_eq!(status["state"], json!("cancelled"));
}

#[tokio::test]
async fn dollar_cancel_request_cancels_prompt_by_id() {
    let harness =
        common::TestHarness::new(MockScript::new().text("started ").wait_for_cancellation()).await;
    let dir = temp_dir("acp-host-dcancel-");
    harness.prepare_workspace(dir.path()).await;
    let session_id = harness
        .new_session(dir.path().to_str().expect("path"))
        .await;

    let prompt = spawn_prompt(&harness, 8, &session_id, "long task");
    assert!(
        wait_until(|| harness.host.has_active_runs(), Duration::from_secs(10)).await,
        "run 应已注册"
    );
    harness
        .host
        .handle_notification("$/cancel_request", Some(json!({ "requestId": 8 })))
        .await
        .expect("cancel_request 通知应被接受");

    let mut collected = Vec::new();
    let mut prompt = prompt;
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    let result = tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            collect_outbox(&harness, &mut collected);
            match tokio::time::timeout(Duration::from_millis(25), &mut prompt).await {
                Ok(result) => return result.expect("prompt task panicked"),
                Err(_) => {
                    assert!(
                        std::time::Instant::now() < deadline,
                        "prompt 未在取消后结束"
                    );
                    harness
                        .host
                        .handle_notification("$/cancel_request", Some(json!({ "requestId": 8 })))
                        .await
                        .expect("cancel_request 通知可重发");
                    tokio::time::sleep(Duration::from_millis(25)).await;
                }
            }
        }
    })
    .await
    .expect("prompt 应在超时前结束")
    .expect("prompt 应成功");
    assert_eq!(result["stopReason"], json!("cancelled"));
    let _ = collected;
}

#[tokio::test]
async fn session_close_then_resume_reattaches() {
    let harness = common::TestHarness::new(MockScript::new().complete()).await;
    let dir = temp_dir("acp-host-resume-");
    harness.prepare_workspace(dir.path()).await;
    let session_id = harness
        .new_session(dir.path().to_str().expect("path"))
        .await;

    let close = harness
        .host
        .handle_request(
            json!(10),
            "session/close",
            Some(json!({ "sessionId": session_id })),
        )
        .await
        .expect("close 应成功");
    assert_eq!(close, json!({}));
    let record = harness
        .host
        .registry()
        .get(&ClientSessionId::new(&session_id))
        .await
        .expect("记录保留供 resume");
    assert_eq!(record.state, ClientSessionState::Disconnected);

    let resume = harness
        .host
        .handle_request(
            json!(11),
            "session/resume",
            Some(json!({
                "sessionId": session_id,
                "cwd": dir.path().to_str().expect("path"),
                "mcpServers": [],
            })),
        )
        .await
        .expect("resume 应成功");
    assert_eq!(resume, json!({}));
    let record = harness
        .host
        .registry()
        .get(&ClientSessionId::new(&session_id))
        .await
        .expect("记录存在");
    assert_eq!(record.state, ClientSessionState::Subscribed);
    assert_eq!(record.ownership_epoch, 2);
    assert_eq!(record.revision, 3);
}

async fn pump_loop(host: Arc<AcpHost>) {
    loop {
        host.drain_and_pump().await;
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

#[tokio::test]
async fn resume_across_new_connection_uses_authoritative_registry() {
    let mock = Arc::new(common::MockAcpCommandHost::new(
        MockScript::new().complete(),
    ));
    let store = Arc::new(InMemorySessionRegistryStore::default());
    let registry = Arc::new(SessionRegistry::new(store).await.expect("registry"));
    let dir = temp_dir("acp-host-resume-");
    let cwd = std::fs::canonicalize(dir.path())
        .unwrap_or_else(|_| dir.path().to_path_buf())
        .to_string_lossy()
        .into_owned();
    mock.add_workspace(dir.path());

    let host1 = Arc::new(AcpHost::new(
        Arc::clone(&mock) as Arc<dyn AcpCommandHost>,
        Arc::clone(&registry),
    ));
    let pump1 = tokio::spawn(pump_loop(Arc::clone(&host1)));
    let init1 = host1
        .handle_request(json!(1), "initialize", Some(common::initialize_params()))
        .await
        .expect("host1 initialize");
    assert_eq!(init1["protocolVersion"], json!(PROTOCOL_VERSION));
    let new_result = host1
        .handle_request(
            json!(2),
            "session/new",
            Some(json!({ "cwd": cwd, "mcpServers": [] })),
        )
        .await
        .expect("host1 session/new");
    let session_id = new_result["sessionId"]
        .as_str()
        .expect("sessionId")
        .to_string();
    host1
        .handle_request(
            json!(3),
            "session/close",
            Some(json!({ "sessionId": session_id })),
        )
        .await
        .expect("host1 session/close");
    pump1.abort();
    let _ = pump1.await;

    let host2 = Arc::new(AcpHost::new(
        Arc::clone(&mock) as Arc<dyn AcpCommandHost>,
        Arc::clone(&registry),
    ));
    let pump2 = tokio::spawn(pump_loop(Arc::clone(&host2)));
    host2
        .handle_request(json!(4), "initialize", Some(common::initialize_params()))
        .await
        .expect("host2 initialize");
    let resume = host2
        .handle_request(
            json!(5),
            "session/resume",
            Some(json!({
                "sessionId": session_id,
                "cwd": cwd,
                "mcpServers": [],
            })),
        )
        .await
        .expect("host2 session/resume 应成功");
    assert_eq!(resume, json!({}));

    let record = registry
        .get(&ClientSessionId::new(&session_id))
        .await
        .expect("registry 记录存在");
    assert_eq!(record.state, ClientSessionState::Subscribed);
    assert_eq!(record.ownership_epoch, 2, "resume 必须递增 ownership epoch");
    assert_eq!(record.revision, 3, "close(2) + resume(3) 后 revision");
    assert_eq!(
        record.connection_id.as_str(),
        host2.connection_id().as_str(),
        "resume 必须把记录 claim 到新连接"
    );

    let prompt_host = Arc::clone(&host2);
    let prompt_session = session_id.clone();
    let prompt = tokio::spawn(async move {
        prompt_host
            .handle_request(
                json!(6),
                "session/prompt",
                Some(json!({
                    "sessionId": prompt_session,
                    "prompt": [ { "type": "text", "text": "continue" } ],
                })),
            )
            .await
    });
    let mut prompt = prompt;
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    let result = loop {
        let _ = host2.take_outbox();
        match tokio::time::timeout(Duration::from_millis(25), &mut prompt).await {
            Ok(result) => {
                break result
                    .expect("prompt task panicked")
                    .expect("host2 prompt 应成功")
            }
            Err(_) => assert!(
                std::time::Instant::now() < deadline,
                "host2 prompt 未在超时前完成"
            ),
        }
    };
    assert_eq!(result["stopReason"], json!("end_turn"));
    pump2.abort();
    let _ = pump2.await;
}

#[tokio::test]
async fn prompt_resource_link_maps_to_safe_text_reference() {
    let harness = common::TestHarness::new(MockScript::new().complete()).await;
    let dir = temp_dir("acp-host-reslink-");
    harness.prepare_workspace(dir.path()).await;
    let session_id = harness
        .new_session(dir.path().to_str().expect("path"))
        .await;

    let host = Arc::clone(&harness.host);
    let prompt_session = session_id.clone();
    let prompt = tokio::spawn(async move {
        host.handle_request(
            json!(7),
            "session/prompt",
            Some(json!({
                "sessionId": prompt_session,
                "prompt": [
                    { "type": "resource_link", "name": "docs", "uri": "file:///docs/readme.md" },
                    { "type": "text", "text": "summarize" },
                ],
            })),
        )
        .await
    });
    let mut prompt = prompt;
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    let result = loop {
        let _ = harness.take_outbox();
        match tokio::time::timeout(Duration::from_millis(25), &mut prompt).await {
            Ok(result) => {
                break result
                    .expect("prompt task panicked")
                    .expect("prompt 应成功")
            }
            Err(_) => assert!(
                std::time::Instant::now() < deadline,
                "prompt 未在超时前完成"
            ),
        }
    };
    assert_eq!(result["stopReason"], json!("end_turn"));
    let captured = harness.mock.captured_messages().join("\n");
    assert!(
        captured.contains("[docs](file:///docs/readme.md)"),
        "resource_link 应映射为安全文本引用，got: {captured}"
    );
    assert!(
        captured.contains("summarize"),
        "text 块应保留，got: {captured}"
    );
}

#[tokio::test]
async fn same_session_second_prompt_is_rejected_while_first_occupies() {
    let harness =
        common::TestHarness::new(MockScript::new().text("started ").wait_for_cancellation()).await;
    let dir = temp_dir("acp-host-dual-prompt-");
    harness.prepare_workspace(dir.path()).await;
    let session_id = harness
        .new_session(dir.path().to_str().expect("path"))
        .await;

    let first = spawn_prompt(&harness, 21, &session_id, "first turn");
    assert!(
        wait_until(|| harness.host.has_active_runs(), Duration::from_secs(10)).await,
        "first prompt must occupy the session",
    );
    let error = harness
        .host
        .handle_request(
            json!(22),
            "session/prompt",
            Some(json!({
                "sessionId": session_id,
                "prompt": [ { "type": "text", "text": "second turn" } ],
            })),
        )
        .await
        .expect_err("second prompt must be rejected");
    assert_eq!(error.code, ERROR_INVALID_REQUEST);
    assert!(
        error.message.contains("already has an active prompt turn"),
        "got {}",
        error.message
    );

    harness
        .host
        .handle_notification("session/cancel", Some(json!({ "sessionId": session_id })))
        .await
        .expect("cancel first prompt");
    let mut collected = Vec::new();
    let result = await_prompt(&harness, first, &mut collected)
        .await
        .expect("first prompt");
    assert_eq!(result["stopReason"], json!("cancelled"));
}

#[tokio::test]
async fn idle_and_unknown_cancels_do_not_poison_next_prompt() {
    let harness = common::TestHarness::new(MockScript::new().text("ok").complete()).await;
    let dir = temp_dir("acp-host-idle-cancel-");
    harness.prepare_workspace(dir.path()).await;
    let session_id = harness
        .new_session(dir.path().to_str().expect("path"))
        .await;

    harness
        .host
        .handle_notification("session/cancel", Some(json!({ "sessionId": session_id })))
        .await
        .expect("idle session/cancel is accepted and ignored");
    harness
        .host
        .handle_notification("$/cancel_request", Some(json!({ "requestId": 404 })))
        .await
        .expect("unknown request cancel is accepted and ignored");

    let prompt = spawn_prompt(&harness, 23, &session_id, "should complete");
    let mut collected = Vec::new();
    let result = await_prompt(&harness, prompt, &mut collected)
        .await
        .expect("next prompt must not inherit idle cancel");
    assert_eq!(result["stopReason"], json!("end_turn"));
}

#[tokio::test]
async fn unknown_session_prompt_releases_occupancy() {
    let harness = common::TestHarness::new(MockScript::new().complete()).await;
    let dir = temp_dir("acp-host-unknown-session-occupancy-");
    harness.prepare_workspace(dir.path()).await;
    let session_id = harness
        .new_session(dir.path().to_str().expect("path"))
        .await;
    let error = harness
        .host
        .handle_request(
            json!(27),
            "session/prompt",
            Some(json!({
                "sessionId": "missing-session",
                "prompt": [ { "type": "text", "text": "nope" } ],
            })),
        )
        .await
        .expect_err("unknown session must fail");
    assert_eq!(error.code, ERROR_RESOURCE_NOT_FOUND);
    assert!(!harness.host.has_active_runs());

    let prompt = spawn_prompt(&harness, 28, &session_id, "after leak");
    let mut collected = Vec::new();
    let result = await_prompt(&harness, prompt, &mut collected)
        .await
        .expect("later prompt must start after occupancy release");
    assert_eq!(result["stopReason"], json!("end_turn"));
    assert!(!harness.host.has_active_runs());
}

#[tokio::test]
async fn early_cancel_during_registration_window_is_honored() {
    let harness =
        common::TestHarness::new(MockScript::new().text("started ").wait_for_cancellation()).await;
    let dir = temp_dir("acp-host-early-cancel-");
    harness.prepare_workspace(dir.path()).await;
    let session_id = harness
        .new_session(dir.path().to_str().expect("path"))
        .await;

    let prompt = spawn_prompt(&harness, 24, &session_id, "race cancel");
    assert!(
        wait_until(|| harness.host.has_active_runs(), Duration::from_secs(10)).await,
        "prompt must reserve occupancy before early cancel",
    );
    harness
        .host
        .handle_notification("session/cancel", Some(json!({ "sessionId": session_id })))
        .await
        .expect("early session/cancel");
    harness
        .host
        .handle_notification("$/cancel_request", Some(json!({ "requestId": 24 })))
        .await
        .expect("early request cancel");

    let mut collected = Vec::new();
    let result = tokio::time::timeout(
        Duration::from_secs(15),
        await_prompt(&harness, prompt, &mut collected),
    )
    .await
    .expect("early-cancelled prompt must resolve")
    .expect("prompt result");
    assert_eq!(result["stopReason"], json!("cancelled"));
}

#[tokio::test]
async fn fail_closed_releases_inflight_prompt() {
    let harness =
        common::TestHarness::new(MockScript::new().text("started ").wait_for_cancellation()).await;
    let dir = temp_dir("acp-host-fail-closed-");
    harness.prepare_workspace(dir.path()).await;
    let session_id = harness
        .new_session(dir.path().to_str().expect("path"))
        .await;
    let prompt = spawn_prompt(&harness, 25, &session_id, "hang");
    assert!(
        wait_until(|| harness.host.has_active_runs(), Duration::from_secs(10)).await,
        "prompt must be in flight",
    );
    harness.host.fail_closed_all_prompts("test fail-closed");
    let result = tokio::time::timeout(Duration::from_secs(5), prompt)
        .await
        .expect("fail-closed prompt must not hang")
        .expect("prompt task");
    let error = result.expect_err("fail-closed prompt must fail");
    assert_eq!(error.code, pawork_cli::channels::acp::wire::ERROR_INTERNAL);
    assert!(!harness.host.has_active_runs());
}

#[tokio::test]
async fn fail_closed_cancels_core_runs_and_denies_pending_permissions() {
    let harness = common::TestHarness::new(MockScript::new().tool_then_complete()).await;
    let dir = temp_dir("acp-host-fail-closed-core-");
    harness.prepare_workspace(dir.path()).await;
    let session_id = harness
        .new_session(dir.path().to_str().expect("path"))
        .await;
    let prompt = spawn_prompt(&harness, 27, &session_id, "hang on approval");
    let mut collected = Vec::new();
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let run_id = loop {
        collect_outbox(&harness, &mut collected);
        if find_outbox(&collected, "session/request_permission").is_some() {
            break harness
                .host
                .pending_run(&ClientSessionId::new(&session_id))
                .expect("run must bind before permission request");
        }
        assert!(
            std::time::Instant::now() < deadline,
            "未在超时前收到权限请求"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    };
    // 不回权限响应：pending permission 保持悬挂，只能靠 fail-closed 补偿。
    harness
        .host
        .fail_closed_all_prompts("test fail-closed core");
    let result = tokio::time::timeout(Duration::from_secs(5), prompt)
        .await
        .expect("fail-closed prompt must not hang")
        .expect("prompt task");
    assert!(result.is_err(), "fail-closed prompt must fail");
    assert!(
        wait_until(
            || matches!(
                harness.mock.run_state(&run_id),
                Some(pawork_protocol::RunState::Cancelled)
            ),
            Duration::from_secs(5)
        )
        .await,
        "fail-closed 必须向 Core 补发 RunCancel（当前 {:?}）",
        harness.mock.run_state(&run_id)
    );
    assert_eq!(
        harness.mock.run_approval(&run_id),
        Some(pawork_protocol::ApprovalDecision::Deny),
        "fail-closed 必须对挂起权限补发 ToolApprove Deny"
    );
}

#[tokio::test]
async fn pump_events_delivers_terminal_state() {
    use pawork_protocol::{AppEvent, AppEventEnvelope, EventSource, EventStream, GlobalSequence};
    let harness =
        common::TestHarness::new(MockScript::new().text("started ").wait_for_cancellation()).await;
    let dir = temp_dir("acp-host-replay-");
    harness.prepare_workspace(dir.path()).await;
    let session_id = harness
        .new_session(dir.path().to_str().expect("path"))
        .await;
    let prompt = spawn_prompt(&harness, 26, &session_id, "replay terminal");
    assert!(
        wait_until(
            || harness
                .host
                .pending_run(&ClientSessionId::new(&session_id))
                .is_some(),
            Duration::from_secs(10)
        )
        .await,
        "run must register before pump",
    );
    let run_id = harness
        .host
        .pending_run(&ClientSessionId::new(&session_id))
        .expect("run");
    harness
        .host
        .pump_events(vec![AppEventEnvelope {
            api_version: API_VERSION,
            instance_id: pawork_domain::CoreInstanceId::from("acp-replay"),
            event_id: pawork_domain::EventId::from("acp-replay-terminal"),
            global_sequence: GlobalSequence(99),
            stream: EventStream::Run(run_id.clone()),
            stream_sequence: 1,
            timestamp: Timestamp::from_unix_millis(1),
            source: EventSource::Core,
            payload: AppEvent::RunChanged {
                run_id,
                state: pawork_protocol::RunState::Completed,
            },
        }])
        .await;
    let mut collected = Vec::new();
    let result = await_prompt(&harness, prompt, &mut collected)
        .await
        .expect("pumped terminal must resolve prompt");
    assert_eq!(result["stopReason"], json!("end_turn"));
}

#[tokio::test]
async fn pump_events_session_stream_run_changed_resolves_prompt() {
    use pawork_protocol::{AppEvent, AppEventEnvelope, EventSource, EventStream, GlobalSequence};
    let harness =
        common::TestHarness::new(MockScript::new().text("started ").wait_for_cancellation()).await;
    let dir = temp_dir("acp-host-session-stream-");
    harness.prepare_workspace(dir.path()).await;
    let session_id = harness
        .new_session(dir.path().to_str().expect("path"))
        .await;
    let prompt = spawn_prompt(&harness, 27, &session_id, "gui session stream");
    assert!(
        wait_until(
            || harness
                .host
                .pending_run(&ClientSessionId::new(&session_id))
                .is_some(),
            Duration::from_secs(10)
        )
        .await,
        "run must register before pump",
    );
    let run_id = harness
        .host
        .pending_run(&ClientSessionId::new(&session_id))
        .expect("run");
    harness
        .host
        .pump_events(vec![AppEventEnvelope {
            api_version: API_VERSION,
            instance_id: pawork_domain::CoreInstanceId::from("acp-session-stream"),
            event_id: pawork_domain::EventId::from("acp-session-stream-terminal"),
            global_sequence: GlobalSequence(99),
            stream: EventStream::Session(pawork_domain::SessionId::from(session_id.as_str())),
            stream_sequence: 1,
            timestamp: Timestamp::from_unix_millis(1),
            source: EventSource::Core,
            payload: AppEvent::RunChanged {
                run_id,
                state: pawork_protocol::RunState::Completed,
            },
        }])
        .await;
    let mut collected = Vec::new();
    let result = await_prompt(&harness, prompt, &mut collected)
        .await
        .expect("GuiEventBus Session-stream RunChanged must resolve prompt");
    assert_eq!(result["stopReason"], json!("end_turn"));
}

#[tokio::test]
async fn lagged_subscription_is_fail_closed() {
    use pawork_protocol::{AppEvent, EventStream};
    let mock = Arc::new(common::MockAcpCommandHost::with_capacity(
        MockScript::new().complete(),
        2,
    ));
    let registry = Arc::new(
        SessionRegistry::new(Arc::new(InMemorySessionRegistryStore::default()))
            .await
            .expect("registry"),
    );
    let host = AcpHost::new(Arc::clone(&mock) as Arc<dyn AcpCommandHost>, registry);
    for index in 0..8 {
        mock.publish(
            EventStream::Global,
            AppEvent::Diagnostic {
                level: pawork_protocol::DiagnosticLevel::Info,
                code: format!("lag-{index}"),
                message: "overflow".into(),
            },
        );
    }
    host.drain_and_pump().await;
    assert!(!host.has_active_runs());
}

#[tokio::test]
async fn drained_outbox_barriers_must_be_released_after_partial_write() {
    use pawork_cli::channels::acp::{OutboxItem, PromptResolution};
    let (tx, mut rx) = tokio::sync::mpsc::channel(1);
    let items = vec![
        OutboxItem::Frame(json!({"method": "session/update"})),
        OutboxItem::FlushBarrier {
            completion: tx,
            resolution: PromptResolution::Failed,
        },
    ];
    AcpHost::release_drained_barriers(items);
    let resolution = tokio::time::timeout(Duration::from_secs(1), rx.recv())
        .await
        .expect("released barrier must not hang")
        .expect("barrier resolution");
    assert_eq!(resolution, PromptResolution::Failed);
}

async fn query_run_status(harness: &common::TestHarness, run_id: &pawork_domain::RunId) -> Value {
    let envelope = AppQueryEnvelope {
        api_version: API_VERSION,
        request_id: QueryId::from("acp-test-run-status"),
        source: CommandSource::Automation,
        identity: ActorIdentity::Automation {
            name: "acp:test-harness".into(),
        },
        issued_at: Timestamp::from_unix_millis(1),
        query: AppQuery::RunStatus {
            run_id: run_id.clone(),
        },
    };
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        let response = harness
            .mock
            .query(envelope.clone())
            .await
            .expect("run status");
        if let AppResponse::Data(value) = response.response {
            if value["state"] == json!("cancelled") || std::time::Instant::now() >= deadline {
                return value;
            }
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test]
async fn diagnostic_events_are_not_emitted_on_acp_session_update() {
    use pawork_protocol::{
        AppEvent, AppEventEnvelope, DiagnosticLevel, EventSource, EventStream, GlobalSequence,
    };
    let harness =
        common::TestHarness::new(MockScript::new().text("started ").wait_for_cancellation()).await;
    let dir = temp_dir("acp-host-diagnostic-pin-");
    harness.prepare_workspace(dir.path()).await;
    let session_id = harness
        .new_session(dir.path().to_str().expect("path"))
        .await;
    let prompt = spawn_prompt(&harness, 41, &session_id, "hold for diagnostic pin");
    assert!(
        wait_until(
            || harness
                .host
                .pending_run(&ClientSessionId::new(&session_id))
                .is_some(),
            Duration::from_secs(10),
        )
        .await,
        "run must register before diagnostic pump",
    );
    let run_id = harness
        .host
        .pending_run(&ClientSessionId::new(&session_id))
        .expect("run");
    let _ = harness.take_outbox();
    harness
        .host
        .pump_events(vec![AppEventEnvelope {
            api_version: API_VERSION,
            instance_id: pawork_domain::CoreInstanceId::from("acp-diagnostic-pin"),
            event_id: pawork_domain::EventId::from("acp-diagnostic-pin-1"),
            global_sequence: GlobalSequence(1),
            stream: EventStream::Run(run_id.clone()),
            stream_sequence: 1,
            timestamp: Timestamp::from_unix_millis(1),
            source: EventSource::Core,
            payload: AppEvent::Diagnostic {
                level: DiagnosticLevel::Warning,
                code: "degrade.acp_state".into(),
                message: "internal".into(),
            },
        }])
        .await;
    let frames = harness.take_outbox();
    assert!(
        frames
            .iter()
            .all(|frame| frame.get("method").and_then(Value::as_str) != Some("session/update")),
        "Diagnostic must not be encoded as ACP session/update, got {frames:?}"
    );
    harness
        .host
        .handle_notification("session/cancel", Some(json!({ "sessionId": session_id })))
        .await
        .expect("cancel diagnostic pin prompt");
    let mut collected = Vec::new();
    let _ = await_prompt(&harness, prompt, &mut collected).await;
}

#[tokio::test]
async fn interleaved_prompts_from_two_clients_keep_session_serial_and_cancel_unblocked() {
    let harness =
        common::TestHarness::new(MockScript::new().text("started ").wait_for_cancellation()).await;
    let dir = temp_dir("acp-host-two-client-interleave-");
    harness.prepare_workspace(dir.path()).await;
    let cwd = std::fs::canonicalize(dir.path())
        .unwrap_or_else(|_| dir.path().to_path_buf())
        .to_string_lossy()
        .into_owned();
    harness.initialize().await.expect("initialize");
    let new1 = harness
        .host
        .handle_request(
            json!(2),
            "session/new",
            Some(json!({ "cwd": cwd.clone(), "mcpServers": [] })),
        )
        .await
        .expect("session1 new");
    let session1 = new1["sessionId"].as_str().expect("sessionId").to_string();
    let new2 = harness
        .host
        .handle_request(
            json!(3),
            "session/new",
            Some(json!({ "cwd": cwd.clone(), "mcpServers": [] })),
        )
        .await
        .expect("session2 new");
    let session2 = new2["sessionId"].as_str().expect("sessionId").to_string();
    let first = spawn_prompt(&harness, 51, &session1, "client-a first");
    assert!(
        wait_until(|| harness.host.has_active_runs(), Duration::from_secs(10)).await,
        "first client prompt must occupy session1",
    );
    let rejected = harness
        .host
        .handle_request(
            json!(52),
            "session/prompt",
            Some(json!({
                "sessionId": session1,
                "prompt": [ { "type": "text", "text": "client-b overlap" } ],
            })),
        )
        .await
        .expect_err("same-session overlap from a second client must be rejected");
    assert_eq!(rejected.code, ERROR_INVALID_REQUEST);
    assert!(
        rejected
            .message
            .contains("already has an active prompt turn"),
        "got {}",
        rejected.message
    );
    let second = spawn_prompt(&harness, 53, &session2, "client-b other session");
    let sid2 = ClientSessionId::new(&session2);
    assert!(
        wait_until(
            || harness.host.pending_run(&sid2).is_some(),
            Duration::from_secs(10)
        )
        .await,
        "second client prompt on another session must register without waiting for the first",
    );
    harness
        .host
        .handle_notification("session/cancel", Some(json!({ "sessionId": session2 })))
        .await
        .expect("cancel second client prompt must not be blocked by first prompt");
    let mut collected = Vec::new();
    let done2 = await_prompt(&harness, second, &mut collected)
        .await
        .expect("second client prompt");
    assert_eq!(done2["stopReason"], json!("cancelled"));
    harness
        .host
        .handle_notification("session/cancel", Some(json!({ "sessionId": session1 })))
        .await
        .expect("cancel first client prompt");
    let done1 = await_prompt(&harness, first, &mut collected)
        .await
        .expect("first client prompt");
    assert_eq!(done1["stopReason"], json!("cancelled"));
}
