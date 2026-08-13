//! P17-7 全链路测试（请求翻译 → Core 执行 → 事件回译）：
//! 握手协商/降级、session 生命周期、prompt 流式回译、权限请求、
//! cancel / $/cancel_request、resume/close、未知方法拒绝。

mod common;

use std::sync::Arc;
use std::time::Duration;

use acp_host::wire::{ERROR_INVALID_REQUEST, ERROR_RESOURCE_NOT_FOUND, PROTOCOL_VERSION};
use agent_domain::ProviderId;
use core_api::{AppCommand, AppQuery, AppQueryEnvelope, AppResponse};
use serde_json::{json, Value};
use test_support::MockScript;

use common::{collect_outbox, find_outbox, wait_until, TwoTurnToolProvider};

fn temp_dir(tag: &str) -> tempfile::TempDir {
    tempfile::TempDir::with_prefix(tag).expect("temp dir")
}

/// 会话内启动 prompt（spawn 任务，返回 JoinHandle；终态由事件泵驱动）。
fn spawn_prompt(
    harness: &common::TestHarness,
    id: u64,
    session_id: &str,
    text: &str,
) -> tokio::task::JoinHandle<Result<Value, acp_host::JsonRpcError>> {
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

/// 等待 prompt 任务结束并返回结果；期间收集 outbox 消息。
async fn await_prompt(
    harness: &common::TestHarness,
    prompt: tokio::task::JoinHandle<Result<Value, acp_host::JsonRpcError>>,
    collected: &mut Vec<Value>,
) -> Result<Value, acp_host::JsonRpcError> {
    let mut prompt = prompt;
    loop {
        collect_outbox(harness, collected);
        match tokio::time::timeout(Duration::from_millis(25), &mut prompt).await {
            Ok(result) => {
                // 事件泵在发出 prompt 完成信号前已把 delta 通知推入 outbox；
                // 观察到完成后再收一次，避免错过最后一批回译消息。
                collect_outbox(harness, collected);
                return result.expect("prompt task panicked");
            }
            Err(_) => continue,
        }
    }
}

// ---------------------------------------------------------------------
// 握手与能力协商
// ---------------------------------------------------------------------

/// 握手：protocolVersion=1 + agentCapabilities（resume/close 已声明），
/// 未支持能力显式降级记录，不静默丢字段。
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
    // 客户端声明的 fs/terminal 能力不在白名单 → 显式降级记录。
    let degraded: Vec<String> = harness
        .degraded_capabilities()
        .into_iter()
        .map(|capability| capability.0)
        .collect();
    assert_eq!(degraded, vec!["fs.read_text_file", "terminal"]);
    // 重复握手被拒绝（每次连接一次握手）。
    let error = harness
        .host
        .handle_request(json!(99), "initialize", Some(common::initialize_params()))
        .await
        .expect_err("重复握手必须拒绝");
    assert_eq!(error.code, ERROR_INVALID_REQUEST);
}

/// 未握手直接请求被拒绝。
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

// ---------------------------------------------------------------------
// 会话生命周期
// ---------------------------------------------------------------------

/// session/new：SessionCreate（引导例外）→ Attach → registry 记录 + Core session。
#[tokio::test]
async fn session_new_creates_core_session_and_attaches() {
    let harness = common::TestHarness::new(MockScript::new().complete()).await;
    let dir = temp_dir("acp-host-ws-");
    let workspace_id = harness.prepare_workspace(dir.path()).await;
    let session_id = harness
        .new_session(dir.path().to_str().expect("path"))
        .await;
    assert!(!session_id.is_empty());

    // registry：authoritative 记录（Subscribed / epoch 1 / revision 1）。
    let record = harness
        .host
        .registry()
        .get(&client_adapter_api::ClientSessionId::new(&session_id))
        .await
        .expect("registry 记录存在");
    assert_eq!(
        record.state,
        client_adapter_api::ClientSessionState::Subscribed
    );
    assert_eq!(record.ownership_epoch, 1);
    assert_eq!(record.revision, 1);
    // Core session 真实存在。
    assert!(harness
        .service
        .router()
        .aggregate()
        .session_exists(&record.core_session_id));
    let _ = workspace_id;
}

/// cwd 不在任何已登记 workspace root 内 → 显式错误（不静默创建）。
#[tokio::test]
async fn session_new_rejects_cwd_outside_registered_workspaces() {
    let harness = common::TestHarness::new(MockScript::new().complete()).await;
    harness.initialize().await.expect("initialize");
    // 未登记任何 workspace；cwd 用系统临时目录（必然不匹配）。
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
    assert_eq!(error.code, acp_host::wire::ERROR_INTERNAL);
    assert!(
        error
            .message
            .contains("not inside any registered workspace"),
        "{}",
        error.message
    );
}

/// 路径规范化回归：客户端传入未解析 symlink / 重复分隔符的原始 cwd（如
/// macOS 上 TMPDIR 的 `/var/folders/...` vs 登记的 canonical
/// `/private/var/folders/...`）时，仍应匹配到已登记的 workspace root。
#[tokio::test]
async fn session_new_matches_cwd_across_normalization_aliases() {
    let harness = common::TestHarness::new(MockScript::new().complete()).await;
    let dir = temp_dir("acp-host-alias-");
    // 登记 canonical root；客户端侧故意传原始路径（不 canonicalize）。
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

/// 未知 session 的 prompt 显式拒绝（-32002）。
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

// ---------------------------------------------------------------------
// prompt 流式回译
// ---------------------------------------------------------------------

/// prompt：text/thinking delta 回译为 session/update 通知，返回 end_turn。
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
    // delta 合并（限流）：text 增量与 thought 增量各至少一条。
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

/// prompt + 工具调用：ToolStarted/ToolOutput/ToolCompleted 回译为
/// tool_call / tool_call_update；ToolApprovalRequired 转为
/// session/request_permission 请求，allow-once 决策后 run 完成。
#[tokio::test]
async fn prompt_with_tool_emits_permission_request_and_tool_events() {
    let harness = common::TestHarness::new(MockScript::new()).await;
    // MockScript 会逐轮重放，工具后完成需自建两轮 provider。
    let provider: Arc<dyn provider_api::ModelProvider> =
        Arc::new(TwoTurnToolProvider::new(ProviderId::from("mock")));
    harness.service.register_provider(provider);
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

    // 权限请求形状：toolCall + 两个选项。
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

    // tool 事件回译：tool_call 通知 + tool_call_update completed。
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
                && message["params"]["update"]["status"] == json!("completed")
        }),
        "应收到 tool_call_update completed"
    );
}

/// 两 session 并发 prompt：各自从 Accepted 响应绑定自己的 run id（不依赖
/// 全局 last_started_run），两个 run 同时活跃且互不相同，事件回译按
/// run → session 路由不串流——ACP host 层的因果 run_id 回归。
///
/// 用 `wait_for_cancellation` 让两个 run 确定性重叠（都挂起等待取消），
/// 采样不会因 run 瞬时完成而错过注册窗口；随后分别取消，各自收敛。
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
    // 同一连接创建两个 session（握手只做一次，不复用 new_session 助手）。
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

    // 并发 prompt：两个独立任务同时运行。
    let prompt1 = spawn_prompt(&harness, 4, &session1, "first prompt");
    let prompt2 = spawn_prompt(&harness, 5, &session2, "second prompt");
    let sid1 = client_adapter_api::ClientSessionId::new(&session1);
    let sid2 = client_adapter_api::ClientSessionId::new(&session2);
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

    // 两个 run 同时挂起（确定性重叠），随后分别取消。
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

    // 边收 outbox（flush barrier 释放）边等两个 prompt 完成。
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

    // 因果路由：session/update 只归属两个 session 之一，且各自都收到本
    // session 的回译更新（run → session 映射不串流）。
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

    // 因果收敛：每个 session 绑定的 run 各自以 Cancelled 终态收尾。
    let status1 = query_run_status(&harness, &run1).await;
    assert_eq!(status1["state"], json!("cancelled"));
    let status2 = query_run_status(&harness, &run2).await;
    assert_eq!(status2["state"], json!("cancelled"));
}

// ---------------------------------------------------------------------
// 取消
// ---------------------------------------------------------------------

/// session/cancel：RunCancel → run 终态 Cancelled → prompt 返回 cancelled。
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
    // 等待 run 注册（否则 cancel 落在空转路径）。
    assert!(
        wait_until(|| harness.host.has_active_runs(), Duration::from_secs(10)).await,
        "run 应已注册"
    );
    let run_id = harness
        .host
        .pending_run(&client_adapter_api::ClientSessionId::new(&session_id))
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
                    // 幂等重发：若通知落在注册窗口前，补发直到生效。
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

    // RunStatus：聚合终态 Cancelled。
    let status = query_run_status(&harness, &run_id).await;
    assert_eq!(status["state"], json!("cancelled"));
}

/// $/cancel_request：客户端取消自己的 prompt 请求（按 id 取消）。
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
                    // 幂等重发，覆盖注册窗口竞态。
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

// ---------------------------------------------------------------------
// resume / close
// ---------------------------------------------------------------------

/// session/close 先取消未决工作再 Disconnect；session/resume 重新 claim
/// 已断连记录（epoch/revision 递增，state 回 Subscribed）。
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
        .get(&client_adapter_api::ClientSessionId::new(&session_id))
        .await
        .expect("记录保留供 resume");
    assert_eq!(
        record.state,
        client_adapter_api::ClientSessionState::Disconnected
    );

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
        .get(&client_adapter_api::ClientSessionId::new(&session_id))
        .await
        .expect("记录存在");
    assert_eq!(
        record.state,
        client_adapter_api::ClientSessionState::Subscribed
    );
    assert_eq!(record.ownership_epoch, 2);
    assert_eq!(record.revision, 3);
}

// ---------------------------------------------------------------------
// 内部工具
// ---------------------------------------------------------------------

/// 常驻事件泵循环（独立 host 用；与 TestHarness 内部泵同语义）。
async fn pump_loop(host: Arc<acp_host::AcpHost>) {
    loop {
        host.drain_and_pump().await;
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

/// 跨连接 resume：新 AcpHost（新连接）私有 map 为空时，从 authoritative
/// registry 构造 context；Reattach 以新连接 claim（epoch/revision 递增、
/// connection_id 迁移），随后可继续 prompt。
#[tokio::test]
async fn resume_across_new_connection_uses_authoritative_registry() {
    use acp_host::AcpHost;
    use client_adapter_api::{InMemorySessionRegistryStore, SessionRegistry};
    use core_api::{AppCommand, AppResponse};

    let service = Arc::new(app_service::AppService::new("acp-host-cross-conn"));
    let provider: Arc<dyn provider_api::ModelProvider> = Arc::new(
        test_support::MockProvider::new(MockScript::new().complete())
            .with_id(ProviderId::from("mock")),
    );
    service.register_provider(provider);
    let store = Arc::new(InMemorySessionRegistryStore::default());
    let registry = Arc::new(SessionRegistry::new(store).await.expect("registry"));
    let dir = temp_dir("acp-host-resume-");
    let cwd = std::fs::canonicalize(dir.path())
        .unwrap_or_else(|_| dir.path().to_path_buf())
        .to_string_lossy()
        .into_owned();
    let response = service.dispatch_envelope(common::command_envelope(
        "cross-conn-workspace",
        AppCommand::WorkspaceAdd {
            root_path: cwd.clone(),
        },
    ));
    assert!(
        matches!(response.response, AppResponse::Data(_)),
        "WorkspaceAdd 应成功"
    );

    // 连接 1：initialize + session/new + close（记录进入 Disconnected）。
    let host1 = Arc::new(AcpHost::new(Arc::clone(&service), Arc::clone(&registry)));
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

    // 连接 2：私有 map 为空 → resume 必须从 registry 构造 context 并成功。
    let host2 = Arc::new(AcpHost::new(Arc::clone(&service), Arc::clone(&registry)));
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

    // authoritative 记录已 claim 到新连接：epoch 递增、Subscribed、新 connection。
    let record = registry
        .get(&client_adapter_api::ClientSessionId::new(&session_id))
        .await
        .expect("registry 记录存在");
    assert_eq!(
        record.state,
        client_adapter_api::ClientSessionState::Subscribed
    );
    assert_eq!(record.ownership_epoch, 2, "resume 必须递增 ownership epoch");
    assert_eq!(record.revision, 3, "close(2) + resume(3) 后 revision");
    assert_eq!(
        record.connection_id.as_str(),
        host2.connection_id().as_str(),
        "resume 必须把记录 claim 到新连接"
    );

    // 新连接继续 prompt：完整 run 往返。完成信号经 flush barrier 释放，
    // 必须边收 outbox 边等（模拟传输层冲刷）。
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

/// 跨 host/进程恢复：host A 用 SQLite registry 建会话并 close（记录落盘），
/// host B 以全新 AppService（Core aggregate 不含旧 core session）+ 同一
/// `session.db` resume。host B 必须以 registry 已绑定的同一
/// `core_session_id` 做幂等 materialize（不新建随机 session、不重绑映射），
/// 随后只做 ownership CAS claim；映射在「进程重启」前后都不得缺失，也不产生
/// 临时幽灵会话。随后 reattach 使用同一 handle 并继续 prompt。
#[tokio::test]
async fn resume_across_restart_materializes_bound_core_session_idempotently() {
    use acp_host::AcpHost;
    use client_adapter_api::{ClientSessionId, ClientSessionState, SessionRegistry};
    use session_store::{SessionStore, SqliteClientSessionRegistryStore};

    let temp = temp_dir("acp-host-restart-");
    let db_path = temp.path().join("session.db");
    let cwd = std::fs::canonicalize(temp.path())
        .unwrap_or_else(|_| temp.path().to_path_buf())
        .to_string_lossy()
        .into_owned();

    // ---- host A（进程 A）：建会话 → close，记录落盘。 ----
    let (store_a, _) = SessionStore::open(&db_path).await.expect("open a");
    let registry_a = Arc::new(
        SessionRegistry::new(Arc::new(SqliteClientSessionRegistryStore::new(
            store_a.clone(),
        )))
        .await
        .expect("registry a"),
    );
    let service_a = Arc::new(app_service::AppService::new("acp-host-restart-a"));
    let provider: Arc<dyn provider_api::ModelProvider> = Arc::new(
        test_support::MockProvider::new(MockScript::new().complete())
            .with_id(ProviderId::from("mock")),
    );
    service_a.register_provider(provider);
    let response = service_a.dispatch_envelope(common::command_envelope(
        "restart-a-workspace",
        AppCommand::WorkspaceAdd {
            root_path: cwd.clone(),
        },
    ));
    assert!(
        matches!(response.response, AppResponse::Data(_)),
        "WorkspaceAdd 应成功"
    );
    let host_a = Arc::new(AcpHost::new(
        Arc::clone(&service_a),
        Arc::clone(&registry_a),
    ));
    let pump_a = tokio::spawn(pump_loop(Arc::clone(&host_a)));
    host_a
        .handle_request(json!(1), "initialize", Some(common::initialize_params()))
        .await
        .expect("host a initialize");
    let new_result = host_a
        .handle_request(
            json!(2),
            "session/new",
            Some(json!({ "cwd": cwd, "mcpServers": [] })),
        )
        .await
        .expect("host a session/new");
    let session_id = new_result["sessionId"]
        .as_str()
        .expect("sessionId")
        .to_string();
    let id = ClientSessionId::new(&session_id);
    let before = registry_a
        .get(&id)
        .await
        .expect("host a 记录存在")
        .core_session_id;
    host_a
        .handle_request(
            json!(3),
            "session/close",
            Some(json!({ "sessionId": session_id })),
        )
        .await
        .expect("host a session/close");
    let disconnected = registry_a
        .get(&id)
        .await
        .expect("close 后记录仍在（close 不删除映射）");
    assert_eq!(disconnected.state, ClientSessionState::Disconnected);
    assert_eq!(
        (disconnected.ownership_epoch, disconnected.revision),
        (1, 2)
    );
    pump_a.abort();
    let _ = pump_a.await;
    drop(host_a);
    drop(registry_a);
    store_a.shutdown().await.expect("shutdown a");

    // ---- host B（进程 B）：同一 db 重开 → resume 幂等 materialize 同一 core id。 ----
    let (store_b, _) = SessionStore::open(&db_path).await.expect("open b");
    let registry_b = Arc::new(
        SessionRegistry::new(Arc::new(SqliteClientSessionRegistryStore::new(
            store_b.clone(),
        )))
        .await
        .expect("registry b"),
    );
    let reloaded = registry_b
        .get(&id)
        .await
        .expect("重启后映射必须仍在（无 remove+register 崩溃窗口）");
    assert_eq!(reloaded.core_session_id, before);
    assert_eq!((reloaded.ownership_epoch, reloaded.revision), (1, 2));

    let service_b = Arc::new(app_service::AppService::new("acp-host-restart-b"));
    let provider_b: Arc<dyn provider_api::ModelProvider> = Arc::new(
        test_support::MockProvider::new(MockScript::new().complete())
            .with_id(ProviderId::from("mock")),
    );
    service_b.register_provider(provider_b);
    let response = service_b.dispatch_envelope(common::command_envelope(
        "restart-b-workspace",
        AppCommand::WorkspaceAdd {
            root_path: cwd.clone(),
        },
    ));
    assert!(
        matches!(response.response, AppResponse::Data(_)),
        "WorkspaceAdd 应成功"
    );
    // 进程 B 的 Core aggregate 是全新内存态：旧 core session 必须不存在，
    // 否则 resume 不会走 materialize 路径。
    assert!(
        !service_b.router().aggregate().session_exists(&before),
        "旧 core session 不在进程 B 的 aggregate 中（跨进程恢复前提）"
    );
    let host_b = Arc::new(AcpHost::new(
        Arc::clone(&service_b),
        Arc::clone(&registry_b),
    ));
    let pump_b = tokio::spawn(pump_loop(Arc::clone(&host_b)));
    host_b
        .handle_request(json!(4), "initialize", Some(common::initialize_params()))
        .await
        .expect("host b initialize");
    let resume = host_b
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
        .expect("host b session/resume 应成功");
    assert_eq!(resume, json!({}));

    // 幂等 materialize + reattach claim：core_session_id 必须仍是 registry
    // 已绑定的同一 id（不新建幽灵会话）；claim 只推进 ownership
    // （close 后 1/2 → resume 2/3），不额外做 rebind revision。
    let record = registry_b.get(&id).await.expect("resume 后映射必须存在");
    assert_eq!(record.state, ClientSessionState::Subscribed);
    assert_eq!(record.core_session_id, before);
    assert_eq!((record.ownership_epoch, record.revision), (2, 3));
    assert_eq!(
        record.connection_id.as_str(),
        host_b.connection_id().as_str(),
        "resume 必须把记录 claim 到新连接"
    );
    assert!(
        service_b
            .router()
            .aggregate()
            .session_exists(&record.core_session_id),
        "materialize 后同一 core_session_id 必须在 Core aggregate 中存在"
    );
    assert_eq!(
        service_b.router().aggregate().snapshot().sessions.len(),
        1,
        "resume 不得额外创建幽灵 session"
    );

    // 新连接继续 prompt：完整 run 往返，证明重绑后 handle 可执行。
    let prompt = tokio::spawn({
        let host = Arc::clone(&host_b);
        let session_id = session_id.clone();
        async move {
            host.handle_request(
                json!(6),
                "session/prompt",
                Some(json!({
                    "sessionId": session_id,
                    "prompt": [ { "type": "text", "text": "continue after restart" } ],
                })),
            )
            .await
        }
    });
    let mut prompt = prompt;
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    let result = loop {
        let _ = host_b.take_outbox();
        match tokio::time::timeout(Duration::from_millis(25), &mut prompt).await {
            Ok(result) => {
                break result
                    .expect("prompt task panicked")
                    .expect("host b prompt 应成功")
            }
            Err(_) => assert!(
                std::time::Instant::now() < deadline,
                "host b prompt 未在超时前完成"
            ),
        }
    };
    assert_eq!(result["stopReason"], json!("end_turn"));
    pump_b.abort();
    let _ = pump_b.await;
    drop(host_b);
    drop(registry_b);
    store_b.shutdown().await.expect("shutdown b");
}

/// ACP v1 基线 `resource_link` prompt 块映射为安全文本引用拼入用户消息，
/// 不拉取资源、不误要求 image/audio/embeddedContext 能力。
#[tokio::test]
async fn prompt_resource_link_maps_to_safe_text_reference() {
    use agent_domain::{ContentPart, MessageRole};

    struct CaptureProvider {
        id: ProviderId,
        messages: Arc<std::sync::Mutex<Vec<String>>>,
    }

    #[async_trait::async_trait]
    impl provider_api::ModelProvider for CaptureProvider {
        fn id(&self) -> ProviderId {
            self.id.clone()
        }

        async fn list_models(
            &self,
            _credential: Option<&provider_api::ResolvedCredential>,
        ) -> Result<Vec<provider_api::ModelDefinition>, provider_api::ProviderError> {
            Ok(Vec::new())
        }

        async fn stream(
            &self,
            request: provider_api::CanonicalModelRequest,
            sink: &dyn provider_api::ProviderEventSink,
            _cancel: agent_domain::CancellationToken,
        ) -> Result<provider_api::ModelResponseSummary, provider_api::ProviderError> {
            let captured: Vec<String> = request
                .messages
                .iter()
                .filter(|message| message.role == MessageRole::User)
                .flat_map(|message| {
                    message.content.iter().filter_map(|part| match part {
                        ContentPart::Text(text) => Some(text.text.clone()),
                        _ => None,
                    })
                })
                .collect();
            self.messages
                .lock()
                .expect("messages mutex")
                .extend(captured);
            sink.emit(provider_api::ProviderStreamEvent::TextDelta("ok".into()))
                .await?;
            sink.emit(provider_api::ProviderStreamEvent::ResponseCompleted(
                agent_domain::StopReason::Completed,
            ))
            .await?;
            Ok(provider_api::ModelResponseSummary {
                stop_reason: agent_domain::StopReason::Completed,
                usage: agent_domain::TokenUsage::default(),
                response_id: None,
                provider_metadata: Value::Null,
            })
        }
    }

    let harness = common::TestHarness::new(MockScript::new()).await;
    let messages = Arc::new(std::sync::Mutex::new(Vec::new()));
    harness.service.register_provider(Arc::new(CaptureProvider {
        id: ProviderId::from("mock"),
        messages: Arc::clone(&messages),
    }));
    let dir = temp_dir("acp-host-reslink-");
    harness.prepare_workspace(dir.path()).await;
    let session_id = harness
        .new_session(dir.path().to_str().expect("path"))
        .await;

    // prompt 完成信号经 flush barrier 释放：必须边收 outbox 边等（模拟传输层
    // 冲刷），否则屏障不会释放。
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
    let captured = messages.lock().expect("messages mutex").join("\n");
    assert!(
        captured.contains("[docs](file:///docs/readme.md)"),
        "resource_link 应映射为安全文本引用，got: {captured}"
    );
    assert!(
        captured.contains("summarize"),
        "text 块应保留，got: {captured}"
    );
}

// ---------------------------------------------------------------------
// P17-7 对抗审查 P1：占用窗口 / cancel 隔离 / Lagged fail-closed / outbox 半写
// ---------------------------------------------------------------------

/// 同 session 双 prompt：第二个必须在注册窗口被拒绝，只启动一个 turn。
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
    let result = await_prompt(&harness, first, &mut collected).await.expect("first prompt");
    assert_eq!(result["stopReason"], json!("cancelled"));
}

/// idle / 终态后的 session/cancel 与未知 request cancel 不污染下一 prompt。
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

/// 未知 session 的 session/prompt 必须释放预占 occupancy，不能挡住后续合法 prompt。
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

/// 注册窗口内的 early cancel 必须兑现到当前占位 prompt。
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
    // 等 handle_request 预占 occupancy 后再 cancel：覆盖 Reserved 窗口
    //（run_id 尚未绑定）或刚激活窗口。spawn 后立刻 cancel 可能落在预占
    // 之前，会被当成 idle cancel 忽略，造成 prompt 悬挂。
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

/// fail-closed 必须解除全部未决 prompt，不能静默悬挂。
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
    harness
        .host
        .fail_closed_all_prompts("test fail-closed");
    let result = tokio::time::timeout(Duration::from_secs(5), prompt)
        .await
        .expect("fail-closed prompt must not hang")
        .expect("prompt task");
    let error = result.expect_err("fail-closed prompt must fail");
    assert_eq!(error.code, acp_host::wire::ERROR_INTERNAL);
    assert!(!harness.host.has_active_runs());
}

/// replay 可用时补回错过的终态，而不是丢事件。
#[tokio::test]
async fn replay_missed_events_pumps_terminal_state() {
    use core_api::{AppEvent, AppEventEnvelope, EventSource, EventStream, GlobalSequence};
    let harness =
        common::TestHarness::new(MockScript::new().text("started ").wait_for_cancellation()).await;
    let dir = temp_dir("acp-host-replay-");
    harness.prepare_workspace(dir.path()).await;
    let session_id = harness
        .new_session(dir.path().to_str().expect("path"))
        .await;
    let prompt = spawn_prompt(&harness, 26, &session_id, "replay terminal");
    assert!(
        wait_until(|| harness.host.pending_run(&client_adapter_api::ClientSessionId::new(&session_id)).is_some(), Duration::from_secs(10)).await,
        "run must register before replay",
    );
    let run_id = harness
        .host
        .pending_run(&client_adapter_api::ClientSessionId::new(&session_id))
        .expect("run");
    let hub = harness.host.hub();
    hub.publish(AppEventEnvelope {
        api_version: core_api::API_VERSION,
        instance_id: agent_domain::CoreInstanceId::from("acp-replay"),
        event_id: agent_domain::EventId::from("acp-replay-terminal"),
        global_sequence: GlobalSequence(0),
        stream: EventStream::Run(run_id.clone()),
        stream_sequence: 1,
        timestamp: agent_domain::Timestamp::from_unix_millis(1),
        source: EventSource::Core,
        payload: AppEvent::RunChanged {
            run_id,
            state: core_api::RunState::Completed,
        },
    });
    let last_seen = GlobalSequence(hub.current().0.saturating_sub(1));
    harness
        .host
        .replay_missed_events(last_seen)
        .await
        .expect("replay must succeed");
    let mut collected = Vec::new();
    let result = await_prompt(&harness, prompt, &mut collected)
        .await
        .expect("replayed terminal must resolve prompt");
    assert_eq!(result["stopReason"], json!("end_turn"));
}

/// replay 窗口不可用时明确失败，供传输层 fail-closed。
#[tokio::test]
async fn replay_unavailable_is_fail_closed_signal() {
    use core_api::{AppEvent, AppEventEnvelope, EventSource, EventStream, GlobalSequence};
    let service = std::sync::Arc::new(app_service::AppService::new("acp-host-replay-gap"));
    let registry = std::sync::Arc::new(
        client_adapter_api::SessionRegistry::new(std::sync::Arc::new(
            client_adapter_api::InMemorySessionRegistryStore::default(),
        ))
        .await
        .expect("registry"),
    );
    let hub = std::sync::Arc::new(subscription_hub::EventHub::with_capacity(2));
    let host = acp_host::AcpHost::with_hub(service, registry, std::sync::Arc::clone(&hub));
    for index in 0..4 {
        hub.publish(AppEventEnvelope {
            api_version: core_api::API_VERSION,
            instance_id: agent_domain::CoreInstanceId::from("acp-replay-gap"),
            event_id: agent_domain::EventId::from(format!("gap-{index}")),
            global_sequence: GlobalSequence(0),
            stream: EventStream::Global,
            stream_sequence: index as u64 + 1,
            timestamp: agent_domain::Timestamp::from_unix_millis(index as u64 + 1),
            source: EventSource::Core,
            payload: AppEvent::CoreReady {
                handle: core_api::ApiHandle {
                    api_version: core_api::API_VERSION,
                    instance_id: agent_domain::CoreInstanceId::from("acp-replay-gap"),
                },
            },
        });
    }
    let error = host
        .replay_missed_events(GlobalSequence(0))
        .await
        .expect_err("stale last_seen must fail closed");
    assert!(
        error.contains("replay unavailable"),
        "got {error}"
    );
}

/// outbox 半写：已 drain 的剩余屏障必须释放，prompt 不得悬挂。
#[tokio::test]
async fn drained_outbox_barriers_must_be_released_after_partial_write() {
    use acp_host::{OutboxItem, PromptResolution};
    let (tx, mut rx) = tokio::sync::mpsc::channel(1);
    let items = vec![
        OutboxItem::Frame(json!({"method": "session/update"})),
        OutboxItem::FlushBarrier {
            completion: tx,
            resolution: PromptResolution::Failed,
        },
    ];
    // 模拟传输层已 drain：帧丢失，剩余屏障必须由调用方释放。
    acp_host::AcpHost::release_drained_barriers(items);
    let resolution = tokio::time::timeout(Duration::from_secs(1), rx.recv())
        .await
        .expect("released barrier must not hang")
        .expect("barrier resolution");
    assert_eq!(resolution, PromptResolution::Failed);
}

async fn query_run_status(harness: &common::TestHarness, run_id: &agent_domain::RunId) -> Value {
    let envelope = AppQueryEnvelope {
        api_version: core_api::API_VERSION,
        request_id: agent_domain::QueryId::from("acp-test-run-status"),
        source: core_api::CommandSource::Automation,
        identity: core_api::ActorIdentity::Automation {
            name: "acp:test-harness".into(),
        },
        issued_at: agent_domain::Timestamp::from_unix_millis(1),
        query: AppQuery::RunStatus {
            run_id: run_id.clone(),
        },
    };
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        let response = harness.service.dispatch_query(envelope.clone());
        if let AppResponse::Data(value) = response.response {
            if value["state"] == json!("cancelled") || std::time::Instant::now() >= deadline {
                return value;
            }
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}
