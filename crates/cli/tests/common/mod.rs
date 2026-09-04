//! ACP 通道测试装配：内存 [`AcpCommandHost`] + [`AcpHost`] + 事件泵。
//!
//! 本模块被 `fixtures` / `floor` 两个测试二进制分别编译，各自只用部分装配；
//! 对单个二进制而言其余项是死代码，故模块级允许 `dead_code`。

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use pawork_cli::channels::acp::{
    AcpCommandHost, AcpHost, AcpHostError, JsonRpcError, JsonRpcMessage,
};
use pawork_domain::{
    CommandId, CoreInstanceId, EventId, QueryId, RunId, SessionId, Timestamp, ToolCallId,
    WorkspaceId,
};
use pawork_protocol::adapter::{InMemorySessionRegistryStore, SessionRegistry};
use pawork_protocol::{
    AppCommand, AppCommandEnvelope, AppEvent, AppEventEnvelope, AppQuery, AppQueryEnvelope,
    AppResponse, AppResponseEnvelope, ApprovalDecision, EventSource, EventStream, GlobalSequence,
    RunState, API_VERSION,
};
use serde_json::{json, Value};
use tokio::sync::{broadcast, Notify};

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

/// 规范化路径（解析符号链接；失败时原样返回）。
pub fn canonicalize(path: &Path) -> String {
    std::fs::canonicalize(path)
        .unwrap_or_else(|_| path.to_path_buf())
        .to_string_lossy()
        .into_owned()
}

/// floor / fixtures 用的确定性脚本（不依赖真实 Provider）。
#[derive(Clone, Debug, Default)]
pub struct MockScript {
    texts: Vec<String>,
    thinking: Vec<String>,
    wait_for_cancel: bool,
    tool: bool,
}

impl MockScript {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn text(mut self, text: impl Into<String>) -> Self {
        self.texts.push(text.into());
        self
    }

    pub fn thinking(mut self, text: impl Into<String>) -> Self {
        self.thinking.push(text.into());
        self
    }

    pub fn complete(self) -> Self {
        self
    }

    pub fn wait_for_cancellation(mut self) -> Self {
        self.wait_for_cancel = true;
        self
    }

    pub fn tool_then_complete(mut self) -> Self {
        self.tool = true;
        self
    }
}

struct WorkspaceEntry {
    id: WorkspaceId,
    root: String,
}

struct SessionEntry {
    id: SessionId,
}

struct RunEntry {
    id: RunId,
    state: Mutex<RunState>,
    cancel: Arc<Notify>,
    approval: Mutex<Option<ApprovalDecision>>,
    approval_notify: Arc<Notify>,
}

struct MockInner {
    workspaces: Mutex<Vec<WorkspaceEntry>>,
    sessions: Mutex<BTreeMap<SessionId, SessionEntry>>,
    runs: Mutex<BTreeMap<RunId, Arc<RunEntry>>>,
    events: broadcast::Sender<AppEventEnvelope>,
    next_id: AtomicU64,
    next_seq: AtomicU64,
    script: MockScript,
    captured_messages: Mutex<Vec<String>>,
    instance_id: CoreInstanceId,
}

/// 内存 Core 替身：登记 workspace / session / run，并按脚本发布事件。
pub struct MockAcpCommandHost {
    inner: Arc<MockInner>,
}

impl MockAcpCommandHost {
    pub fn new(script: MockScript) -> Self {
        Self::with_capacity(script, 256)
    }

    pub fn with_capacity(script: MockScript, event_capacity: usize) -> Self {
        let (events, _) = broadcast::channel(event_capacity);
        Self {
            inner: Arc::new(MockInner {
                workspaces: Mutex::new(Vec::new()),
                sessions: Mutex::new(BTreeMap::new()),
                runs: Mutex::new(BTreeMap::new()),
                events,
                next_id: AtomicU64::new(1),
                next_seq: AtomicU64::new(1),
                script,
                captured_messages: Mutex::new(Vec::new()),
                instance_id: CoreInstanceId::from("acp-mock"),
            }),
        }
    }

    pub fn add_workspace(&self, dir: &Path) -> WorkspaceId {
        let root = canonicalize(dir);
        let id = WorkspaceId::from(format!(
            "ws-{}",
            self.inner.next_id.fetch_add(1, Ordering::SeqCst)
        ));
        self.inner
            .workspaces
            .lock()
            .expect("workspaces")
            .push(WorkspaceEntry {
                id: id.clone(),
                root,
            });
        id
    }

    pub fn session_exists(&self, session_id: &SessionId) -> bool {
        self.inner
            .sessions
            .lock()
            .expect("sessions")
            .contains_key(session_id)
    }

    pub fn captured_messages(&self) -> Vec<String> {
        self.inner
            .captured_messages
            .lock()
            .expect("messages")
            .clone()
    }

    pub fn run_state(&self, run_id: &RunId) -> Option<RunState> {
        self.inner
            .runs
            .lock()
            .expect("runs")
            .get(run_id)
            .map(|run| run.state.lock().expect("run state").clone())
    }

    /// 观测 fail-closed / 权限拒绝是否真的把决策送进了 Core 替身。
    pub fn run_approval(&self, run_id: &RunId) -> Option<ApprovalDecision> {
        self.inner
            .runs
            .lock()
            .expect("runs")
            .get(run_id)
            .and_then(|run| run.approval.lock().expect("approval").clone())
    }

    fn next_name(&self, prefix: &str) -> String {
        format!(
            "{prefix}-{}",
            self.inner.next_id.fetch_add(1, Ordering::SeqCst)
        )
    }

    pub fn publish(&self, stream: EventStream, payload: AppEvent) {
        let seq = self.inner.next_seq.fetch_add(1, Ordering::SeqCst);
        let _ = self.inner.events.send(AppEventEnvelope {
            api_version: API_VERSION,
            instance_id: self.inner.instance_id.clone(),
            event_id: EventId::from(format!("evt-{seq}")),
            global_sequence: GlobalSequence(seq),
            stream,
            stream_sequence: seq,
            timestamp: Timestamp::from_unix_millis(seq),
            source: EventSource::Core,
            payload,
        });
    }

    fn response(request_id: QueryId, response: AppResponse) -> AppResponseEnvelope {
        AppResponseEnvelope {
            api_version: API_VERSION,
            request_id,
            responded_at: Timestamp::from_unix_millis(1),
            response,
        }
    }

    fn play_script(self: Arc<Self>, run: Arc<RunEntry>) {
        let script = self.inner.script.clone();
        tokio::spawn(async move {
            tokio::task::yield_now().await;
            if script.tool {
                self.publish(
                    EventStream::Run(run.id.clone()),
                    AppEvent::ToolStarted {
                        run_id: run.id.clone(),
                        tool_call_id: ToolCallId::from("mock-tool-call-0"),
                        name: "echo".into(),
                    },
                );
                self.publish(
                    EventStream::Run(run.id.clone()),
                    AppEvent::ToolApprovalRequired {
                        run_id: run.id.clone(),
                        tool_call_id: ToolCallId::from("mock-tool-call-0"),
                        reason: "echo".into(),
                    },
                );
                tokio::select! {
                    _ = run.cancel.notified() => {
                        run_set_state(&run, RunState::Cancelled);
                        self.publish(
                            EventStream::Run(run.id.clone()),
                            AppEvent::RunChanged {
                                run_id: run.id.clone(),
                                state: RunState::Cancelled,
                            },
                        );
                        return;
                    }
                    _ = run.approval_notify.notified() => {}
                }
                let decision = run.approval.lock().expect("approval").clone();
                match decision {
                    Some(ApprovalDecision::ApproveOnce) | Some(ApprovalDecision::ApproveForRun) => {
                        self.publish(
                            EventStream::Run(run.id.clone()),
                            AppEvent::ToolCompleted {
                                run_id: run.id.clone(),
                                tool_call_id: ToolCallId::from("mock-tool-call-0"),
                                success: true,
                            },
                        );
                        self.publish(
                            EventStream::Run(run.id.clone()),
                            AppEvent::AssistantDelta {
                                run_id: run.id.clone(),
                                message_id: pawork_domain::MessageId::from("msg-tool"),
                                delta: "tool done".into(),
                            },
                        );
                        run_set_state(&run, RunState::Completed);
                        self.publish(
                            EventStream::Run(run.id.clone()),
                            AppEvent::RunChanged {
                                run_id: run.id.clone(),
                                state: RunState::Completed,
                            },
                        );
                    }
                    _ => {
                        run_set_state(&run, RunState::Cancelled);
                        self.publish(
                            EventStream::Run(run.id.clone()),
                            AppEvent::RunChanged {
                                run_id: run.id.clone(),
                                state: RunState::Cancelled,
                            },
                        );
                    }
                }
                return;
            }

            for text in &script.texts {
                self.publish(
                    EventStream::Run(run.id.clone()),
                    AppEvent::AssistantDelta {
                        run_id: run.id.clone(),
                        message_id: pawork_domain::MessageId::from("msg-1"),
                        delta: text.clone(),
                    },
                );
            }
            for text in &script.thinking {
                self.publish(
                    EventStream::Run(run.id.clone()),
                    AppEvent::ThinkingDelta {
                        run_id: run.id.clone(),
                        message_id: pawork_domain::MessageId::from("msg-1"),
                        delta: text.clone(),
                    },
                );
            }
            if script.wait_for_cancel {
                run.cancel.notified().await;
                run_set_state(&run, RunState::Cancelled);
                self.publish(
                    EventStream::Run(run.id.clone()),
                    AppEvent::RunChanged {
                        run_id: run.id.clone(),
                        state: RunState::Cancelled,
                    },
                );
                return;
            }
            run_set_state(&run, RunState::Completed);
            self.publish(
                EventStream::Run(run.id.clone()),
                AppEvent::RunChanged {
                    run_id: run.id.clone(),
                    state: RunState::Completed,
                },
            );
        });
    }
}

fn run_set_state(run: &RunEntry, state: RunState) {
    *run.state.lock().expect("run state") = state;
}

#[async_trait]
impl AcpCommandHost for MockAcpCommandHost {
    async fn dispatch(
        &self,
        command: AppCommandEnvelope,
    ) -> Result<AppResponseEnvelope, AcpHostError> {
        let request_id = QueryId::from(command.command_id.as_str());
        match command.command {
            AppCommand::WorkspaceAdd { root_path } => {
                let id = self.add_workspace(Path::new(&root_path));
                Ok(Self::response(
                    request_id,
                    AppResponse::Data(json!({ "id": id.as_str() })),
                ))
            }
            AppCommand::SessionCreate { .. } => {
                let id = SessionId::from(self.next_name("sess"));
                self.inner
                    .sessions
                    .lock()
                    .expect("sessions")
                    .insert(id.clone(), SessionEntry { id: id.clone() });
                Ok(Self::response(
                    request_id,
                    AppResponse::Data(json!({ "session_id": id.as_str() })),
                ))
            }
            AppCommand::RunStart {
                session_id,
                user_message,
                ..
            } => {
                if !self.session_exists(&session_id) {
                    return Ok(Self::response(
                        request_id,
                        AppResponse::Error(pawork_domain::ErrorContext {
                            category: pawork_domain::ErrorCategory::NotFound,
                            message: format!("unknown session {}", session_id.as_str()),
                            retryable: false,
                            retry_after_ms: None,
                            diagnostics: Default::default(),
                        }),
                    ));
                }
                self.inner
                    .captured_messages
                    .lock()
                    .expect("messages")
                    .push(user_message);
                let run_id = RunId::from(self.next_name("run"));
                let run = Arc::new(RunEntry {
                    id: run_id.clone(),
                    state: Mutex::new(RunState::StreamingResponse),
                    cancel: Arc::new(Notify::new()),
                    approval: Mutex::new(None),
                    approval_notify: Arc::new(Notify::new()),
                });
                self.inner
                    .runs
                    .lock()
                    .expect("runs")
                    .insert(run_id.clone(), Arc::clone(&run));
                Arc::new(Self {
                    inner: Arc::clone(&self.inner),
                })
                .play_script(run);
                Ok(Self::response(
                    request_id,
                    AppResponse::Accepted {
                        command_id: CommandId::from(command.command_id.as_str()),
                        run_id: Some(run_id),
                    },
                ))
            }
            AppCommand::RunCancel { run_id } => {
                if let Some(run) = self.inner.runs.lock().expect("runs").get(&run_id).cloned() {
                    run.cancel.notify_waiters();
                }
                Ok(Self::response(
                    request_id,
                    AppResponse::Accepted {
                        command_id: CommandId::from(command.command_id.as_str()),
                        run_id: Some(run_id),
                    },
                ))
            }
            AppCommand::ToolApprove {
                run_id, decision, ..
            } => {
                if let Some(run) = self.inner.runs.lock().expect("runs").get(&run_id).cloned() {
                    *run.approval.lock().expect("approval") = Some(decision);
                    run.approval_notify.notify_waiters();
                }
                Ok(Self::response(
                    request_id,
                    AppResponse::Accepted {
                        command_id: CommandId::from(command.command_id.as_str()),
                        run_id: Some(run_id),
                    },
                ))
            }
            other => Err(AcpHostError::Unavailable(format!(
                "mock host does not implement {other:?}"
            ))),
        }
    }

    async fn query(&self, query: AppQueryEnvelope) -> Result<AppResponseEnvelope, AcpHostError> {
        match query.query {
            AppQuery::WorkspaceList => {
                let workspaces = self
                    .inner
                    .workspaces
                    .lock()
                    .expect("workspaces")
                    .iter()
                    .map(|workspace| {
                        json!({
                            "id": workspace.id.as_str(),
                            "roots": [{ "path": workspace.root }],
                        })
                    })
                    .collect();
                Ok(Self::response(
                    query.request_id,
                    AppResponse::Data(Value::Array(workspaces)),
                ))
            }
            AppQuery::SessionGet { session_id, .. } => {
                if self.session_exists(&session_id) {
                    Ok(Self::response(
                        query.request_id,
                        AppResponse::Data(json!({ "session_id": session_id.as_str() })),
                    ))
                } else {
                    Ok(Self::response(
                        query.request_id,
                        AppResponse::Error(pawork_domain::ErrorContext {
                            category: pawork_domain::ErrorCategory::NotFound,
                            message: format!("unknown session {}", session_id.as_str()),
                            retryable: false,
                            retry_after_ms: None,
                            diagnostics: Default::default(),
                        }),
                    ))
                }
            }
            AppQuery::RunStatus { run_id } => {
                let state = match self.run_state(&run_id) {
                    Some(RunState::Cancelled) => "cancelled",
                    Some(RunState::Completed) => "completed",
                    Some(RunState::Failed) => "failed",
                    Some(_) => "running",
                    None => "unknown",
                };
                Ok(Self::response(
                    query.request_id,
                    AppResponse::Data(json!({ "state": state })),
                ))
            }
            other => Err(AcpHostError::Unavailable(format!(
                "mock host does not implement query {other:?}"
            ))),
        }
    }

    fn subscribe(&self) -> broadcast::Receiver<AppEventEnvelope> {
        self.inner.events.subscribe()
    }
}

/// 测试装配：内存 host + AcpHost + 常驻事件泵。
pub struct TestHarness {
    pub mock: Arc<MockAcpCommandHost>,
    pub host: Arc<AcpHost>,
    pump: tokio::task::JoinHandle<()>,
}

impl TestHarness {
    pub async fn new(script: MockScript) -> Self {
        let mock = Arc::new(MockAcpCommandHost::new(script));
        let store = Arc::new(InMemorySessionRegistryStore::default());
        let registry = Arc::new(SessionRegistry::new(store).await.expect("registry"));
        let host = Arc::new(AcpHost::new(
            Arc::clone(&mock) as Arc<dyn AcpCommandHost>,
            registry,
        ));
        let pump_host = Arc::clone(&host);
        let pump = tokio::spawn(async move {
            loop {
                pump_host.drain_and_pump().await;
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        });
        Self { mock, host, pump }
    }

    pub async fn initialize(&self) -> Result<Value, JsonRpcError> {
        self.host
            .handle_request(json!(1), "initialize", Some(initialize_params()))
            .await
    }

    /// 预置 workspace（Host 侧引导，不经 adapter 通道，不静默 WorkspaceAdd）。
    pub async fn prepare_workspace(&self, dir: &Path) -> WorkspaceId {
        self.mock.add_workspace(dir)
    }

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

    pub fn degraded_capabilities(&self) -> Vec<pawork_protocol::adapter::ClientCapability> {
        self.host.degraded_capabilities()
    }
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
