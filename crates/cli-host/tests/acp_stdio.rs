//! P17-7 `acp serve` stdio 传输层集成测试（真实帧循环）。
//!
//! 用 tokio duplex 模拟 stdin/stdout 跑 [`CliHost::acp_loop`]，验证：
//! 握手 → session/new → prompt 全链路帧往返；协议错误回 Parse error 帧且
//! stdout 只含合法 JSON 帧（无 TUI/CLI 文本）；EOF 后干净退出。

use std::sync::Arc;
use std::time::Duration;

use cli_host::CliHost;
use client_adapter_api::{InMemorySessionRegistryStore, SessionRegistry};
use core_api::{ActorIdentity, AppCommand, AppCommandEnvelope, CommandSource, API_VERSION};
use core_runtime::{CoreRuntime, CoreRuntimeConfig};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

fn initialize_params() -> Value {
    json!({
        "protocolVersion": 1,
        "clientCapabilities": {},
        "clientInfo": {
            "name": "acp-stdio-test",
            "title": "ACP Stdio Test",
            "version": "1.0.0"
        }
    })
}

fn request_line(id: u64, method: &str, params: Value) -> String {
    format!(
        "{}\n",
        json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        })
    )
}

/// 读一帧 JSON-RPC（带超时；行必须是合法 JSON，否则直接 panic——stdout 纯度断言）。
async fn read_frame(reader: &mut BufReader<impl tokio::io::AsyncRead + Unpin>) -> Value {
    let mut line = String::new();
    tokio::time::timeout(Duration::from_secs(10), reader.read_line(&mut line))
        .await
        .expect("帧读取超时")
        .expect("读帧失败");
    let line = line.trim();
    assert!(!line.is_empty(), "空行不是合法协议帧");
    serde_json::from_str(line).unwrap_or_else(|error| {
        panic!("stdout 被非 JSON 内容污染：{error:?} -> {line:?}");
    })
}

#[tokio::test]
async fn acp_stdio_full_prompt_round_trip_and_clean_eof() {
    let runtime = CoreRuntime::try_with_config(CoreRuntimeConfig {
        instance: "acp-stdio-test".into(),
        team_db_path: None,
        ..CoreRuntimeConfig::default()
    })
    .expect("core runtime");
    runtime.register_provider(Arc::new(
        test_support::MockProvider::new(test_support::MockScript::new().complete())
            .with_id(agent_domain::ProviderId::from("mock")),
    ));
    let host = CliHost::with_hub(runtime.service().clone(), runtime.hub().clone());
    let registry = Arc::new(
        SessionRegistry::new(Arc::new(InMemorySessionRegistryStore::default()))
            .await
            .expect("registry"),
    );

    let dir = tempfile::TempDir::with_prefix("acp-stdio-").expect("temp dir");
    let cwd = std::fs::canonicalize(dir.path())
        .unwrap_or_else(|_| dir.path().to_path_buf())
        .to_string_lossy()
        .into_owned();
    let response = runtime.service().dispatch_envelope(AppCommandEnvelope {
        api_version: API_VERSION,
        command_id: agent_domain::CommandId::from("acp-stdio-workspace"),
        source: CommandSource::Automation,
        identity: ActorIdentity::Automation {
            name: "acp:stdio-test".into(),
        },
        expected_revision: None,
        idempotency_key: None,
        issued_at: agent_domain::Timestamp::from_unix_millis(1),
        command: AppCommand::WorkspaceAdd {
            root_path: cwd.clone(),
        },
    });
    assert!(
        matches!(response.response, core_api::AppResponse::Data(_)),
        "WorkspaceAdd 应成功"
    );

    let (client_io, server_io) = tokio::io::duplex(1 << 16);
    let (client_r, client_w) = tokio::io::split(client_io);
    let (server_r, server_w) = tokio::io::split(server_io);
    let server = tokio::spawn(async move {
        host.acp_loop(BufReader::new(server_r), server_w, registry)
            .await
    });

    let mut reader = BufReader::new(client_r);
    let mut writer = client_w;

    // 1) 握手。
    writer
        .write_all(request_line(1, "initialize", initialize_params()).as_bytes())
        .await
        .expect("写 initialize");
    let frame = read_frame(&mut reader).await;
    assert_eq!(frame["id"], json!(1));
    assert_eq!(frame["result"]["protocolVersion"], json!(1));

    // 2) session/new（mcpServers 必填字段显式传空数组）。
    writer
        .write_all(
            request_line(2, "session/new", json!({ "cwd": cwd, "mcpServers": [] })).as_bytes(),
        )
        .await
        .expect("写 session/new");
    let frame = read_frame(&mut reader).await;
    assert_eq!(frame["id"], json!(2));
    let session_id = frame["result"]["sessionId"]
        .as_str()
        .expect("sessionId")
        .to_string();

    // 3) session/prompt：跳过中间的 session/update 通知，等 id=3 的响应。
    writer
        .write_all(
            request_line(
                3,
                "session/prompt",
                json!({
                    "sessionId": session_id,
                    "prompt": [ { "type": "text", "text": "hello stdio" } ],
                }),
            )
            .as_bytes(),
        )
        .await
        .expect("写 session/prompt");
    let mut prompt_result = None;
    for _ in 0..200 {
        let frame = read_frame(&mut reader).await;
        if frame.get("id") == Some(&json!(3)) {
            prompt_result = Some(frame);
            break;
        }
    }
    let frame = prompt_result.expect("prompt 响应帧");
    assert_eq!(frame["result"]["stopReason"], json!("end_turn"));

    // 4) 协议错误：非法 JSON → -32700 Parse error 帧，stdout 保持纯 JSON。
    writer
        .write_all(b"this is not json at all\n")
        .await
        .expect("写坏帧");
    let frame = read_frame(&mut reader).await;
    assert_eq!(frame["id"], Value::Null);
    assert_eq!(frame["error"]["code"], json!(-32700));

    // 5) EOF：关闭 stdin（split duplex 需显式 shutdown 才能向对端传播 EOF），
    // 帧循环收敛后干净退出（无残留输出）。
    writer.shutdown().await.expect("shutdown stdin");
    drop(writer);
    let result = tokio::time::timeout(Duration::from_secs(30), server)
        .await
        .expect("acp loop 在超时前退出")
        .expect("acp loop 无 IO 错误");
    assert!(result.is_ok(), "acp loop 应无 IO 错误: {result:?}");
    runtime.shutdown();
}

/// 长 Prompt 进行中，读循环仍能处理 `session/cancel`：cancel 入站后
/// prompt 必须收敛为 cancelled，而不是卡在读循环里永远等不到通知。
#[tokio::test]
async fn acp_stdio_cancel_is_processed_during_long_prompt() {
    let runtime = CoreRuntime::try_with_config(CoreRuntimeConfig {
        instance: "acp-stdio-cancel".into(),
        team_db_path: None,
        ..CoreRuntimeConfig::default()
    })
    .expect("core runtime");
    runtime.register_provider(Arc::new(
        test_support::MockProvider::new(
            test_support::MockScript::new()
                .text("started ")
                .wait_for_cancellation(),
        )
        .with_id(agent_domain::ProviderId::from("mock")),
    ));
    let host = CliHost::with_hub(runtime.service().clone(), runtime.hub().clone());
    let registry = Arc::new(
        SessionRegistry::new(Arc::new(InMemorySessionRegistryStore::default()))
            .await
            .expect("registry"),
    );

    let dir = tempfile::TempDir::with_prefix("acp-stdio-cancel-").expect("temp dir");
    let cwd = std::fs::canonicalize(dir.path())
        .unwrap_or_else(|_| dir.path().to_path_buf())
        .to_string_lossy()
        .into_owned();
    let response = runtime.service().dispatch_envelope(AppCommandEnvelope {
        api_version: API_VERSION,
        command_id: agent_domain::CommandId::from("acp-stdio-cancel-workspace"),
        source: CommandSource::Automation,
        identity: ActorIdentity::Automation {
            name: "acp:stdio-cancel".into(),
        },
        expected_revision: None,
        idempotency_key: None,
        issued_at: agent_domain::Timestamp::from_unix_millis(1),
        command: AppCommand::WorkspaceAdd {
            root_path: cwd.clone(),
        },
    });
    assert!(
        matches!(response.response, core_api::AppResponse::Data(_)),
        "WorkspaceAdd 应成功"
    );

    let (client_io, server_io) = tokio::io::duplex(1 << 16);
    let (client_r, client_w) = tokio::io::split(client_io);
    let (server_r, server_w) = tokio::io::split(server_io);
    let server = tokio::spawn(async move {
        host.acp_loop(BufReader::new(server_r), server_w, registry)
            .await
    });

    let mut reader = BufReader::new(client_r);
    let mut writer = client_w;

    writer
        .write_all(request_line(1, "initialize", initialize_params()).as_bytes())
        .await
        .expect("写 initialize");
    let frame = read_frame(&mut reader).await;
    assert_eq!(frame["id"], json!(1));

    writer
        .write_all(
            request_line(2, "session/new", json!({ "cwd": cwd, "mcpServers": [] })).as_bytes(),
        )
        .await
        .expect("写 session/new");
    let frame = read_frame(&mut reader).await;
    assert_eq!(frame["id"], json!(2));
    let session_id = frame["result"]["sessionId"]
        .as_str()
        .expect("sessionId")
        .to_string();

    writer
        .write_all(
            request_line(
                3,
                "session/prompt",
                json!({
                    "sessionId": session_id,
                    "prompt": [ { "type": "text", "text": "hang until cancel" } ],
                }),
            )
            .as_bytes(),
        )
        .await
        .expect("写 session/prompt");

    // 先等到本 prompt 的第一条 session/update：证明读循环已把 prompt
    // 放到独立 task，run 已注册。此时再发 cancel，才能区分「读循环堵死」
    // 与「cancel 落在注册窗口前被丢掉」。
    let mut saw_update = false;
    for _ in 0..200 {
        let frame = read_frame(&mut reader).await;
        if frame.get("method").and_then(Value::as_str) == Some("session/update") {
            saw_update = true;
            break;
        }
        if frame.get("id") == Some(&json!(3)) {
            panic!("prompt 在 cancel 前已结束: {frame}");
        }
    }
    assert!(saw_update, "长 Prompt 应先产出 session/update");

    writer
        .write_all(
            format!(
                "{}\n",
                json!({
                    "jsonrpc": "2.0",
                    "method": "session/cancel",
                    "params": { "sessionId": session_id },
                })
            )
            .as_bytes(),
        )
        .await
        .expect("写 session/cancel");

    let mut prompt_result = None;
    for _ in 0..400 {
        let frame = read_frame(&mut reader).await;
        if frame.get("id") == Some(&json!(3)) {
            prompt_result = Some(frame);
            break;
        }
    }
    let frame = prompt_result.expect("cancel 后应收到 prompt 响应");
    assert_eq!(frame["result"]["stopReason"], json!("cancelled"));

    writer.shutdown().await.expect("shutdown stdin");
    drop(writer);
    let result = tokio::time::timeout(Duration::from_secs(30), server)
        .await
        .expect("acp loop 在超时前退出")
        .expect("acp loop 无 IO 错误");
    assert!(result.is_ok(), "acp loop 应无 IO 错误: {result:?}");
    runtime.shutdown();
}

/// 长 Prompt 进行中，读循环仍能处理 `session/request_permission` 响应：
/// 权限请求写出后客户端立刻回 allow-once，prompt 必须收敛为 end_turn。
#[tokio::test]
async fn acp_stdio_permission_response_is_processed_during_long_prompt() {
    let runtime = CoreRuntime::try_with_config(CoreRuntimeConfig {
        instance: "acp-stdio-permission".into(),
        team_db_path: None,
        ..CoreRuntimeConfig::default()
    })
    .expect("core runtime");
    let provider: std::sync::Arc<dyn provider_api::ModelProvider> = std::sync::Arc::new(
        TwoTurnToolProvider::new(agent_domain::ProviderId::from("mock")),
    );
    runtime.register_provider(provider);
    let host = CliHost::with_hub(runtime.service().clone(), runtime.hub().clone());
    let registry = Arc::new(
        SessionRegistry::new(Arc::new(InMemorySessionRegistryStore::default()))
            .await
            .expect("registry"),
    );

    let dir = tempfile::TempDir::with_prefix("acp-stdio-permission-").expect("temp dir");
    let cwd = std::fs::canonicalize(dir.path())
        .unwrap_or_else(|_| dir.path().to_path_buf())
        .to_string_lossy()
        .into_owned();
    let response = runtime.service().dispatch_envelope(AppCommandEnvelope {
        api_version: API_VERSION,
        command_id: agent_domain::CommandId::from("acp-stdio-permission-workspace"),
        source: CommandSource::Automation,
        identity: ActorIdentity::Automation {
            name: "acp:stdio-permission".into(),
        },
        expected_revision: None,
        idempotency_key: None,
        issued_at: agent_domain::Timestamp::from_unix_millis(1),
        command: AppCommand::WorkspaceAdd {
            root_path: cwd.clone(),
        },
    });
    assert!(
        matches!(response.response, core_api::AppResponse::Data(_)),
        "WorkspaceAdd 应成功"
    );

    let (client_io, server_io) = tokio::io::duplex(1 << 16);
    let (client_r, client_w) = tokio::io::split(client_io);
    let (server_r, server_w) = tokio::io::split(server_io);
    let server = tokio::spawn(async move {
        host.acp_loop(BufReader::new(server_r), server_w, registry)
            .await
    });

    let mut reader = BufReader::new(client_r);
    let mut writer = client_w;

    writer
        .write_all(request_line(1, "initialize", initialize_params()).as_bytes())
        .await
        .expect("写 initialize");
    let frame = read_frame(&mut reader).await;
    assert_eq!(frame["id"], json!(1));

    writer
        .write_all(
            request_line(2, "session/new", json!({ "cwd": cwd, "mcpServers": [] })).as_bytes(),
        )
        .await
        .expect("写 session/new");
    let frame = read_frame(&mut reader).await;
    assert_eq!(frame["id"], json!(2));
    let session_id = frame["result"]["sessionId"]
        .as_str()
        .expect("sessionId")
        .to_string();

    writer
        .write_all(
            request_line(
                3,
                "session/prompt",
                json!({
                    "sessionId": session_id,
                    "prompt": [ { "type": "text", "text": "run the tool" } ],
                }),
            )
            .as_bytes(),
        )
        .await
        .expect("写 session/prompt");

    let mut prompt_result = None;
    let mut permission_answered = false;
    for _ in 0..400 {
        let frame = read_frame(&mut reader).await;
        if frame.get("method").and_then(Value::as_str) == Some("session/request_permission")
            && !permission_answered
        {
            let request_id = frame["id"].clone();
            writer
                .write_all(
                    format!(
                        "{}\n",
                        json!({
                            "jsonrpc": "2.0",
                            "id": request_id,
                            "result": {
                                "outcome": { "outcome": "selected", "optionId": "allow-once" }
                            }
                        })
                    )
                    .as_bytes(),
                )
                .await
                .expect("写 permission 响应");
            permission_answered = true;
            continue;
        }
        if frame.get("id") == Some(&json!(3)) {
            prompt_result = Some(frame);
            break;
        }
    }
    assert!(
        permission_answered,
        "长 Prompt 期间应发出 session/request_permission"
    );
    let frame = prompt_result.expect("permission 响应后应收到 prompt 响应");
    assert_eq!(frame["result"]["stopReason"], json!("end_turn"));

    writer.shutdown().await.expect("shutdown stdin");
    drop(writer);
    let result = tokio::time::timeout(Duration::from_secs(30), server)
        .await
        .expect("acp loop 在超时前退出")
        .expect("acp loop 无 IO 错误");
    assert!(result.is_ok(), "acp loop 应无 IO 错误: {result:?}");
    runtime.shutdown();
}

struct TwoTurnToolProvider {
    id: agent_domain::ProviderId,
    calls: std::sync::Mutex<u64>,
}

impl TwoTurnToolProvider {
    fn new(id: agent_domain::ProviderId) -> Self {
        Self {
            id,
            calls: std::sync::Mutex::new(0),
        }
    }
}

#[async_trait::async_trait]
impl provider_api::ModelProvider for TwoTurnToolProvider {
    fn id(&self) -> agent_domain::ProviderId {
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
        _request: provider_api::CanonicalModelRequest,
        sink: &dyn provider_api::ProviderEventSink,
        _cancel: agent_domain::CancellationToken,
    ) -> Result<provider_api::ModelResponseSummary, provider_api::ProviderError> {
        let call = {
            let mut calls = self.calls.lock().expect("calls mutex");
            *calls += 1;
            *calls
        };
        let tool_call_id = agent_domain::ToolCallId::from("mock-tool-call-0");
        if call == 1 {
            sink.emit(provider_api::ProviderStreamEvent::ToolCallStarted {
                id: tool_call_id.clone(),
                name: "echo".into(),
            })
            .await?;
            sink.emit(provider_api::ProviderStreamEvent::ToolCallArgumentsDelta {
                id: tool_call_id.clone(),
                json: "{}".into(),
            })
            .await?;
            sink.emit(provider_api::ProviderStreamEvent::ToolCallCompleted {
                id: tool_call_id.clone(),
            })
            .await?;
            sink.emit(provider_api::ProviderStreamEvent::ResponseCompleted(
                agent_domain::StopReason::ToolUse,
            ))
            .await?;
        } else {
            sink.emit(provider_api::ProviderStreamEvent::TextDelta(
                "tool done".into(),
            ))
            .await?;
            sink.emit(provider_api::ProviderStreamEvent::ResponseCompleted(
                agent_domain::StopReason::Completed,
            ))
            .await?;
        }
        Ok(provider_api::ModelResponseSummary {
            stop_reason: if call == 1 {
                agent_domain::StopReason::ToolUse
            } else {
                agent_domain::StopReason::Completed
            },
            usage: agent_domain::TokenUsage::default(),
            response_id: None,
            provider_metadata: Value::Null,
        })
    }
}
