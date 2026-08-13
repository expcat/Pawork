//! LSP 客户端核心：JSON-RPC 关联、读循环、`initialize` / `shutdown` 握手与
//! 崩溃 restart 状态机。
//!
//! 设计要点：
//! - reader（只读半边）由读循环任务独占；writer / lifecycle 放在共享 inner 里，
//!   三者互不阻塞。
//! - restart 唯一入口在读循环：EOF / 读错误 / 显式触发都关闭 lifecycle 使服务端
//!   stdout EOF，读循环据此走同一 restart 路径（重新经注入 spawner spawn +
//!   initialize + resync），保证 sandbox guarantee 在 restart 阶段不降级。
//! - 崩溃代际隔离：pending 请求注册时记录当前代际（`generation`），restart
//!   settle 后只失败旧代际的 pending，restart 之后注册的新请求不受影响。
//! - restart 握手完成前不安装新 writer/lifecycle：任何握手 / 写失败路径都会先
//!   关闭刚 spawn 的 lifecycle（不泄漏新进程），期间新请求在 writer 不可用时
//!   快速失败，不会滞留 pending 被崩溃清理误伤。
//! - 重启预算语义一致：restart 尝试失败（spawn 失败 / 握手失败）在预算内按同一
//!   计数继续重试，预算耗尽才进入 Failed。
//! - diagnostics 按 URI 保留最新一次 `publishDiagnostics`；服务端通知经有界队列
//!   缓冲（超限丢弃最旧并计数），可经 `drain_notifications` 排空。

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use agent_domain::CancellationToken;
use serde_json::Value;
use tokio::sync::{oneshot, Mutex, Notify};
use tokio::task::JoinHandle;

use crate::capabilities::{normalize_capabilities, ClientCapabilities, ServerCapabilities};
use crate::descriptor::LanguageServerDescriptor;
use crate::doc::DocumentSync;
use crate::error::LspError;
use crate::framing::{encode_message, FrameEvent, LspFrameDecoder, MAX_FRAME_BYTES_HARD_LIMIT};
use crate::jsonrpc::{Notification, Request, ServerMessage};
use crate::transport::{
    ServerLifecycle, ServerReader, ServerSpawnConfig, ServerWriter, SharedSpawner,
};

/// 服务端通知缓冲上限：超过后丢弃最旧通知并计数（有界，可经
/// [`LspClient::drain_notifications`] 排空）。
pub const MAX_BUFFERED_NOTIFICATIONS: usize = 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    NotConnected,
    Initializing,
    Initialized,
    Restarting,
    ShuttingDown,
    Failed,
    Closed,
}

#[derive(Debug)]
pub(crate) struct PhaseState {
    phase: Phase,
    server_caps: Option<ServerCapabilities>,
    restart_count: u32,
    docs: DocumentSync,
}

type ResponseOutcome = Result<Value, LspError>;

/// 一个在途请求：发送响应的 oneshot + 注册时的崩溃代际。
#[derive(Debug)]
pub(crate) struct PendingRequest {
    pub sender: oneshot::Sender<ResponseOutcome>,
    /// 注册时的代际。restart settle 后的清理（`fail_pending_older_than`）
    /// 只失败 `generation < 当前代际` 的请求，restart 后注册的新请求不受影响。
    pub generation: u64,
}

pub(crate) struct ClientInner {
    pub descriptor: LanguageServerDescriptor,
    pub spawn_config: ServerSpawnConfig,
    pub spawner: SharedSpawner,
    pub client_caps: ClientCapabilities,
    pub writer: Mutex<Option<Box<dyn ServerWriter>>>,
    pub lifecycle: Mutex<Option<Box<dyn ServerLifecycle>>>,
    pub pending: Mutex<HashMap<i64, PendingRequest>>,
    pub next_id: AtomicI64,
    /// uri → 该 uri 最新一次 `publishDiagnostics`（覆盖旧值，不累积）。
    pub diagnostics: Mutex<HashMap<String, crate::protocol::DocumentDiagnostic>>,
    /// 有界服务端通知队列（见 [`MAX_BUFFERED_NOTIFICATIONS`]）。
    pub notifications: Mutex<VecDeque<ServerMessage>>,
    /// 因超限被丢弃的通知计数（有界性可观测）。
    pub dropped_notifications: AtomicU64,
    pub state: Mutex<PhaseState>,
    pub restarted: Notify,
    /// restart 尝试完成（成功或失败）的代数；在 `restarted` notify 之前递增，
    /// 供 `wait_restarted` 检测「notify 早于 waiter 注册」的丢失窗口。
    pub restarted_seq: AtomicU64,
    /// 崩溃代际：每次 restart 开始时递增；pending 请求按注册时代际隔离清理。
    pub generation: AtomicU64,
    pub cancel: CancellationToken,
}

pub struct LspClient {
    inner: Arc<ClientInner>,
    reader_task: Mutex<Option<JoinHandle<()>>>,
}

impl LspClient {
    pub async fn start(
        descriptor: LanguageServerDescriptor,
        spawner: SharedSpawner,
        spawn_config: ServerSpawnConfig,
        client_caps: ClientCapabilities,
    ) -> Result<Self, LspError> {
        let cancel = CancellationToken::new();
        let inner = Arc::new(ClientInner {
            descriptor: descriptor.clone(),
            spawn_config,
            spawner,
            client_caps,
            writer: Mutex::new(None),
            lifecycle: Mutex::new(None),
            pending: Mutex::new(HashMap::new()),
            next_id: AtomicI64::new(1),
            diagnostics: Mutex::new(HashMap::new()),
            notifications: Mutex::new(VecDeque::new()),
            dropped_notifications: AtomicU64::new(0),
            state: Mutex::new(PhaseState {
                phase: Phase::Initializing,
                server_caps: None,
                restart_count: 0,
                docs: DocumentSync::new(),
            }),
            restarted: Notify::new(),
            restarted_seq: AtomicU64::new(0),
            generation: AtomicU64::new(0),
            cancel: cancel.clone(),
        });

        let spawned = inner
            .spawner
            .spawn(&inner.descriptor, &inner.spawn_config, cancel.clone())
            .await?;
        *inner.writer.lock().await = Some(spawned.writer);
        *inner.lifecycle.lock().await = Some(spawned.lifecycle);

        let client = Self {
            inner: inner.clone(),
            reader_task: Mutex::new(None),
        };
        let reader_handle = tokio::spawn(run_reader(inner.clone(), spawned.reader));
        *client.reader_task.lock().await = Some(reader_handle);

        if let Err(e) = client.initialize_handshake().await {
            // 初始握手失败：关闭刚 spawn 的进程并让读循环退出，避免泄漏。
            *inner.writer.lock().await = None;
            if let Some(mut life) = inner.lifecycle.lock().await.take() {
                let _ = life.close().await;
            }
            inner.cancel.cancel();
            if let Some(handle) = client.reader_task.lock().await.take() {
                let _ = tokio::time::timeout(Duration::from_secs(2), handle).await;
            }
            return Err(e);
        }
        Ok(client)
    }

    pub async fn phase(&self) -> Phase {
        self.inner.state.lock().await.phase
    }

    pub async fn server_capabilities(&self) -> Option<ServerCapabilities> {
        self.inner.state.lock().await.server_caps.clone()
    }

    pub async fn restart_count(&self) -> u32 {
        self.inner.state.lock().await.restart_count
    }

    pub fn descriptor(&self) -> &LanguageServerDescriptor {
        &self.inner.descriptor
    }

    pub fn cancel_token(&self) -> CancellationToken {
        self.inner.cancel.clone()
    }

    pub async fn wait_restarted(&self, timeout: Duration) -> bool {
        // 读循环保证：请求报错（fail_pending）发生在 restart 尝试 settle 之后；
        // 因此调用方看到 seq > 0 即代表客户端已稳定（restart 成功或进入 Failed）。
        if self.inner.restarted_seq.load(Ordering::Acquire) > 0 {
            return true;
        }
        match tokio::time::timeout(timeout, self.inner.restarted.notified()).await {
            Ok(()) => true,
            Err(_) => false,
        }
    }

    async fn initialize_handshake(&self) -> Result<(), LspError> {
        let result = self
            .request_raw(
                "initialize",
                Some(self.init_params()),
                self.inner.descriptor.startup_timeout,
            )
            .await?;
        let caps =
            normalize_capabilities(&result.get("capabilities").cloned().unwrap_or(Value::Null));
        {
            let mut st = self.inner.state.lock().await;
            st.server_caps = Some(caps);
            st.phase = Phase::Initialized;
        }
        self.notify("initialized", Some(serde_json::json!({})))
            .await?;
        self.write_frame(&configuration_frame(&self.inner.descriptor)?)
            .await?;
        Ok(())
    }

    fn init_params(&self) -> Value {
        init_params_for(&self.inner)
    }

    pub async fn request_value(
        &self,
        method: &str,
        params: Option<Value>,
        timeout: Duration,
        cancel: Option<&CancellationToken>,
    ) -> Result<Value, LspError> {
        let phase = self.inner.state.lock().await.phase;
        if phase != Phase::Initializing && phase != Phase::Restarting {
            if phase != Phase::Initialized {
                return Err(LspError::InvalidState(format!("client in phase {phase:?}")));
            }
            if let Some(caps) = self.inner.state.lock().await.server_caps.clone() {
                if !crate::capabilities::method_supported(&caps, method) {
                    return Err(LspError::Unsupported {
                        method: method.to_string(),
                    });
                }
            }
        }
        match cancel {
            Some(c) => {
                self.request_cancelable_inner(method, params, timeout, c)
                    .await
            }
            None => self.request_raw(method, params, timeout).await,
        }
    }

    async fn request_raw(
        &self,
        method: &str,
        params: Option<Value>,
        timeout: Duration,
    ) -> Result<Value, LspError> {
        let id = self.inner.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel::<ResponseOutcome>();
        let generation = self.inner.generation.load(Ordering::Acquire);
        self.inner.pending.lock().await.insert(
            id,
            PendingRequest {
                sender: tx,
                generation,
            },
        );

        let request = Request::new(id, method, params);
        let body = serde_json::to_vec(&request).map_err(LspError::Json)?;
        if let Err(e) = self.write_frame(&encode_message(&body)).await {
            self.inner.pending.lock().await.remove(&id);
            return Err(e);
        }

        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(Ok(v))) => Ok(v),
            Ok(Ok(Err(e))) => Err(e),
            Ok(Err(_)) => Err(LspError::Transport(format!("channel closed for {method}"))),
            Err(_) => {
                let _ = self
                    .notify("$/cancelRequest", Some(serde_json::json!({ "id": id })))
                    .await;
                self.inner.pending.lock().await.remove(&id);
                Err(LspError::Timeout {
                    method: method.to_string(),
                    timeout,
                })
            }
        }
    }

    async fn request_cancelable_inner(
        &self,
        method: &str,
        params: Option<Value>,
        timeout: Duration,
        cancel: &CancellationToken,
    ) -> Result<Value, LspError> {
        let id = self.inner.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel::<ResponseOutcome>();
        let generation = self.inner.generation.load(Ordering::Acquire);
        self.inner.pending.lock().await.insert(
            id,
            PendingRequest {
                sender: tx,
                generation,
            },
        );
        let request = Request::new(id, method, params);
        let body = serde_json::to_vec(&request).map_err(LspError::Json)?;
        if let Err(e) = self.write_frame(&encode_message(&body)).await {
            self.inner.pending.lock().await.remove(&id);
            return Err(e);
        }
        let cancel_clone = cancel.clone();
        tokio::select! {
            biased;
            _ = cancel_clone.cancelled() => {
                let _ = self.notify("$/cancelRequest", Some(serde_json::json!({ "id": id }))).await;
                self.inner.pending.lock().await.remove(&id);
                Err(LspError::Cancelled { method: method.to_string() })
            }
            _ = tokio::time::sleep(timeout) => {
                let _ = self.notify("$/cancelRequest", Some(serde_json::json!({ "id": id }))).await;
                self.inner.pending.lock().await.remove(&id);
                Err(LspError::Timeout { method: method.to_string(), timeout })
            }
            resp = rx => match resp {
                Ok(Ok(v)) => Ok(v),
                Ok(Err(e)) => Err(e),
                Err(_) => Err(LspError::Transport(format!("channel closed for {method}"))),
            },
        }
    }

    pub async fn notify(&self, method: &str, params: Option<Value>) -> Result<(), LspError> {
        let n = Notification::new(method, params);
        let body = serde_json::to_vec(&n).map_err(LspError::Json)?;
        self.write_frame(&encode_message(&body)).await
    }

    async fn write_frame(&self, frame: &[u8]) -> Result<(), LspError> {
        let mut guard = self.inner.writer.lock().await;
        match guard.as_mut() {
            Some(w) => w.write(frame).await,
            None => Err(LspError::Transport(
                "writer unavailable (closed/restarting)".into(),
            )),
        }
    }

    pub(crate) async fn with_docs<R>(&self, f: impl FnOnce(&mut DocumentSync) -> R) -> R {
        let mut st = self.inner.state.lock().await;
        f(&mut st.docs)
    }

    pub(crate) fn inner(&self) -> &Arc<ClientInner> {
        &self.inner
    }

    pub async fn diagnostics_snapshot(&self) -> Vec<crate::protocol::DocumentDiagnostic> {
        self.inner
            .diagnostics
            .lock()
            .await
            .values()
            .cloned()
            .collect()
    }

    /// 排空缓冲的服务端通知。队列有界（[`MAX_BUFFERED_NOTIFICATIONS`]），
    /// 超限时丢弃最旧通知并计入 [`Self::dropped_notifications`]。
    pub async fn drain_notifications(&self) -> Vec<ServerMessage> {
        std::mem::take(&mut *self.inner.notifications.lock().await).into()
    }

    /// 因超出通知缓冲上限而被丢弃的通知数。
    pub async fn dropped_notifications(&self) -> u64 {
        self.inner.dropped_notifications.load(Ordering::Relaxed)
    }

    pub async fn shutdown(self) -> Result<(), LspError> {
        {
            let mut st = self.inner.state.lock().await;
            if matches!(st.phase, Phase::Closed | Phase::Failed) {
                return Ok(());
            }
            st.phase = Phase::ShuttingDown;
        }
        let _ = self
            .request_raw("shutdown", None, self.inner.descriptor.shutdown_timeout)
            .await;
        let _ = self.notify("exit", None).await;
        {
            let mut st = self.inner.state.lock().await;
            st.phase = Phase::Closed;
        }
        // 关闭 writer / lifecycle：让对端读到 EOF、读循环退出。
        *self.inner.writer.lock().await = None;
        if let Some(mut life) = self.inner.lifecycle.lock().await.take() {
            let _ = life.close().await;
        }
        self.inner.cancel.cancel();
        if let Some(handle) = self.reader_task.lock().await.take() {
            // reader 在对端 EOF 后会自然退出；给一个有界等待避免永久挂起。
            let _ = tokio::time::timeout(Duration::from_secs(2), handle).await;
        }
        Ok(())
    }

    /// 显式 restart：关闭 lifecycle 并丢弃 writer（服务端读到 EOF / stdout 关闭），
    /// 读循环据此走统一 restart 路径（重新经注入 spawner）。
    pub async fn restart(&self, timeout: Duration) -> Result<(), LspError> {
        {
            let st = self.inner.state.lock().await;
            if matches!(
                st.phase,
                Phase::Closed | Phase::Failed | Phase::ShuttingDown
            ) {
                return Err(LspError::InvalidState(format!(
                    "cannot restart from {:?}",
                    st.phase
                )));
            }
        }
        self.inner.fail_pending_with("explicit restart").await;
        // 锚定当前已完成代数：只等待「本次触发」的 restart 尝试完成，
        // 不因更早的崩溃 restart 已结束而提前返回。
        let anchor = self.inner.restarted_seq.load(Ordering::Acquire);
        if let Some(mut life) = self.inner.lifecycle.lock().await.take() {
            let _ = life.close().await;
        }
        *self.inner.writer.lock().await = None;
        if !self.wait_seq_after(anchor, timeout).await {
            return Err(LspError::Timeout {
                method: "restart".into(),
                timeout,
            });
        }
        if self.inner.state.lock().await.phase == Phase::Failed {
            return Err(LspError::InvalidState("restart exhausted budget".into()));
        }
        Ok(())
    }

    /// 等待 restart 完成代数超过 `anchor`。`restarted_seq` 在 notify 前递增，
    /// 因此即使 notify 早于 waiter 注册，超时后的回读也能确认完成。
    async fn wait_seq_after(&self, anchor: u64, timeout: Duration) -> bool {
        if self.inner.restarted_seq.load(Ordering::Acquire) > anchor {
            return true;
        }
        match tokio::time::timeout(timeout, self.inner.restarted.notified()).await {
            Ok(()) => self.inner.restarted_seq.load(Ordering::Acquire) > anchor,
            Err(_) => self.inner.restarted_seq.load(Ordering::Acquire) > anchor,
        }
    }
}

async fn run_reader(inner: Arc<ClientInner>, mut reader: Box<dyn ServerReader>) {
    let mut decoder = LspFrameDecoder::new(MAX_FRAME_BYTES_HARD_LIMIT);
    loop {
        let outcome = reader.read().await;
        if inner.cancel.is_cancelled() {
            return;
        }
        match outcome {
            Ok(Some(bytes)) => {
                decoder.feed(&bytes);
                let mut poisoned = false;
                loop {
                    match decoder.decode_next() {
                        Ok(FrameEvent::Complete(body)) => {
                            if let Err(e) = dispatch_message(&inner, &body).await {
                                tracing::warn!(target: "pawork.lsp", error = %e, "dispatch error");
                            }
                        }
                        Ok(FrameEvent::NeedMoreData) => break,
                        Err(e) => {
                            tracing::error!(target: "pawork.lsp", error = %e, "fatal framing");
                            inner
                                .fail_pending_with(format!("fatal framing error: {e}"))
                                .await;
                            poisoned = true;
                            break;
                        }
                    }
                }
                if poisoned {
                    match recover_after_crash(&inner, "fatal framing error; stream poisoned").await
                    {
                        Some(new_reader) => {
                            reader = new_reader;
                            decoder = LspFrameDecoder::new(MAX_FRAME_BYTES_HARD_LIMIT);
                        }
                        None => return,
                    }
                }
            }
            Ok(None) => {
                let phase = inner.state.lock().await.phase;
                if matches!(phase, Phase::ShuttingDown | Phase::Closed) {
                    return;
                }
                match recover_after_crash(&inner, "language server closed stream").await {
                    Some(new_reader) => {
                        reader = new_reader;
                        decoder = LspFrameDecoder::new(MAX_FRAME_BYTES_HARD_LIMIT);
                    }
                    None => return,
                }
            }
            Err(e) => {
                let reason = format!("{e}");
                match recover_after_crash(&inner, &reason).await {
                    Some(new_reader) => {
                        reader = new_reader;
                        decoder = LspFrameDecoder::new(MAX_FRAME_BYTES_HARD_LIMIT);
                    }
                    None => return,
                }
            }
        }
    }
}

/// 崩溃后的统一恢复路径：按预算重试 restart，settle 后只失败旧代际的 pending。
/// 返回 `Some(new_reader)` 表示已稳定恢复；`None` 表示已进入 Failed / 被取消，
/// 读循环应退出。
async fn recover_after_crash(
    inner: &Arc<ClientInner>,
    reason: &str,
) -> Option<Box<dyn ServerReader>> {
    match restart_after_failure(inner).await {
        Some(new_reader) => {
            // 只失败崩溃代际（`generation < 当前`）的 pending；restart 期间 / 之后
            // 注册的新代际请求不受本次崩溃清理影响。
            let generation = inner.generation.load(Ordering::Acquire);
            inner.fail_pending_older_than(reason, generation).await;
            Some(new_reader)
        }
        None => {
            // 客户端已进入 Failed：所有 pending 都无望，全部失败。
            inner.fail_pending_with(reason).await;
            None
        }
    }
}

async fn restart_after_failure(inner: &Arc<ClientInner>) -> Option<Box<dyn ServerReader>> {
    // 连续重启预算语义：单次 restart 尝试失败（spawn 失败 / 握手失败）不是终点，
    // 在预算内按同一 restart_count 继续尝试；预算耗尽（restart_once 已置 Failed）
    // 或客户端被取消时才结束。每次尝试都计入 budget，循环必然有界。
    loop {
        match restart_once(inner).await {
            Ok(reader) => return Some(reader),
            Err(e) => {
                if inner.cancel.is_cancelled() {
                    tracing::warn!(target: "pawork.lsp", error = %e, "restart aborted by cancellation");
                    return None;
                }
                if inner.state.lock().await.phase == Phase::Failed {
                    tracing::error!(target: "pawork.lsp", error = %e, "restart budget exhausted; entering Failed");
                    // 通知等待 restart 的调用方：restart 尝试已结束（失败），
                    // 由调用方检查 phase。
                    inner.restarted_seq.fetch_add(1, Ordering::AcqRel);
                    inner.restarted.notify_waiters();
                    return None;
                }
                tracing::warn!(target: "pawork.lsp", error = %e, "restart attempt failed; retrying within budget");
            }
        }
    }
}

async fn close_lifecycle(lifecycle: &mut Option<Box<dyn ServerLifecycle>>) {
    if let Some(mut life) = lifecycle.take() {
        let _ = life.close().await;
    }
}

async fn restart_once(inner: &Arc<ClientInner>) -> Result<Box<dyn ServerReader>, LspError> {
    let descriptor = inner.descriptor.clone();
    {
        let mut st = inner.state.lock().await;
        let allow = descriptor.restart_on_crash;
        // max_restarts=0 表示「不重启」：不加 `.max(1)`，否则预算 0 也会重启一次。
        let under_budget = st.restart_count < descriptor.max_restarts;
        if !allow || !under_budget {
            st.phase = Phase::Failed;
            return Err(LspError::InvalidState(format!(
                "restart denied (policy={allow}, budget_left={})",
                descriptor.max_restarts.saturating_sub(st.restart_count)
            )));
        }
        st.restart_count += 1;
        st.phase = Phase::Restarting;
    }
    // 崩溃代际边界：本次 restart 之后注册的请求属于新代际，settle 后的清理
    // 只失败旧代际（见 `fail_pending_older_than`）。
    inner.generation.fetch_add(1, Ordering::AcqRel);
    // 先摘除并关闭旧 writer/lifecycle：restart 全程 writer 不可用，期间新请求
    // 快速失败，不会滞留 pending 被崩溃清理误伤。
    *inner.writer.lock().await = None;
    if let Some(mut life) = inner.lifecycle.lock().await.take() {
        let _ = life.close().await;
    }
    let spawned = inner
        .spawner
        .spawn(&descriptor, &inner.spawn_config, inner.cancel.clone())
        .await?;
    let mut reader = spawned.reader;
    let mut writer = spawned.writer;
    let mut lifecycle = Some(spawned.lifecycle);

    // 握手必须由 reader 任务自行读取新服务端流完成：restart 期间没有其他读循环，
    // 若经 pending map 等待响应会与读循环互锁。非握手帧（如 publishDiagnostics）
    // 正常分发。
    //
    // 握手全部完成前不安装 writer/lifecycle：任何失败路径都会先关闭新 lifecycle，
    // 绝不泄漏刚 spawn 的进程；restart 期间新请求在 writer=None 时快速失败。
    let handshake = async {
        let id = inner.next_id.fetch_add(1, Ordering::Relaxed);
        let request = Request::new(id, "initialize", Some(init_params_for(inner)));
        let body = serde_json::to_vec(&request).map_err(LspError::Json)?;
        writer.write(&encode_message(&body)).await?;

        let mut decoder = LspFrameDecoder::new(MAX_FRAME_BYTES_HARD_LIMIT);
        let result = 'read: loop {
            if inner.cancel.is_cancelled() {
                return Err(LspError::InvalidState(
                    "cancelled during restart handshake".into(),
                ));
            }
            let chunk = reader.read().await?;
            let bytes = match chunk {
                Some(b) => b,
                None => {
                    return Err(LspError::Transport(
                        "language server exited during restart handshake".into(),
                    ))
                }
            };
            decoder.feed(&bytes);
            while let FrameEvent::Complete(body) = decoder.decode_next()? {
                let msg: ServerMessage = serde_json::from_slice(&body).map_err(LspError::Json)?;
                if let crate::jsonrpc::ServerMessageKind::Response(resp) = msg.kind() {
                    if resp.id.as_ref().and_then(|v| v.as_i64()) == Some(id) {
                        break 'read match resp.error.clone() {
                            Some(err) => {
                                return Err(LspError::ServerError {
                                    code: err.code,
                                    message: err.message,
                                    data: err.data,
                                })
                            }
                            None => resp.result.clone().unwrap_or(Value::Null),
                        };
                    }
                }
                if let Err(e) = dispatch_message(inner, &body).await {
                    tracing::warn!(
                        target: "pawork.lsp",
                        error = %e,
                        "dispatch error during restart handshake"
                    );
                }
            }
        };
        let caps =
            normalize_capabilities(&result.get("capabilities").cloned().unwrap_or(Value::Null));
        {
            let mut st = inner.state.lock().await;
            st.server_caps = Some(caps);
            st.phase = Phase::Initialized;
        }
        let n = Notification::new("initialized", Some(serde_json::json!({})));
        let body = serde_json::to_vec(&n).map_err(LspError::Json)?;
        writer.write(&encode_message(&body)).await?;
        writer.write(&configuration_frame(&descriptor)?).await?;
        let resync = inner.state.lock().await.docs.resync();
        for params in resync {
            let n = Notification::new("textDocument/didOpen", Some(params));
            let body = serde_json::to_vec(&n).map_err(LspError::Json)?;
            writer.write(&encode_message(&body)).await?;
        }
        Ok(())
    };
    match tokio::time::timeout(descriptor.startup_timeout, handshake).await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => {
            close_lifecycle(&mut lifecycle).await;
            return Err(e);
        }
        Err(_) => {
            close_lifecycle(&mut lifecycle).await;
            return Err(LspError::Timeout {
                method: "initialize".into(),
                timeout: descriptor.startup_timeout,
            });
        }
    }

    // 全部成功：安装新句柄。
    *inner.writer.lock().await = Some(writer);
    *inner.lifecycle.lock().await = Some(lifecycle.take().expect("lifecycle present"));
    inner.restarted_seq.fetch_add(1, Ordering::AcqRel);
    inner.restarted.notify_waiters();
    Ok(reader)
}

fn init_params_for(inner: &ClientInner) -> Value {
    serde_json::json!({
        "processId": std::process::id(),
        "clientInfo": { "name": "pawork-lsp", "version": "0.0.0" },
        "rootUri": inner.descriptor.workspace_folder.as_ref().map(|f| f.uri.clone()),
        "workspaceFolders": inner.descriptor.workspace_folder.as_ref().map(|f| {
            serde_json::json!([{ "uri": f.uri, "name": f.name }])
        }),
        "initializationOptions": inner.descriptor.initialization_options.clone(),
        "capabilities": inner.client_caps.to_lsp()["capabilities"].clone(),
    })
}

/// `workspace/didChangeConfiguration` 帧：把描述符 settings 发送给服务端。
/// 未配置 settings 时发送空对象 `{}`（多数语言服务按惯例需要至少一次配置通知）。
fn configuration_frame(descriptor: &LanguageServerDescriptor) -> Result<Vec<u8>, LspError> {
    let settings = descriptor
        .settings
        .clone()
        .unwrap_or_else(|| serde_json::json!({}));
    let n = Notification::new(
        "workspace/didChangeConfiguration",
        Some(serde_json::json!({ "settings": settings })),
    );
    let body = serde_json::to_vec(&n).map_err(LspError::Json)?;
    Ok(encode_message(&body))
}

async fn dispatch_message(inner: &Arc<ClientInner>, body: &[u8]) -> Result<(), LspError> {
    let msg: ServerMessage = serde_json::from_slice(body).map_err(LspError::Json)?;
    match msg.kind() {
        crate::jsonrpc::ServerMessageKind::Response(resp) => {
            let id = resp.id.as_ref().and_then(|v| v.as_i64());
            if let Some(id) = id {
                let sender = inner.pending.lock().await.remove(&id);
                if let Some(sender) = sender {
                    let outcome = match resp.error.clone() {
                        Some(err) => Err(LspError::ServerError {
                            code: err.code,
                            message: err.message,
                            data: err.data,
                        }),
                        None => Ok(resp.result.clone().unwrap_or(Value::Null)),
                    };
                    let _ = sender.sender.send(outcome);
                }
            }
        }
        crate::jsonrpc::ServerMessageKind::Notification(_) => {
            if msg.method.as_deref() == Some("textDocument/publishDiagnostics") {
                if let Some(params) = &msg.params {
                    if let Ok(diag) = parse_publish_diagnostics(params) {
                        // 按 URI 保留最新一次推送（覆盖旧值，不累积）。
                        inner
                            .diagnostics
                            .lock()
                            .await
                            .insert(diag.uri.clone(), diag);
                    }
                }
            }
            let mut queue = inner.notifications.lock().await;
            if queue.len() >= MAX_BUFFERED_NOTIFICATIONS {
                queue.pop_front();
                inner.dropped_notifications.fetch_add(1, Ordering::Relaxed);
            }
            queue.push_back(msg);
        }
    }
    Ok(())
}

fn parse_publish_diagnostics(
    params: &Value,
) -> Result<crate::protocol::DocumentDiagnostic, LspError> {
    let uri = params
        .get("uri")
        .and_then(|v| v.as_str())
        .ok_or_else(|| LspError::Transport("publishDiagnostics missing uri".into()))?
        .to_string();
    let version = params.get("version").and_then(|v| v.as_i64());
    let diagnostics: Vec<crate::protocol::Diagnostic> = params
        .get("diagnostics")
        .and_then(|v| v.as_array())
        .map(|arr| {
            serde_json::from_value::<Vec<crate::protocol::Diagnostic>>(Value::Array(arr.clone()))
                .unwrap_or_default()
        })
        .unwrap_or_default();
    Ok(crate::protocol::DocumentDiagnostic {
        uri,
        version,
        diagnostics,
    })
}

impl ClientInner {
    async fn fail_pending_with(&self, reason: impl Into<String>) {
        let drained = std::mem::take(&mut *self.pending.lock().await);
        let reason = reason.into();
        for (_id, pending) in drained {
            let _ = pending
                .sender
                .send(Err(LspError::Transport(reason.clone())));
        }
    }

    /// 只失败注册代际早于 `generation` 的 pending（崩溃代际隔离）：
    /// restart settle 后调用，restart 期间 / 之后注册的新代际请求不受影响。
    async fn fail_pending_older_than(&self, reason: impl Into<String>, generation: u64) {
        let reason = reason.into();
        let mut pending = self.pending.lock().await;
        let doomed: Vec<i64> = pending
            .iter()
            .filter(|(_, p)| p.generation < generation)
            .map(|(id, _)| *id)
            .collect();
        for id in doomed {
            if let Some(p) = pending.remove(&id) {
                let _ = p.sender.send(Err(LspError::Transport(reason.clone())));
            }
        }
    }
}
