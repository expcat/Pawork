//! `PaworkClient`：连接 pawork Host 的 typed client。

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use pawork_domain::{
    CommandId, EventId, ModelId, QueryId, RunId, SessionId, Timestamp, WorkspaceId,
};
use pawork_protocol::{
    ActorIdentity, ApiHandle, ApiVersion, AppCommand, AppCommandEnvelope, AppEventEnvelope,
    AppQuery, AppQueryEnvelope, AppResponse, AppResponseEnvelope, CommandSource, EventStream,
    RunState,
};
use pawork_protocol::headless::wire::{
    CompatHistoryEntry, CompatImportReport, CompatSource, HeadlessRequest, HeadlessResponse,
    SdkCapability, TranslatedRequest,
};
use serde::Deserialize;
use serde_json::Value;
use tokio::sync::{mpsc, oneshot, Mutex};
use tokio::task::JoinHandle;

use crate::error::SdkError;
use crate::stream::{BackpressurePolicy, EventSubscription};
use crate::transport::{PaworkOptions, StdioTransport, Transport};
use crate::version::{SDK_API_VERSION, SDK_SUPPORTED_API_VERSIONS};

/// 启动 pawork Host 并建立连接的便捷入口（等价
/// [`PaworkClient::spawn`]）。
pub async fn spawn_pawork(options: PaworkOptions) -> Result<PaworkClient, SdkError> {
    PaworkClient::spawn(options).await
}

/// 连接后的会话视图（由 `AppResponse::Data` 解析；字段与 core-api 输出同构）。
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct SessionView {
    pub session_id: SessionId,
    pub workspace_id: WorkspaceId,
    pub title: String,
    pub revision: u64,
    pub open: bool,
    #[serde(default)]
    pub forked_from: Option<SessionId>,
}

/// Run 视图。
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct RunView {
    pub run_id: RunId,
    pub session_id: SessionId,
    pub model: ModelId,
    pub state: RunState,
    pub message_count: u64,
    pub revision: u64,
}

/// 取消结果。
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct CancelOutcome {
    pub run_id: RunId,
    pub cancelled: bool,
    pub already_cancelled: bool,
}

/// compat 导入结果（稳定协议入口）。
#[derive(Clone, Debug, PartialEq)]
pub struct CompatOutcome {
    pub request_id: String,
    pub report: CompatImportReport,
}

/// compat 历史分页。
#[derive(Clone, Debug, PartialEq)]
pub struct CompatHistoryPage {
    pub request_id: String,
    pub entries: Vec<CompatHistoryEntry>,
    pub cursor: Option<String>,
}

struct SubscriptionSlot {
    stream: EventStream,
    policy: BackpressurePolicy,
    sender: mpsc::Sender<AppEventEnvelope>,
    dropped: Arc<AtomicU64>,
    overflow_error: Arc<AtomicBool>,
}

/// 响应帧分类结果：未知帧类型与格式错误区分开（显式 unknown 错误）。
enum ClassifiedResponse {
    Frame(Box<HeadlessResponse>),
    UnknownType(String),
    Malformed(String),
}

fn classify_response(line: &str) -> ClassifiedResponse {
    let Ok(value) = serde_json::from_str::<Value>(line) else {
        return ClassifiedResponse::Malformed(format!("response line is not JSON: {line:?}"));
    };
    let frame_type = value
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    const KNOWN_TYPES: &[&str] = &[
        "hello_ack",
        "response",
        "event",
        "compat_import_result",
        "compat_history_result",
        "error",
    ];
    if !KNOWN_TYPES.contains(&frame_type) {
        return ClassifiedResponse::UnknownType(frame_type.to_string());
    }
    match serde_json::from_value::<HeadlessResponse>(value) {
        Ok(frame) => ClassifiedResponse::Frame(Box::new(frame)),
        Err(error) => ClassifiedResponse::Malformed(format!("response frame: {error}")),
    }
}

/// 事件路由/响应分发的共享状态。
#[derive(Default)]
struct RouterState {
    pending: HashMap<String, oneshot::Sender<Result<HeadlessResponse, SdkError>>>,
    subscriptions: HashMap<String, SubscriptionSlot>,
    unmatched_errors: u64,
    next_subscription: u64,
}

/// pawork Host 的 typed client。
///
/// 线程安全：`command`/`query`/`subscribe` 可从多个任务并发调用；事件由
/// 后台 reader 任务路由到各自订阅。`close` 后所有在途请求以
/// [`SdkErrorKind::Cancelled`] 结束。
pub struct PaworkClient {
    transport: Arc<dyn Transport>,
    router: Arc<Mutex<RouterState>>,
    reader: Arc<Mutex<Option<JoinHandle<()>>>>,
    options: PaworkOptions,
    handle: Arc<Mutex<Option<ApiHandle>>>,
    granted: Arc<Mutex<Vec<SdkCapability>>>,
    next_id: AtomicU64,
    closed: Arc<AtomicBool>,
}

impl PaworkClient {
    /// 启动 `pawork` 进程（`headless --json-stdio`）并完成握手。
    pub async fn spawn(options: PaworkOptions) -> Result<Self, SdkError> {
        let transport = StdioTransport::spawn(&options)?;
        Self::from_transport(Box::new(transport), options).await
    }

    /// 从任意 [`Transport`] 建立客户端并完成握手（mock 与测试入口）。
    pub async fn from_transport(
        transport: Box<dyn Transport>,
        options: PaworkOptions,
    ) -> Result<Self, SdkError> {
        let transport: Arc<dyn Transport> = Arc::from(transport);
        let router = Arc::new(Mutex::new(RouterState::default()));
        let closed = Arc::new(AtomicBool::new(false));
        let client = Self {
            transport: transport.clone(),
            router: router.clone(),
            reader: Arc::new(Mutex::new(None)),
            options,
            handle: Arc::new(Mutex::new(None)),
            granted: Arc::new(Mutex::new(Vec::new())),
            next_id: AtomicU64::new(1),
            closed: closed.clone(),
        };
        let reader = tokio::spawn(reader_loop(transport.clone(), router, closed.clone()));
        let mut client = client;
        *client.reader.lock().await = Some(reader);
        client.handshake().await?;
        Ok(client)
    }

    // ---------- 握手与元信息 ----------

    async fn handshake(&mut self) -> Result<(), SdkError> {
        let request = HeadlessRequest::Hello {
            client_name: self.options.client_name.clone(),
            client_version: self.options.client_version.clone(),
            supported_api_versions: SDK_SUPPORTED_API_VERSIONS.to_vec(),
            capabilities: self.options.capabilities.clone(),
        };
        let response = self
            .exchange("hello".to_string(), request, self.options.timeout)
            .await?;
        match response {
            HeadlessResponse::HelloAck {
                instance_id,
                negotiated,
                granted,
            } => {
                *self.handle.lock().await = Some(ApiHandle {
                    instance_id: instance_id.into(),
                    api_version: negotiated,
                });
                *self.granted.lock().await = granted;
                Ok(())
            }
            other => Err(SdkError::UnknownResponseType(format!(
                "expected hello_ack, got {other:?}"
            ))),
        }
    }

    /// 协商后的协议版本。
    pub async fn api_version(&self) -> Option<ApiVersion> {
        self.handle.lock().await.as_ref().map(|h| h.api_version)
    }

    /// Host 实例 id。
    pub async fn instance_id(&self) -> Option<String> {
        self.handle
            .lock()
            .await
            .as_ref()
            .map(|h| h.instance_id.as_str().to_string())
    }

    /// Host 授予的能力。
    pub async fn capabilities(&self) -> Vec<SdkCapability> {
        self.granted.lock().await.clone()
    }

    /// 是否仍有能力使用（未关闭）。
    pub fn is_open(&self) -> bool {
        !self.closed.load(Ordering::SeqCst)
    }

    /// 未关联到任何在途请求的 error 帧计数（Host 应总是关联 request_id）。
    pub async fn unmatched_error_count(&self) -> u64 {
        self.router.lock().await.unmatched_errors
    }

    // ---------- 底层往返 ----------

    /// 发送命令并等待响应信封。
    pub async fn send(&self, command: AppCommand) -> Result<AppResponseEnvelope, SdkError> {
        self.command(command).await
    }

    /// 发送命令并等待响应信封（[`send`](Self::send) 的别名）。
    pub async fn command(&self, command: AppCommand) -> Result<AppResponseEnvelope, SdkError> {
        let id = self.next_request_id("cmd");
        let envelope = AppCommandEnvelope {
            api_version: SDK_API_VERSION,
            command_id: CommandId::from(id.clone()),
            source: CommandSource::Automation,
            identity: ActorIdentity::Automation {
                name: self.options.client_name.clone(),
            },
            expected_revision: None,
            idempotency_key: None,
            issued_at: now_timestamp(),
            command,
        };
        let frame = self
            .exchange(
                id,
                HeadlessRequest::Command { envelope },
                self.options.timeout,
            )
            .await?;
        match frame {
            HeadlessResponse::Response { envelope } => Ok(envelope),
            other => Err(SdkError::UnknownResponseType(format!(
                "expected response frame, got {other:?}"
            ))),
        }
    }

    /// 发送查询并等待响应信封。
    pub async fn query(&self, query: AppQuery) -> Result<AppResponseEnvelope, SdkError> {
        let id = self.next_request_id("qry");
        let envelope = AppQueryEnvelope {
            api_version: SDK_API_VERSION,
            request_id: QueryId::from(id.clone()),
            source: CommandSource::Automation,
            identity: ActorIdentity::Automation {
                name: self.options.client_name.clone(),
            },
            issued_at: now_timestamp(),
            query,
        };
        let frame = self
            .exchange(
                id,
                HeadlessRequest::Query { envelope },
                self.options.timeout,
            )
            .await?;
        match frame {
            HeadlessResponse::Response { envelope } => Ok(envelope),
            other => Err(SdkError::UnknownResponseType(format!(
                "expected response frame, got {other:?}"
            ))),
        }
    }

    /// 统一交换：注册 pending → 写请求 → 带超时等待路由结果。
    async fn exchange(
        &self,
        request_id: String,
        request: HeadlessRequest,
        timeout: Duration,
    ) -> Result<HeadlessResponse, SdkError> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(SdkError::Closed("client is closed".into()));
        }
        let (tx, rx) = oneshot::channel();
        self.router
            .lock()
            .await
            .pending
            .insert(request_id.clone(), tx);
        let line = pawork_protocol::headless::translate::encode_request(&request)
            .map_err(|error| SdkError::from_error_frame(error.kind, error.message))?;
        let result = self.transport.write_line(&line).await;
        if let Err(error) = result {
            self.router.lock().await.pending.remove(&request_id);
            return Err(error);
        }
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(Ok(frame))) => Ok(frame),
            Ok(Ok(Err(error))) => Err(error),
            Ok(Err(_)) => {
                self.router.lock().await.pending.remove(&request_id);
                Err(SdkError::Cancelled(format!(
                    "request {request_id} was dropped by the reader"
                )))
            }
            Err(_) => {
                self.router.lock().await.pending.remove(&request_id);
                Err(SdkError::timeout(timeout))
            }
        }
    }

    fn next_request_id(&self, prefix: &str) -> String {
        let n = self.next_id.fetch_add(1, Ordering::SeqCst);
        format!("{prefix}-{n}")
    }

    // ---------- 高层 API ----------

    /// 创建会话。
    pub async fn create_session(
        &self,
        workspace_id: WorkspaceId,
        title: Option<String>,
    ) -> Result<SessionView, SdkError> {
        let envelope = self
            .command(AppCommand::SessionCreate {
                workspace_id,
                title,
            })
            .await?;
        parse_data(&envelope)
    }

    /// 打开既有会话。
    pub async fn open_session(&self, session_id: SessionId) -> Result<SessionView, SdkError> {
        let envelope = self.command(AppCommand::SessionOpen { session_id }).await?;
        parse_data(&envelope)
    }

    /// fork 会话（从指定父事件分叉）。
    pub async fn fork(
        &self,
        session_id: SessionId,
        parent_event_id: EventId,
    ) -> Result<SessionView, SdkError> {
        self.fork_session(session_id, parent_event_id).await
    }

    /// fork 会话（[`fork`](Self::fork) 的别名）。
    pub async fn fork_session(
        &self,
        session_id: SessionId,
        parent_event_id: EventId,
    ) -> Result<SessionView, SdkError> {
        let envelope = self
            .command(AppCommand::SessionFork {
                session_id,
                parent_event_id,
            })
            .await?;
        parse_data(&envelope)
    }

    /// 启动 run。
    pub async fn run_start(
        &self,
        session_id: SessionId,
        user_message: String,
        model: Option<ModelId>,
    ) -> Result<RunView, SdkError> {
        let envelope = self
            .command(AppCommand::RunStart {
                session_id,
                user_message,
                model,
                profile: None,
            })
            .await?;
        parse_data(&envelope)
    }

    /// 查询 run 状态。
    pub async fn run_status(&self, run_id: RunId) -> Result<RunView, SdkError> {
        let envelope = self.query(AppQuery::RunStatus { run_id }).await?;
        parse_data(&envelope)
    }

    /// 取消 run（Host 侧 `RunCancel`；订阅侧取消见
    /// [`EventSubscription`]）。
    pub async fn cancel(&self, run_id: RunId) -> Result<CancelOutcome, SdkError> {
        let envelope = self.command(AppCommand::RunCancel { run_id }).await?;
        parse_data(&envelope)
    }

    /// 重试 run。
    pub async fn run_retry(&self, run_id: RunId) -> Result<(), SdkError> {
        let envelope = self.command(AppCommand::RunRetry { run_id }).await?;
        match &envelope.response {
            AppResponse::Error(context) => Err(SdkError::RequestFailed(context.clone())),
            _ => Ok(()),
        }
    }

    /// 列出工作区。
    pub async fn list_workspaces(&self) -> Result<Value, SdkError> {
        let envelope = self.query(AppQuery::WorkspaceList).await?;
        match envelope.response {
            AppResponse::Data(value) => Ok(value),
            AppResponse::Error(context) => Err(SdkError::RequestFailed(context)),
            other => Err(SdkError::UnknownResponseType(format!(
                "expected data, got {other:?}"
            ))),
        }
    }

    /// 订阅事件流（有界通道；背压见 [`BackpressurePolicy`]）。
    ///
    /// `Global` 订阅收到所有事件；具体流订阅只收对应流事件。
    pub async fn subscribe(
        &self,
        stream: EventStream,
        policy: BackpressurePolicy,
        capacity: usize,
    ) -> Result<EventSubscription, SdkError> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(SdkError::Closed("client is closed".into()));
        }
        let (sender, receiver) = mpsc::channel(capacity.max(1));
        let dropped = Arc::new(AtomicU64::new(0));
        let overflow_error = Arc::new(AtomicBool::new(false));
        let label = stream_label(&stream);
        let mut router = self.router.lock().await;
        router.next_subscription += 1;
        let key = format!("{label}#{}", router.next_subscription);
        router.subscriptions.insert(
            key,
            SubscriptionSlot {
                stream,
                policy,
                sender,
                dropped: dropped.clone(),
                overflow_error: overflow_error.clone(),
            },
        );
        Ok(EventSubscription::new_with_counters(
            label,
            policy,
            receiver,
            dropped,
            overflow_error,
        ))
    }

    /// 取消订阅（等价 drop 订阅句柄）。
    pub async fn unsubscribe(&self, stream: EventStream) -> usize {
        let label = stream_label(&stream);
        let mut router = self.router.lock().await;
        let before = router.subscriptions.len();
        router
            .subscriptions
            .retain(|key, _| !key.starts_with(&label));
        before - router.subscriptions.len()
    }

    /// resume：重新打开会话并挂上事件流（会话级恢复语义）。
    pub async fn resume(
        &self,
        session_id: SessionId,
        capacity: usize,
    ) -> Result<(SessionView, EventSubscription), SdkError> {
        let session = self.open_session(session_id.clone()).await?;
        let subscription = self
            .subscribe(
                EventStream::Session(session_id),
                BackpressurePolicy::Error,
                capacity,
            )
            .await?;
        Ok((session, subscription))
    }

    // ---------- compat 协议入口 ----------

    /// 导入外部会话（稳定协议入口；Host 映射到 session-store 实现）。
    pub async fn import_compat(
        &self,
        source: CompatSource,
        content: String,
        dry_run: bool,
    ) -> Result<CompatOutcome, SdkError> {
        let request_id = self.next_request_id("compat");
        let frame = self
            .exchange(
                request_id.clone(),
                HeadlessRequest::CompatImport {
                    request_id: request_id.clone(),
                    source,
                    content,
                    options: Some(pawork_protocol::headless::CompatImportOptions { dry_run }),
                },
                self.options.timeout,
            )
            .await?;
        match frame {
            HeadlessResponse::CompatImportResult {
                request_id: _,
                report,
            } => Ok(CompatOutcome { request_id, report }),
            HeadlessResponse::Error {
                request_id: _,
                kind,
                message,
            } => Err(SdkError::from_error_frame(kind, message)),
            other => Err(SdkError::UnknownResponseType(format!(
                "expected compat_import_result, got {other:?}"
            ))),
        }
    }

    /// 查询导入历史（稳定协议入口，分页）。
    pub async fn compat_history(
        &self,
        limit: Option<u32>,
        cursor: Option<String>,
    ) -> Result<CompatHistoryPage, SdkError> {
        let request_id = self.next_request_id("compat-history");
        let frame = self
            .exchange(
                request_id.clone(),
                HeadlessRequest::CompatHistory {
                    request_id: request_id.clone(),
                    limit,
                    cursor,
                },
                self.options.timeout,
            )
            .await?;
        match frame {
            HeadlessResponse::CompatHistoryResult {
                request_id: _,
                entries,
                cursor,
            } => Ok(CompatHistoryPage {
                request_id,
                entries,
                cursor,
            }),
            HeadlessResponse::Error {
                request_id: _,
                kind,
                message,
            } => Err(SdkError::from_error_frame(kind, message)),
            other => Err(SdkError::UnknownResponseType(format!(
                "expected compat_history_result, got {other:?}"
            ))),
        }
    }

    /// 关闭连接：等待 reader 退出并回收子进程。
    pub async fn close(&self) -> Result<(), SdkError> {
        if self.closed.swap(true, Ordering::SeqCst) {
            return Ok(());
        }
        self.transport.close().await?;
        if let Some(reader) = self.reader.lock().await.take() {
            let _ = reader.await;
        }
        Ok(())
    }

    /// 翻译辅助：把请求帧翻译为可分发切片（转发给 Host 接线层/测试）。
    pub fn translate_request(request: &HeadlessRequest) -> Result<TranslatedRequest, SdkError> {
        pawork_protocol::headless::translate::translate_request(request)
            .map_err(|error| SdkError::from_error_frame(error.kind, error.message))
    }
}

/// 后台 reader：读行 → 分类 → 路由到 pending / 订阅。
async fn reader_loop(
    transport: Arc<dyn Transport>,
    router: Arc<Mutex<RouterState>>,
    closed: Arc<AtomicBool>,
) {
    loop {
        let line = transport.read_line().await;
        let line = match line {
            Ok(line) => line,
            Err(error) => {
                closed.store(true, Ordering::SeqCst);
                fail_all(&router, error).await;
                break;
            }
        };
        let classified = classify_response(&line);
        match classified {
            ClassifiedResponse::Frame(frame) => route_frame(&router, *frame).await,
            ClassifiedResponse::UnknownType(frame_type) => {
                let mut router = router.lock().await;
                fail_all_pending(&mut router, || {
                    SdkError::UnknownResponseType(frame_type.clone())
                });
            }
            ClassifiedResponse::Malformed(message) => {
                let mut router = router.lock().await;
                fail_all_pending(&mut router, || SdkError::MalformedFrame(message.clone()));
            }
        }
    }
}

async fn route_frame(router: &Arc<Mutex<RouterState>>, frame: HeadlessResponse) {
    if let HeadlessResponse::Event { envelope } = &frame {
        route_event(router, envelope.clone()).await;
        return;
    }
    let mut router = router.lock().await;
    if let HeadlessResponse::HelloAck { .. } = &frame {
        if let Some(tx) = router.pending.remove("hello") {
            let _ = tx.send(Ok(frame));
        } else {
            router.unmatched_errors += 1;
        }
        return;
    }
    let request_id = frame_request_id(&frame);
    if let Some(request_id) = request_id {
        if let Some(tx) = router.pending.remove(&request_id) {
            if let HeadlessResponse::Error { kind, message, .. } = &frame {
                let _ = tx.send(Err(SdkError::from_error_frame(*kind, message.clone())));
            } else {
                let _ = tx.send(Ok(frame));
            }
            return;
        }
    }
    // 无 request_id 的 error 帧：握手错误（如 incompatible_api_version）按
    // 协议不带 request_id，精确路由到握手期唯一、确定标识的 hello 交换
    // （不是「唯一 pending」猜测）。其余情况计数丢弃，绝不误配到普通在途
    // 请求（Host 的 lagged / backpressure error 帧即属此类）。
    if let HeadlessResponse::Error { kind, message, .. } = &frame {
        if let Some(tx) = router.pending.remove("hello") {
            let _ = tx.send(Err(SdkError::from_error_frame(*kind, message.clone())));
            return;
        }
    }
    router.unmatched_errors += 1;
}

async fn fail_all(router: &Arc<Mutex<RouterState>>, error: SdkError) {
    let mut router = router.lock().await;
    fail_all_pending(&mut router, || SdkError::Closed(error.to_string()));
    // 丢弃所有订阅 sender：订阅接收端随后读到 `None` → Cancelled。
    router.subscriptions.clear();
}

fn fail_all_pending(router: &mut RouterState, error: impl Fn() -> SdkError) {
    for (_, tx) in router.pending.drain() {
        let _ = tx.send(Err(error()));
    }
}

fn frame_request_id(frame: &HeadlessResponse) -> Option<String> {
    match frame {
        HeadlessResponse::Response { envelope } => Some(envelope.request_id.as_str().to_string()),
        HeadlessResponse::CompatImportResult { request_id, .. }
        | HeadlessResponse::CompatHistoryResult { request_id, .. } => Some(request_id.clone()),
        HeadlessResponse::Error { request_id, .. } => request_id.clone(),
        HeadlessResponse::HelloAck { .. } | HeadlessResponse::Event { .. } => None,
    }
}

async fn route_event(router: &Arc<Mutex<RouterState>>, envelope: AppEventEnvelope) {
    let mut router = router.lock().await;
    let mut prune = Vec::new();
    for (key, slot) in router.subscriptions.iter_mut() {
        let matches = match (&slot.stream, &envelope.stream) {
            (EventStream::Global, _) => true,
            (a, b) => a == b,
        };
        if !matches {
            continue;
        }
        match slot.sender.try_send(envelope.clone()) {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Full(_)) => {
                slot.dropped.fetch_add(1, Ordering::SeqCst);
                if slot.policy == BackpressurePolicy::Error {
                    slot.overflow_error.store(true, Ordering::SeqCst);
                }
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                prune.push(key.clone());
            }
        }
    }
    for key in prune {
        router.subscriptions.remove(&key);
    }
}

fn stream_label(stream: &EventStream) -> String {
    match stream {
        EventStream::Global => "global".into(),
        EventStream::Workspace(id) => format!("workspace/{}", id.as_str()),
        EventStream::Session(id) => format!("session/{}", id.as_str()),
        EventStream::Run(id) => format!("run/{}", id.as_str()),
        EventStream::Terminal(id) => format!("terminal/{id}"),
        EventStream::GuiClient(id) => format!("gui_client/{}", id.as_str()),
    }
}

fn parse_data<T: serde::de::DeserializeOwned>(
    envelope: &AppResponseEnvelope,
) -> Result<T, SdkError> {
    match &envelope.response {
        AppResponse::Data(value) => serde_json::from_value(value.clone())
            .map_err(|error| SdkError::MalformedFrame(format!("response data: {error}"))),
        AppResponse::Error(context) => Err(SdkError::RequestFailed(context.clone())),
        other => Err(SdkError::UnknownResponseType(format!(
            "expected data, got {other:?}"
        ))),
    }
}

/// Unix epoch 起的当前毫秒时间戳。
pub fn now_timestamp() -> Timestamp {
    use std::time::{SystemTime, UNIX_EPOCH};
    Timestamp::from_unix_millis(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_millis() as u64)
            .unwrap_or(0),
    )
}
