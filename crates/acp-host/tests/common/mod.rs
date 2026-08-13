//! P17-7 共享测试装配：AppService + mock provider + [`AcpHost`] + 事件泵。
//!
//! 事件泵是独立 task（`handle_request` 的 `session/prompt` 是阻塞式，等待 run
//! 终态）；测试通过 outbox 收集回译出的 `session/update` 通知与
//! `session/request_permission` 请求。
//!
//! 本模块被 `fixtures` / `floor` 两个测试二进制分别编译，各自只用部分装配；
//! 对单个二进制而言其余项是死代码，故模块级允许 `dead_code`。

#![allow(dead_code)]

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use acp_host::{AcpHost, JsonRpcError, JsonRpcMessage};
use agent_domain::{
    CancellationToken, CommandId, ProviderId, StopReason, Timestamp, TokenUsage, ToolCallId,
    WorkspaceId,
};
use async_trait::async_trait;
use client_adapter_api::{InMemorySessionRegistryStore, SessionRegistry};
use core_api::{
    ActorIdentity, AppCommand, AppCommandEnvelope, AppResponse, CommandSource, API_VERSION,
};
use provider_api::{
    CanonicalModelRequest, ModelDefinition, ModelProvider, ModelResponseSummary, ProviderError,
    ProviderEventSink, ProviderStreamEvent, ResolvedCredential,
};
use serde_json::{json, Value};
use test_support::{MockProvider, MockScript};

/// 构造 ACP JSON-RPC 请求（jsonrpc 2.0 + id + method + params）。
pub fn acp_request(id: u64, method: &str, params: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": method,
        "params": params,
    })
}

/// 构造 ACP JSON-RPC 通知（无 id）。
pub fn acp_notification(method: &str, params: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "method": method,
        "params": params,
    })
}

/// 构造 ACP JSON-RPC 成功响应。
pub fn acp_response(id: u64, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

/// 按 wire 解析（走生产解析路径，获得规范错误语义）。
pub fn parse(value: Value) -> Result<JsonRpcMessage, JsonRpcError> {
    JsonRpcMessage::parse(value)
}

/// 标准握手参数（声明 fs.readTextFile + terminal，host 白名单为空 → 全降级）。
pub fn initialize_params() -> Value {
    json!({
        "protocolVersion": 1,
        "clientCapabilities": {
            "fs": { "readTextFile": true, "writeTextFile": false },
            "terminal": true
        },
        "clientInfo": {
            "name": "test-client",
            "title": "Test Client",
            "version": "1.0.0"
        }
    })
}

/// canonical 命令信封（Automation 来源 + ACP 身份，与 adapter 样式一致）。
pub fn command_envelope(command_id: &str, command: AppCommand) -> AppCommandEnvelope {
    AppCommandEnvelope {
        api_version: API_VERSION,
        command_id: CommandId::from(format!("acp-test-{command_id}")),
        source: CommandSource::Automation,
        identity: ActorIdentity::Automation {
            name: "acp:test-harness".into(),
        },
        expected_revision: None,
        idempotency_key: None,
        issued_at: Timestamp::from_unix_millis(1),
        command,
    }
}

/// 测试装配：AppService + mock provider + AcpHost + 常驻事件泵。
pub struct TestHarness {
    pub service: Arc<app_service::AppService>,
    pub host: Arc<AcpHost>,
    pump: tokio::task::JoinHandle<()>,
}

impl TestHarness {
    pub async fn new(script: MockScript) -> Self {
        let service = Arc::new(app_service::AppService::new("acp-host-test"));
        let provider: Arc<dyn ModelProvider> =
            Arc::new(MockProvider::new(script).with_id(ProviderId::from("mock")));
        service.register_provider(provider);
        let store = Arc::new(InMemorySessionRegistryStore::default());
        let registry = Arc::new(SessionRegistry::new(store).await.expect("registry"));
        let host = Arc::new(AcpHost::new(Arc::clone(&service), registry));
        let pump_host = Arc::clone(&host);
        let pump = tokio::spawn(async move {
            loop {
                pump_host.drain_and_pump().await;
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        });
        Self {
            service,
            host,
            pump,
        }
    }

    /// 握手并返回 result。
    pub async fn initialize(&self) -> Result<Value, JsonRpcError> {
        self.host
            .handle_request(json!(1), "initialize", Some(initialize_params()))
            .await
    }

    /// 预置 workspace（Host 侧引导，不经 adapter 通道）。
    pub async fn prepare_workspace(&self, dir: &Path) -> WorkspaceId {
        // workspace-service 会 canonicalize root（macOS /var → /private/var）；
        // 注册与后续 cwd 匹配必须使用同一规范路径。
        let canonical = canonicalize(dir);
        let response = self.service.dispatch_envelope(command_envelope(
            "workspace-add",
            AppCommand::WorkspaceAdd {
                root_path: canonical.clone(),
            },
        ));
        let AppResponse::Data(value) = response.response else {
            panic!("WorkspaceAdd 应返回 Data，got {:?}", response.response);
        };
        WorkspaceId::from(
            value
                .get("id")
                .and_then(Value::as_str)
                .expect("workspace id"),
        )
    }

    /// 握手 + session/new，返回 ACP sessionId。
    pub async fn new_session(&self, cwd: &str) -> String {
        self.initialize().await.expect("initialize 应成功");
        let cwd = canonicalize(Path::new(cwd));
        let result = self
            .host
            .handle_request(
                json!(2),
                "session/new",
                Some(json!({ "cwd": cwd, "mcpServers": [] })),
            )
            .await
            .expect("session/new 应成功");
        result
            .get("sessionId")
            .and_then(Value::as_str)
            .expect("sessionId")
            .to_string()
    }

    pub fn take_outbox(&self) -> Vec<Value> {
        self.host.take_outbox()
    }

    pub fn is_initialized(&self) -> bool {
        self.host.is_initialized()
    }

    pub fn degraded_capabilities(&self) -> Vec<client_adapter_api::ClientCapability> {
        self.host.degraded_capabilities()
    }
}

/// 规范化路径（解析符号链接；失败时原样返回）。
fn canonicalize(path: &Path) -> String {
    std::fs::canonicalize(path)
        .unwrap_or_else(|_| path.to_path_buf())
        .to_string_lossy()
        .into_owned()
}

impl Drop for TestHarness {
    fn drop(&mut self) {
        self.pump.abort();
    }
}

/// 在超时内等待条件成立（轮询式）。
pub async fn wait_until<F: Fn() -> bool>(condition: F, timeout: Duration) -> bool {
    let deadline = std::time::Instant::now() + timeout;
    while std::time::Instant::now() < deadline {
        if condition() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    condition()
}

/// 取走出站消息并追加到收集器。
pub fn collect_outbox(harness: &TestHarness, collected: &mut Vec<Value>) {
    collected.extend(harness.take_outbox());
}

/// 从收集器里找第一条匹配的通知/请求（按 method 判别）。
pub fn find_outbox<'a>(collected: &'a [Value], method: &str) -> Option<&'a Value> {
    collected
        .iter()
        .find(|message| message.get("method").and_then(Value::as_str) == Some(method))
}

/// 两轮脚本 Provider：第一轮请求工具 `echo`（触发审批），第二轮直接完成。
/// `MockScript` 会逐轮重放同一脚本，无法表达「工具后完成」，故测试自建。
pub struct TwoTurnToolProvider {
    id: ProviderId,
    calls: Arc<std::sync::Mutex<u64>>,
}

impl TwoTurnToolProvider {
    pub fn new(id: ProviderId) -> Self {
        Self {
            id,
            calls: Arc::new(std::sync::Mutex::new(0)),
        }
    }
}

#[async_trait]
impl ModelProvider for TwoTurnToolProvider {
    fn id(&self) -> ProviderId {
        self.id.clone()
    }

    async fn list_models(
        &self,
        _credential: Option<&ResolvedCredential>,
    ) -> Result<Vec<ModelDefinition>, ProviderError> {
        Ok(Vec::new())
    }

    async fn stream(
        &self,
        _request: CanonicalModelRequest,
        sink: &dyn ProviderEventSink,
        _cancel: CancellationToken,
    ) -> Result<ModelResponseSummary, ProviderError> {
        let call = {
            let mut calls = self.calls.lock().expect("calls mutex");
            *calls += 1;
            *calls
        };
        let tool_call_id = ToolCallId::from("mock-tool-call-0");
        if call == 1 {
            sink.emit(ProviderStreamEvent::ToolCallStarted {
                id: tool_call_id.clone(),
                name: "echo".into(),
            })
            .await?;
            sink.emit(ProviderStreamEvent::ToolCallArgumentsDelta {
                id: tool_call_id.clone(),
                json: "{}".into(),
            })
            .await?;
            sink.emit(ProviderStreamEvent::ToolCallCompleted {
                id: tool_call_id.clone(),
            })
            .await?;
            sink.emit(ProviderStreamEvent::ResponseCompleted(StopReason::ToolUse))
                .await?;
        } else {
            sink.emit(ProviderStreamEvent::TextDelta("tool done".into()))
                .await?;
            sink.emit(ProviderStreamEvent::ResponseCompleted(
                StopReason::Completed,
            ))
            .await?;
        }
        Ok(ModelResponseSummary {
            stop_reason: if call == 1 {
                StopReason::ToolUse
            } else {
                StopReason::Completed
            },
            usage: TokenUsage::default(),
            response_id: None,
            provider_metadata: Value::Null,
        })
    }
}
