//! PTY Service：集成终端会话层（portable-pty 之上）。
//!
//! 提供 create / resize / write / output / exit / kill、多会话、
//! 重连快照与游标、有界环形缓冲、session 归属与幂等自动清理。
//!
//! blocking I/O（portable-pty reader/writer/wait/kill）一律放到 `spawn_blocking`，
//! 避免阻塞 async runtime。

mod buffer;

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use parking_lot::Mutex;
use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
use thiserror::Error;
use tokio::sync::{broadcast, Notify};
use tracing::{debug, warn};

use crate::process::ProcessLimits;
use crate::tree::ProcessTreeGuard;

pub use buffer::{OutputCursor, RingBuffer, RingReadError};

/// PTY 归属的工作会话标识（本 crate 不依赖 pawork-domain）。
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct OwnerSessionId(String);

impl OwnerSessionId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for OwnerSessionId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl From<String> for OwnerSessionId {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

impl From<&str> for OwnerSessionId {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

/// PTY 终端会话标识。
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct TerminalId(String);

impl TerminalId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for TerminalId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl From<String> for TerminalId {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

impl From<&str> for TerminalId {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

/// 默认输出缓冲容量（256 KiB）。
pub const DEFAULT_BUFFER_CAPACITY: usize = 256 * 1024;

/// 默认广播通道容量（事件条数）。
const DEFAULT_EVENT_CAPACITY: usize = 256;

/// 终止后等待 waiter 回收子进程的最长时间。
const CLEANUP_GRACE: Duration = Duration::from_secs(5);

/// `portable-pty` 的 Unix spawn 路径会在 fork 后的 `pre_exec` 中配置 session/TTY。
/// musl 下并发进入该路径会触发未定义行为，因此把“打开 PTY → spawn → 绑定进程树”
/// 收敛为一个短临界区；会话 I/O 与后续生命周期仍完全并发。
static PTY_SPAWN_LOCK: Mutex<()> = Mutex::new(());

/// 创建 PTY 会话的规格。
#[derive(Clone, Debug)]
pub struct PtyCreateSpec {
    /// 归属的 Agent / 工作 Session。
    pub owner_session: OwnerSessionId,
    /// shell 程序；`None` 时使用平台默认 shell。
    pub shell: Option<String>,
    /// shell 参数。
    pub args: Vec<String>,
    /// 工作目录。
    pub cwd: Option<PathBuf>,
    /// 额外环境变量。
    pub env: Vec<(String, String)>,
    /// 初始窗口尺寸。
    pub size: PtyWindowSize,
    /// 有界输出缓冲容量（字节）。
    pub buffer_capacity: usize,
}

impl Default for PtyCreateSpec {
    fn default() -> Self {
        Self {
            owner_session: OwnerSessionId::new("default"),
            shell: None,
            args: Vec::new(),
            cwd: None,
            env: Vec::new(),
            size: PtyWindowSize::default(),
            buffer_capacity: DEFAULT_BUFFER_CAPACITY,
        }
    }
}

/// 终端窗口尺寸。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PtyWindowSize {
    pub rows: u16,
    pub cols: u16,
    pub pixel_width: u16,
    pub pixel_height: u16,
}

impl Default for PtyWindowSize {
    fn default() -> Self {
        Self {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        }
    }
}

impl From<PtyWindowSize> for PtySize {
    fn from(value: PtyWindowSize) -> Self {
        PtySize {
            rows: value.rows,
            cols: value.cols,
            pixel_width: value.pixel_width,
            pixel_height: value.pixel_height,
        }
    }
}

impl From<PtySize> for PtyWindowSize {
    fn from(value: PtySize) -> Self {
        Self {
            rows: value.rows,
            cols: value.cols,
            pixel_width: value.pixel_width,
            pixel_height: value.pixel_height,
        }
    }
}

/// 会话运行时状态。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PtySessionState {
    Running,
    Exited,
    Killed,
}

/// 可观察的会话事件。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PtyEvent {
    /// 输出增量；`cursor_end` 为该块写入后的绝对游标。
    Output {
        data: Vec<u8>,
        cursor_end: OutputCursor,
    },
    /// 子进程退出。
    Exit {
        code: Option<i32>,
        signal: Option<String>,
    },
}

/// 重连快照：当前窗口、缓冲区间与数据、退出态。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PtySnapshot {
    pub terminal_id: TerminalId,
    pub owner_session: OwnerSessionId,
    pub state: PtySessionState,
    pub size: PtyWindowSize,
    pub buffer_start: OutputCursor,
    pub buffer_end: OutputCursor,
    pub buffered: Vec<u8>,
    pub exit_code: Option<i32>,
    pub exit_signal: Option<String>,
    /// 实时广播因容量满被覆写丢弃的事件数；重连消费者可据此感知实时流缺失。
    pub dropped_events: u64,
}

/// 增量读取结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PtyOutputChunk {
    pub from: OutputCursor,
    pub to: OutputCursor,
    pub data: Vec<u8>,
}

#[derive(Debug, Error)]
pub enum PtyError {
    #[error("pty session not found: {0}")]
    NotFound(TerminalId),
    #[error("pty session {0} is not owned by {1}")]
    Ownership(TerminalId, OwnerSessionId),
    #[error("pty session {0} is already closed")]
    Closed(TerminalId),
    #[error("output cursor is stale (requested {requested}, available from {available_from})")]
    StaleCursor {
        requested: OutputCursor,
        available_from: OutputCursor,
    },
    #[error("output cursor is in the future (requested {requested}, end {end})")]
    FutureCursor {
        requested: OutputCursor,
        end: OutputCursor,
    },
    #[error("failed to create pty: {0}")]
    Create(String),
    #[error("failed to spawn shell: {0}")]
    Spawn(String),
    #[error("failed to attach pty process tree: {0}")]
    ProcessTree(String),
    #[error("pty io error: {0}")]
    Io(String),
    #[error("pty service is shutting down")]
    ShuttingDown,
}

/// PTY Service：多会话管理器。
pub struct PtyService {
    inner: Arc<ServiceInner>,
}

struct ServiceInner {
    sessions: Mutex<HashMap<TerminalId, Arc<SessionInner>>>,
    next_id: AtomicU64,
    shutting_down: Mutex<bool>,
}

struct SessionInner {
    id: TerminalId,
    owner: OwnerSessionId,
    state: Mutex<SessionRuntime>,
    buffer: Mutex<RingBuffer>,
    size: Mutex<PtyWindowSize>,
    master: Mutex<Option<Box<dyn MasterPty + Send>>>,
    writer: Mutex<Option<Box<dyn Write + Send>>>,
    killer: Mutex<Option<Box<dyn portable_pty::ChildKiller + Send + Sync>>>,
    process_tree: Mutex<Option<ProcessTreeGuard>>,
    events: broadcast::Sender<PtyEvent>,
    /// 实时广播通道容量（事件条数），用于检测满队列覆写。
    event_capacity: usize,
    /// 实时广播因容量满被覆写丢弃的事件数（慢消费者可见性）。
    dropped_events: AtomicU64,
    /// 关闭通知：reader / waiter 退出后触发。
    closed: Notify,
    /// 防止重复 cleanup。
    cleaned: Mutex<bool>,
}

struct SessionRuntime {
    state: PtySessionState,
    /// waiter 已完成 `child.wait()`，可安全认为子进程已回收。
    finished: bool,
    exit_code: Option<i32>,
    exit_signal: Option<String>,
    process_id: Option<u32>,
}

impl Default for SessionRuntime {
    fn default() -> Self {
        Self {
            state: PtySessionState::Running,
            finished: false,
            exit_code: None,
            exit_signal: None,
            process_id: None,
        }
    }
}

impl PtyService {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(ServiceInner {
                sessions: Mutex::new(HashMap::new()),
                next_id: AtomicU64::new(1),
                shutting_down: Mutex::new(false),
            }),
        }
    }

    /// 创建并启动一个 PTY 会话。blocking 的 openpty/spawn 在 `spawn_blocking` 中执行。
    pub async fn create(&self, spec: PtyCreateSpec) -> Result<TerminalId, PtyError> {
        if *self.inner.shutting_down.lock() {
            return Err(PtyError::ShuttingDown);
        }

        let id_num = self.inner.next_id.fetch_add(1, Ordering::Relaxed);
        let terminal_id = TerminalId::new(format!("pty-{id_num}"));
        let terminal_id_for_spawn = terminal_id.clone();
        let owner = spec.owner_session.clone();
        let buffer_capacity = spec.buffer_capacity.max(1);
        let size = spec.size;

        let spawn_result = tokio::task::spawn_blocking(move || open_and_spawn(spec))
            .await
            .map_err(|e| PtyError::Create(format!("join error: {e}")))?;

        let OpenedPty {
            master,
            writer,
            killer,
            process_tree,
            mut reader,
            process_id,
            mut child,
        } = spawn_result?;

        let (events, _) = broadcast::channel(DEFAULT_EVENT_CAPACITY);
        let session = Arc::new(SessionInner {
            id: terminal_id.clone(),
            owner: owner.clone(),
            state: Mutex::new(SessionRuntime {
                state: PtySessionState::Running,
                finished: false,
                exit_code: None,
                exit_signal: None,
                process_id,
            }),
            buffer: Mutex::new(RingBuffer::new(buffer_capacity)),
            size: Mutex::new(size),
            master: Mutex::new(Some(master)),
            writer: Mutex::new(Some(writer)),
            killer: Mutex::new(Some(killer)),
            process_tree: Mutex::new(process_tree),
            events: events.clone(),
            event_capacity: DEFAULT_EVENT_CAPACITY,
            dropped_events: AtomicU64::new(0),
            closed: Notify::new(),
            cleaned: Mutex::new(false),
        });

        {
            let mut guard = self.inner.sessions.lock();
            if *self.inner.shutting_down.lock() {
                // 竞态：创建中途关停 → 立即清理。
                drop(guard);
                session.force_kill_blocking();
                return Err(PtyError::ShuttingDown);
            }
            guard.insert(terminal_id.clone(), Arc::clone(&session));
        }

        // 输出读取线程（OS thread，避免阻塞 runtime）。
        let reader_session = Arc::clone(&session);
        let reader_thread = thread::Builder::new()
            .name(format!("pty-reader-{}", terminal_id_for_spawn))
            .spawn(move || {
                let mut buf = [0u8; 8192];
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => {
                            let chunk = buf[..n].to_vec();
                            let cursor_end = {
                                let mut ring = reader_session.buffer.lock();
                                ring.push(&chunk);
                                ring.end()
                            };
                            reader_session.send_event(PtyEvent::Output {
                                data: chunk,
                                cursor_end,
                            });
                        }
                        Err(err) => {
                            debug!(error = %err, "pty reader ended");
                            break;
                        }
                    }
                }
            });
        if let Err(error) = reader_thread {
            session.force_kill_blocking();
            self.inner.sessions.lock().remove(&terminal_id);
            return Err(PtyError::Create(format!("spawn reader thread: {error}")));
        }

        // 等待退出线程。
        let waiter_session = Arc::clone(&session);
        let waiter_thread = thread::Builder::new()
            .name(format!("pty-waiter-{}", terminal_id_for_spawn))
            .spawn(move || {
                let status = child.wait();
                match status {
                    Ok(status) => {
                        let code = Some(status.exit_code() as i32);
                        let signal = status_signal(&status);
                        {
                            let mut runtime = waiter_session.state.lock();
                            if runtime.state == PtySessionState::Running {
                                runtime.state = PtySessionState::Exited;
                            }
                            runtime.finished = true;
                            runtime.exit_code = code;
                            runtime.exit_signal = signal.clone();
                        }
                        waiter_session.send_event(PtyEvent::Exit { code, signal });
                    }
                    Err(err) => {
                        warn!(error = %err, "pty wait failed");
                        {
                            let mut runtime = waiter_session.state.lock();
                            if runtime.state == PtySessionState::Running {
                                runtime.state = PtySessionState::Exited;
                            }
                            runtime.finished = true;
                            runtime.exit_code = None;
                            runtime.exit_signal = Some(format!("wait error: {err}"));
                        }
                        waiter_session.send_event(PtyEvent::Exit {
                            code: None,
                            signal: Some(err.to_string()),
                        });
                    }
                }
                // 幂等清理资源；会话条目保留以便重连读取缓冲 / exit 状态，
                // 由显式 cleanup / shutdown / owner detach 移除。
                waiter_session.cleanup_handles();
                waiter_session.closed.notify_waiters();
            });
        if let Err(error) = waiter_thread {
            session.force_kill_blocking();
            self.inner.sessions.lock().remove(&terminal_id);
            return Err(PtyError::Create(format!("spawn waiter thread: {error}")));
        }

        Ok(terminal_id)
    }

    /// 调整窗口尺寸。
    pub async fn resize(
        &self,
        terminal_id: &TerminalId,
        owner: &OwnerSessionId,
        size: PtyWindowSize,
    ) -> Result<(), PtyError> {
        let session = self.require_owned(terminal_id, owner)?;
        let terminal_id = terminal_id.clone();
        tokio::task::spawn_blocking(move || {
            session.ensure_running()?;
            let guard = session.master.lock();
            let master = guard
                .as_ref()
                .ok_or_else(|| PtyError::Closed(terminal_id.clone()))?;
            master
                .resize(size.into())
                .map_err(|e| PtyError::Io(e.to_string()))?;
            *session.size.lock() = size;
            Ok(())
        })
        .await
        .map_err(|e| PtyError::Io(format!("join error: {e}")))?
    }

    /// 向 PTY 写入 stdin 数据。blocking write 放到 spawn_blocking。
    pub async fn write(
        &self,
        terminal_id: &TerminalId,
        owner: &OwnerSessionId,
        data: Vec<u8>,
    ) -> Result<(), PtyError> {
        let session = self.require_owned(terminal_id, owner)?;
        session.ensure_running()?;
        let session_for_write = Arc::clone(&session);
        let terminal_id = terminal_id.clone();
        tokio::task::spawn_blocking(move || {
            let mut guard = session_for_write.writer.lock();
            let writer = guard
                .as_mut()
                .ok_or_else(|| PtyError::Closed(terminal_id.clone()))?;
            writer
                .write_all(&data)
                .map_err(|e| PtyError::Io(e.to_string()))?;
            writer.flush().map_err(|e| PtyError::Io(e.to_string()))?;
            Ok(())
        })
        .await
        .map_err(|e| PtyError::Io(format!("join error: {e}")))?
    }

    /// 订阅实时事件（输出 / 退出）。
    pub fn subscribe(
        &self,
        terminal_id: &TerminalId,
        owner: &OwnerSessionId,
    ) -> Result<broadcast::Receiver<PtyEvent>, PtyError> {
        let session = self.require_owned(terminal_id, owner)?;
        Ok(session.events.subscribe())
    }

    /// 重连快照：窗口、缓冲、状态。
    pub fn snapshot(
        &self,
        terminal_id: &TerminalId,
        owner: &OwnerSessionId,
    ) -> Result<PtySnapshot, PtyError> {
        let session = self.require_owned(terminal_id, owner)?;
        let runtime = session.state.lock().clone_fields();
        let size = *session.size.lock();
        let (buffer_start, buffer_end, buffered) = session.buffer.lock().snapshot();
        Ok(PtySnapshot {
            terminal_id: session.id.clone(),
            owner_session: session.owner.clone(),
            state: runtime.state,
            size,
            buffer_start,
            buffer_end,
            buffered,
            exit_code: runtime.exit_code,
            exit_signal: runtime.exit_signal,
            dropped_events: session.dropped_events.load(Ordering::Relaxed),
        })
    }

    /// 按游标读取增量输出（重连后续读）。
    pub fn read_output(
        &self,
        terminal_id: &TerminalId,
        owner: &OwnerSessionId,
        cursor: OutputCursor,
    ) -> Result<PtyOutputChunk, PtyError> {
        let session = self.require_owned(terminal_id, owner)?;
        let result = session.buffer.lock().read_since(cursor);
        match result {
            Ok((from, to, data)) => Ok(PtyOutputChunk { from, to, data }),
            Err(RingReadError::Stale {
                requested,
                available_from,
            }) => Err(PtyError::StaleCursor {
                requested,
                available_from,
            }),
            Err(RingReadError::Future { requested, end }) => {
                Err(PtyError::FutureCursor { requested, end })
            }
        }
    }

    /// 当前会话状态。
    pub fn state(
        &self,
        terminal_id: &TerminalId,
        owner: &OwnerSessionId,
    ) -> Result<PtySessionState, PtyError> {
        let session = self.require_owned(terminal_id, owner)?;
        let state = session.state.lock().state;
        Ok(state)
    }

    /// 列出归属某 owner 的终端会话。
    pub fn list_for_owner(&self, owner: &OwnerSessionId) -> Vec<TerminalId> {
        self.inner
            .sessions
            .lock()
            .iter()
            .filter(|(_, s)| &s.owner == owner)
            .map(|(id, _)| id.clone())
            .collect()
    }

    /// 强制终止会话（幂等）。
    pub async fn kill(
        &self,
        terminal_id: &TerminalId,
        owner: &OwnerSessionId,
    ) -> Result<(), PtyError> {
        let session = self.require_owned(terminal_id, owner)?;
        session.kill_async().await
    }

    /// 等待退出（已退出则立即返回）。
    pub async fn wait_exit(
        &self,
        terminal_id: &TerminalId,
        owner: &OwnerSessionId,
    ) -> Result<(Option<i32>, Option<String>), PtyError> {
        let session = self.require_owned(terminal_id, owner)?;
        session.wait_finished().await;
        let runtime = session.state.lock();
        Ok((runtime.exit_code, runtime.exit_signal.clone()))
    }

    /// 幂等清理并移除会话条目。对已清理会话再次调用返回 Ok。
    pub async fn cleanup(
        &self,
        terminal_id: &TerminalId,
        owner: &OwnerSessionId,
    ) -> Result<(), PtyError> {
        let session = {
            let guard = self.inner.sessions.lock();
            match guard.get(terminal_id) {
                Some(s) if &s.owner == owner => Some(Arc::clone(s)),
                Some(_) => return Err(PtyError::Ownership(terminal_id.clone(), owner.clone())),
                None => None,
            }
        };
        if let Some(session) = session {
            session.kill_async().await?;
            if tokio::time::timeout(CLEANUP_GRACE, session.wait_finished())
                .await
                .is_err()
            {
                warn!(terminal_id = %terminal_id, "pty cleanup timed out waiting for child reap");
            }
            self.inner.sessions.lock().remove(terminal_id);
        }
        Ok(())
    }

    /// 清理某 owner 的全部会话（幂等）。
    pub async fn cleanup_owner(&self, owner: &OwnerSessionId) -> Result<usize, PtyError> {
        let ids: Vec<TerminalId> = self.list_for_owner(owner);
        let count = ids.len();
        for id in ids {
            self.cleanup(&id, owner).await?;
        }
        Ok(count)
    }

    /// 关停服务：终止并移除全部会话（幂等）。
    pub async fn shutdown(&self) -> Result<(), PtyError> {
        *self.inner.shutting_down.lock() = true;
        let sessions: Vec<Arc<SessionInner>> =
            self.inner.sessions.lock().values().cloned().collect();
        for session in sessions {
            let _ = session.kill_async().await;
        }
        let sessions: Vec<Arc<SessionInner>> =
            self.inner.sessions.lock().values().cloned().collect();
        if tokio::time::timeout(CLEANUP_GRACE, async {
            for session in &sessions {
                session.wait_finished().await;
            }
        })
        .await
        .is_err()
        {
            warn!("pty shutdown timed out waiting for child reap");
        }
        self.inner.sessions.lock().clear();
        Ok(())
    }

    /// 当前活跃会话数（含已退出但未 cleanup 的，便于重连）。
    pub fn session_count(&self) -> usize {
        self.inner.sessions.lock().len()
    }

    fn require_owned(
        &self,
        terminal_id: &TerminalId,
        owner: &OwnerSessionId,
    ) -> Result<Arc<SessionInner>, PtyError> {
        let guard = self.inner.sessions.lock();
        match guard.get(terminal_id) {
            Some(session) if &session.owner == owner => Ok(Arc::clone(session)),
            Some(_) => Err(PtyError::Ownership(terminal_id.clone(), owner.clone())),
            None => Err(PtyError::NotFound(terminal_id.clone())),
        }
    }
}

impl Default for PtyService {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionInner {
    fn ensure_running(&self) -> Result<(), PtyError> {
        let state = self.state.lock().state;
        if state == PtySessionState::Running {
            Ok(())
        } else {
            Err(PtyError::Closed(self.id.clone()))
        }
    }

    /// 发送实时事件。tokio 1.53 broadcast 满时静默覆写最旧未读事件
    /// （发送方无错误信号），因此在发送前检测队列已满：本次发送必然
    /// 使一个事件对慢消费者不可达，递增 `dropped_events` 使丢弃事实
    /// 可被快照观察。
    fn send_event(&self, event: PtyEvent) {
        if self.events.receiver_count() == 0 {
            return;
        }
        if self.events.len() >= self.event_capacity {
            self.dropped_events.fetch_add(1, Ordering::Relaxed);
        }
        let _ = self.events.send(event);
    }

    fn cleanup_handles(&self) {
        let mut cleaned = self.cleaned.lock();
        if *cleaned {
            return;
        }
        *cleaned = true;
        drop(cleaned);
        if let Some(tree) = self.process_tree.lock().take() {
            let _ = tree.terminate();
        }
        *self.writer.lock() = None;
        *self.master.lock() = None;
        *self.killer.lock() = None;
    }

    fn force_kill_blocking(&self) {
        if let Some(tree) = self.process_tree.lock().as_ref() {
            let _ = tree.terminate();
        }
        if let Some(mut killer) = self.killer.lock().take() {
            let _ = killer.kill();
        }
        self.cleanup_handles();
    }

    async fn wait_finished(&self) {
        loop {
            // 先创建通知 future，再检查谓词，避免 waiter 在两步之间通知导致永久等待。
            let notified = self.closed.notified();
            if self.state.lock().finished {
                return;
            }
            notified.await;
        }
    }

    async fn kill_async(self: &Arc<Self>) -> Result<(), PtyError> {
        {
            let mut runtime = self.state.lock();
            if runtime.state != PtySessionState::Running {
                // 已退出：仍做幂等 handle 清理。
                drop(runtime);
                self.cleanup_handles();
                return Ok(());
            }
            runtime.state = PtySessionState::Killed;
        }

        let session = Arc::clone(self);
        tokio::task::spawn_blocking(move || {
            if let Some(tree) = session.process_tree.lock().as_ref() {
                let _ = tree.terminate();
            }
            if let Some(mut killer) = session.killer.lock().take() {
                let _ = killer.kill();
            }
            // 丢弃 writer 向 slave 发送 EOF，帮助 shell 退出。
            *session.writer.lock() = None;
        })
        .await
        .map_err(|e| PtyError::Io(format!("join error: {e}")))?;

        Ok(())
    }
}

impl SessionRuntime {
    fn clone_fields(&self) -> SessionRuntime {
        SessionRuntime {
            state: self.state,
            finished: self.finished,
            exit_code: self.exit_code,
            exit_signal: self.exit_signal.clone(),
            process_id: self.process_id,
        }
    }
}

struct OpenedPty {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    killer: Box<dyn portable_pty::ChildKiller + Send + Sync>,
    process_tree: Option<ProcessTreeGuard>,
    reader: Box<dyn Read + Send>,
    process_id: Option<u32>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
}

fn open_and_spawn(spec: PtyCreateSpec) -> Result<OpenedPty, PtyError> {
    let _spawn_guard = PTY_SPAWN_LOCK.lock();
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(spec.size.into())
        .map_err(|e| PtyError::Create(e.to_string()))?;

    let mut cmd = build_command(&spec);
    if let Some(cwd) = &spec.cwd {
        cmd.cwd(cwd);
    }
    for (k, v) in &spec.env {
        cmd.env(k, v);
    }

    let reader = pair
        .master
        .try_clone_reader()
        .map_err(|e| PtyError::Create(e.to_string()))?;
    let writer = pair
        .master
        .take_writer()
        .map_err(|e| PtyError::Create(e.to_string()))?;
    let mut child = pair
        .slave
        .spawn_command(cmd)
        .map_err(|e| PtyError::Spawn(e.to_string()))?;
    // Drop slave：只保留 master 控制端。
    drop(pair.slave);

    let mut killer = child.clone_killer();
    let process_id = match child.process_id() {
        Some(process_id) => process_id,
        None => {
            let _ = killer.kill();
            let _ = child.wait();
            return Err(PtyError::ProcessTree(
                "PTY child did not expose a process id; refusing an unguarded session".into(),
            ));
        }
    };
    let process_tree = match ProcessTreeGuard::attach_external(process_id, ProcessLimits::default())
    {
        Ok(tree) => tree,
        Err(error) => {
            let _ = killer.kill();
            let _ = child.wait();
            return Err(PtyError::ProcessTree(error.to_string()));
        }
    };

    Ok(OpenedPty {
        master: pair.master,
        writer,
        killer,
        process_tree: Some(process_tree),
        reader,
        process_id: Some(process_id),
        child,
    })
}

fn build_command(spec: &PtyCreateSpec) -> CommandBuilder {
    if let Some(shell) = &spec.shell {
        let mut cmd = CommandBuilder::new(shell);
        for arg in &spec.args {
            cmd.arg(arg);
        }
        return cmd;
    }

    // 平台默认 shell。
    #[cfg(windows)]
    {
        let mut cmd = CommandBuilder::new("cmd.exe");
        if spec.args.is_empty() {
            // 保持交互；测试场景会显式传 /C。
        } else {
            for arg in &spec.args {
                cmd.arg(arg);
            }
        }
        cmd
    }
    #[cfg(not(windows))]
    {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
        let mut cmd = CommandBuilder::new(shell);
        if !spec.args.is_empty() {
            for arg in &spec.args {
                cmd.arg(arg);
            }
        }
        cmd
    }
}

fn status_signal(status: &portable_pty::ExitStatus) -> Option<String> {
    // portable-pty 0.8 不公开 signal 字段；通过 Display 兜底提取。
    let text = status.to_string();
    if text.starts_with("Terminated by ") {
        Some(text.trim_start_matches("Terminated by ").to_string())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn owner(name: &str) -> OwnerSessionId {
        OwnerSessionId::new(name)
    }

    fn echo_spec(owner_session: OwnerSessionId) -> PtyCreateSpec {
        #[cfg(windows)]
        {
            PtyCreateSpec {
                owner_session,
                shell: Some("cmd.exe".into()),
                args: vec!["/C".into(), "echo hello-pty".into()],
                cwd: None,
                env: Vec::new(),
                size: PtyWindowSize::default(),
                buffer_capacity: 4096,
            }
        }
        #[cfg(not(windows))]
        {
            PtyCreateSpec {
                owner_session,
                shell: Some("/bin/sh".into()),
                args: vec!["-c".into(), "printf 'hello-pty\\n'".into()],
                cwd: None,
                env: Vec::new(),
                size: PtyWindowSize::default(),
                buffer_capacity: 4096,
            }
        }
    }

    fn sleep_spec(owner_session: OwnerSessionId) -> PtyCreateSpec {
        #[cfg(windows)]
        {
            PtyCreateSpec {
                owner_session,
                shell: Some("cmd.exe".into()),
                // ping 自环约 1 秒/次，便于 kill 测试。
                args: vec!["/C".into(), "ping -n 30 127.0.0.1 >NUL".into()],
                cwd: None,
                env: Vec::new(),
                size: PtyWindowSize::default(),
                buffer_capacity: 4096,
            }
        }
        #[cfg(not(windows))]
        {
            PtyCreateSpec {
                owner_session,
                shell: Some("/bin/sh".into()),
                args: vec!["-c".into(), "sleep 30".into()],
                cwd: None,
                env: Vec::new(),
                size: PtyWindowSize::default(),
                buffer_capacity: 4096,
            }
        }
    }

    fn process_tree_spec(owner_session: OwnerSessionId) -> PtyCreateSpec {
        #[cfg(windows)]
        {
            PtyCreateSpec {
                owner_session,
                shell: Some("powershell.exe".into()),
                args: vec![
                    "-NoProfile".into(),
                    "-NonInteractive".into(),
                    "-Command".into(),
                    "$p=Start-Process -FilePath powershell.exe -ArgumentList '-NoProfile','-NonInteractive','-Command','Start-Sleep -Seconds 30' -PassThru; Write-Output \"CHILD=$($p.Id)\"; Wait-Process -Id $p.Id".into(),
                ],
                cwd: None,
                env: Vec::new(),
                size: PtyWindowSize::default(),
                buffer_capacity: 4096,
            }
        }
        #[cfg(not(windows))]
        {
            PtyCreateSpec {
                owner_session,
                shell: Some("/bin/sh".into()),
                args: vec![
                    "-c".into(),
                    "sleep 30 & child=$!; echo CHILD=$child; wait $child".into(),
                ],
                cwd: None,
                env: Vec::new(),
                size: PtyWindowSize::default(),
                buffer_capacity: 4096,
            }
        }
    }

    /// 一次性倾倒远超广播容量（256 事件）的输出，用于验证丢弃可观测性。
    fn flood_spec(owner_session: OwnerSessionId) -> PtyCreateSpec {
        #[cfg(windows)]
        {
            PtyCreateSpec {
                owner_session,
                shell: Some("powershell.exe".into()),
                args: vec![
                    "-NoProfile".into(),
                    "-NonInteractive".into(),
                    "-Command".into(),
                    "[Console]::Out.Write(('x'*8192)*2048)".into(),
                ],
                cwd: None,
                env: Vec::new(),
                size: PtyWindowSize::default(),
                buffer_capacity: DEFAULT_BUFFER_CAPACITY,
            }
        }
        #[cfg(not(windows))]
        {
            PtyCreateSpec {
                owner_session,
                shell: Some("/bin/sh".into()),
                args: vec![
                    "-c".into(),
                    "dd if=/dev/zero bs=4096 count=2048 2>/dev/null".into(),
                ],
                cwd: None,
                env: Vec::new(),
                size: PtyWindowSize::default(),
                buffer_capacity: DEFAULT_BUFFER_CAPACITY,
            }
        }
    }

    async fn wait_for_output(
        service: &PtyService,
        id: &TerminalId,
        owner: &OwnerSessionId,
    ) -> String {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            let snap = service.snapshot(id, owner).expect("snapshot");
            let text = String::from_utf8_lossy(&snap.buffered).to_string();
            if text.contains("hello-pty") || snap.state != PtySessionState::Running {
                return text;
            }
            if tokio::time::Instant::now() > deadline {
                return text;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    #[tokio::test]
    async fn create_runs_command_and_captures_output() {
        let service = PtyService::new();
        let owner = owner("s1");
        let id = service
            .create(echo_spec(owner.clone()))
            .await
            .expect("create");
        let text = wait_for_output(&service, &id, &owner).await;
        assert!(
            text.contains("hello-pty"),
            "expected hello-pty in output, got {text:?}"
        );
        let (code, _) = service.wait_exit(&id, &owner).await.expect("wait");
        // Windows cmd /C echo 通常 exit 0。
        assert!(code.is_some());
        service.cleanup(&id, &owner).await.expect("cleanup");
        assert_eq!(service.session_count(), 0);
    }

    #[tokio::test]
    async fn ownership_is_enforced() {
        let service = PtyService::new();
        let owner_a = owner("owner-a");
        let other = owner("owner-b");
        let id = service
            .create(echo_spec(owner_a.clone()))
            .await
            .expect("create");
        let err = service
            .write(&id, &other, b"x".to_vec())
            .await
            .expect_err("foreign write");
        assert!(matches!(err, PtyError::Ownership(_, _)));
        let err = service.snapshot(&id, &other).expect_err("foreign snap");
        assert!(matches!(err, PtyError::Ownership(_, _)));
        service.cleanup(&id, &owner_a).await.expect("cleanup");
    }

    #[tokio::test]
    async fn reconnect_snapshot_and_cursor_resume() {
        let service = PtyService::new();
        let owner = owner("reconnect");
        let id = service
            .create(echo_spec(owner.clone()))
            .await
            .expect("create");
        let _ = wait_for_output(&service, &id, &owner).await;
        let snap = service.snapshot(&id, &owner).expect("snapshot");
        assert!(!snap.buffered.is_empty());
        assert_eq!(
            snap.buffer_end,
            snap.buffer_start + snap.buffered.len() as u64
        );

        // 从 0 续读（缓冲未丢弃时应成功）。
        let chunk = service.read_output(&id, &owner, 0).expect("read from 0");
        assert_eq!(chunk.data, snap.buffered);
        assert_eq!(chunk.to, snap.buffer_end);

        // 从 end 续读为空。
        let empty = service
            .read_output(&id, &owner, snap.buffer_end)
            .expect("read end");
        assert!(empty.data.is_empty());

        service.cleanup(&id, &owner).await.expect("cleanup");
    }

    #[tokio::test]
    async fn broadcast_overflow_is_observable_via_snapshot() {
        let service = PtyService::new();
        let owner = owner("dropped-observe");
        let id = service
            .create(flood_spec(owner.clone()))
            .await
            .expect("create");
        // 慢消费者：订阅但从不读取，使 broadcast 持续满并覆写旧事件。
        let _slow = service.subscribe(&id, &owner).expect("subscribe");

        let (_code, _) = service.wait_exit(&id, &owner).await.expect("wait");
        // reader 线程可能在子进程退出后仍排空管道残留，轮询至计数稳定。
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        let mut last = 0u64;
        let mut stable_rounds = 0u32;
        loop {
            let snap = service.snapshot(&id, &owner).expect("snapshot");
            if snap.dropped_events == last {
                stable_rounds += 1;
                if stable_rounds >= 3 {
                    assert!(
                        snap.dropped_events > 0,
                        "expected broadcast drops, got {}",
                        snap.dropped_events
                    );
                    break;
                }
            } else {
                last = snap.dropped_events;
                stable_rounds = 0;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "dropped counter never stabilized at {last}"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        service.cleanup(&id, &owner).await.expect("cleanup");
    }

    #[tokio::test]
    async fn kill_is_idempotent_and_stops_session() {
        let service = PtyService::new();
        let owner = owner("kill");
        let id = service
            .create(sleep_spec(owner.clone()))
            .await
            .expect("create");
        service.kill(&id, &owner).await.expect("kill");
        service.kill(&id, &owner).await.expect("kill again");
        let (code, signal) = service.wait_exit(&id, &owner).await.expect("wait");
        // 被 kill 后状态为 Killed 或 Exited。
        let state = service.state(&id, &owner).expect("state");
        assert!(
            matches!(state, PtySessionState::Killed | PtySessionState::Exited),
            "state={state:?} code={code:?} signal={signal:?}"
        );
        service.cleanup(&id, &owner).await.expect("cleanup");
        // 幂等 cleanup
        service.cleanup(&id, &owner).await.expect("cleanup again");
        assert_eq!(service.session_count(), 0);
    }

    #[tokio::test]
    async fn multi_session_and_owner_cleanup() {
        let service = PtyService::new();
        let owner_a = owner("a");
        let owner_b = owner("b");
        let a1 = service
            .create(echo_spec(owner_a.clone()))
            .await
            .expect("a1");
        let a2 = service
            .create(echo_spec(owner_a.clone()))
            .await
            .expect("a2");
        let b1 = service
            .create(echo_spec(owner_b.clone()))
            .await
            .expect("b1");
        assert_eq!(service.list_for_owner(&owner_a).len(), 2);
        assert_eq!(service.session_count(), 3);

        let cleaned = service.cleanup_owner(&owner_a).await.expect("cleanup a");
        assert_eq!(cleaned, 2);
        assert!(service.list_for_owner(&owner_a).is_empty());
        assert_eq!(service.session_count(), 1);
        assert!(service.list_for_owner(&owner_b).contains(&b1));

        service.cleanup(&b1, &owner_b).await.expect("cleanup b");
        // 确保 a1/a2 引用不悬空
        let _ = (a1, a2);
    }

    #[tokio::test]
    async fn resize_updates_window_size() {
        let service = PtyService::new();
        let owner = owner("resize");
        let id = service
            .create(sleep_spec(owner.clone()))
            .await
            .expect("create");
        let new_size = PtyWindowSize {
            rows: 40,
            cols: 120,
            pixel_width: 0,
            pixel_height: 0,
        };
        service.resize(&id, &owner, new_size).await.expect("resize");
        let snap = service.snapshot(&id, &owner).expect("snapshot");
        assert_eq!(snap.size, new_size);
        service.cleanup(&id, &owner).await.expect("cleanup");
    }

    #[tokio::test]
    async fn shutdown_clears_all_sessions() {
        let service = PtyService::new();
        let owner = owner("shutdown");
        let _ = service
            .create(sleep_spec(owner.clone()))
            .await
            .expect("create");
        let _ = service
            .create(echo_spec(owner.clone()))
            .await
            .expect("create2");
        service.shutdown().await.expect("shutdown");
        assert_eq!(service.session_count(), 0);
        // 幂等
        service.shutdown().await.expect("shutdown again");
        let err = service
            .create(echo_spec(owner))
            .await
            .expect_err("create after shutdown");
        assert!(matches!(err, PtyError::ShuttingDown));
    }

    #[tokio::test]
    async fn cleanup_reaps_descendant_process_tree() {
        let service = PtyService::new();
        let owner = owner("tree-cleanup");
        let id = service
            .create(process_tree_spec(owner.clone()))
            .await
            .expect("create tree");
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        let descendant = loop {
            let snapshot = service.snapshot(&id, &owner).expect("snapshot");
            let text = String::from_utf8_lossy(&snapshot.buffered);
            if let Some(pid) = text.split("CHILD=").nth(1).and_then(|suffix| {
                let digits = suffix
                    .chars()
                    .skip_while(|ch| !ch.is_ascii_digit())
                    .take_while(char::is_ascii_digit)
                    .collect::<String>();
                digits.parse::<u32>().ok()
            }) {
                break pid;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "missing child pid: {text:?}"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        };
        assert!(process_exists(descendant), "descendant should be alive");

        service.cleanup(&id, &owner).await.expect("cleanup tree");
        tokio::time::timeout(Duration::from_secs(3), async {
            while process_exists(descendant) {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .expect("descendant survived PTY cleanup");
    }

    #[cfg(unix)]
    fn process_exists(pid: u32) -> bool {
        let Ok(pid) = i32::try_from(pid) else {
            return false;
        };
        let result = unsafe { libc::kill(pid, 0) };
        result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }

    #[cfg(windows)]
    fn process_exists(pid: u32) -> bool {
        use windows::Win32::Foundation::{CloseHandle, STILL_ACTIVE};
        use windows::Win32::System::Threading::{
            GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
        };

        let Ok(handle) = (unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) })
        else {
            return false;
        };
        let mut exit_code = 0u32;
        let running = unsafe { GetExitCodeProcess(handle, &mut exit_code) }.is_ok()
            && exit_code == STILL_ACTIVE.0 as u32;
        let _ = unsafe { CloseHandle(handle) };
        running
    }

    #[test]
    fn ring_buffer_unit_is_wired() {
        let mut ring = RingBuffer::new(3);
        ring.push(b"abcd");
        assert_eq!(ring.snapshot().2, b"bcd");
    }

    #[test]
    fn broadcast_overflow_increments_dropped_events() {
        let (tx, mut rx) = broadcast::channel(4);
        let session = SessionInner {
            id: TerminalId::new("pty-dropped"),
            owner: owner("dropped-unit"),
            state: Mutex::new(SessionRuntime::default()),
            buffer: Mutex::new(RingBuffer::new(1024)),
            size: Mutex::new(PtyWindowSize::default()),
            master: Mutex::new(None),
            writer: Mutex::new(None),
            killer: Mutex::new(None),
            process_tree: Mutex::new(None),
            events: tx,
            event_capacity: 4,
            dropped_events: AtomicU64::new(0),
            closed: Notify::new(),
            cleaned: Mutex::new(false),
        };
        for i in 0..10u64 {
            session.send_event(PtyEvent::Output {
                data: vec![i as u8],
                cursor_end: i,
            });
        }
        // 容量 4，发送 10 → 6 个最旧事件被覆写丢弃。
        assert_eq!(session.dropped_events.load(Ordering::Relaxed), 6);
        // 慢消费者报告落后 6 条，随后仍可收到最新事件（覆写语义）。
        assert!(matches!(
            rx.try_recv(),
            Err(tokio::sync::broadcast::error::TryRecvError::Lagged(6))
        ));
        assert!(matches!(
            rx.try_recv(),
            Ok(PtyEvent::Output { cursor_end: 6, .. })
        ));
    }
}
