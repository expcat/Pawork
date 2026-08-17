//! 后台任务管理器：统一注册 / 状态机 / 事件广播 / 进程类任务接线。
//!
//! 默认构建是纯状态机，不持有 `SandboxBackend`。进程执行所有权（硬约束）：
//! process 类任务只在 `process-exec` 档经 `with_backend` 注入的
//! `SandboxBackend` → `ProcessRuntime` 执行，本模块只编排事件转发与状态机，
//! 不直接启动子进程、不自行清理进程树、不自定义 sandbox policy。

use std::sync::{Arc, Mutex, MutexGuard};

use pawork_domain::{BackgroundTaskId, CancellationToken, TaskEvent, TaskKind, TaskStatus};
use pawork_domain::AgentEvent;
#[cfg(feature = "process-exec")]
use pawork_exec::{SandboxBackend, SandboxPolicy, SandboxProcess, SandboxProcessSpec};
use tokio::sync::broadcast;

use crate::task::error::TaskManagerError;
use crate::task::state::{TaskManagerSnapshot, TaskManagerState, TaskSnapshot};
#[cfg(feature = "process-exec")]
use crate::task::state::{is_terminal_status, OutputEvent};

/// 实时事件广播默认容量；超出的订阅者会收到 `RecvError::Lagged`，
/// 此时应通过 `snapshot()` + `events_since()` 重连恢复。
const DEFAULT_BROADCAST_CAPACITY: usize = 256;

struct TaskManagerInner {
    state: Mutex<TaskManagerState>,
    #[cfg(feature = "process-exec")]
    backend: Option<Arc<dyn SandboxBackend>>,
    live: broadcast::Sender<AgentEvent>,
}

/// 统一后台任务管理器（process / agent / monitor / automation）。
///
/// 任务运行与连接解耦：任务表 + 事件日志常驻进程内，`snapshot()` /
/// `replay()` 恢复任务视图，`events_since` 续读增量；CLI/GUI 断连不影响
/// 任务执行。
#[derive(Clone)]
pub struct TaskManager {
    inner: Arc<TaskManagerInner>,
}

impl TaskManager {
    /// 纯状态机构造：无沙箱后端，不拉 `pawork-exec`。
    pub fn new() -> Self {
        Self::with_capacity(DEFAULT_BROADCAST_CAPACITY)
    }

    /// 同 [`TaskManager::new`]，可自定义实时事件广播容量（测试用）。
    pub fn with_capacity(capacity: usize) -> Self {
        let (live, _) = broadcast::channel(capacity);
        Self {
            inner: Arc::new(TaskManagerInner {
                state: Mutex::new(TaskManagerState::new()),
                #[cfg(feature = "process-exec")]
                backend: None,
                live,
            }),
        }
    }

    /// 注入沙箱后端；进程执行只经 `backend` 委托。
    #[cfg(feature = "process-exec")]
    pub fn with_backend(backend: Box<dyn SandboxBackend>) -> Self {
        Self::with_backend_capacity(backend, DEFAULT_BROADCAST_CAPACITY)
    }

    /// 同 [`TaskManager::with_backend`]，可自定义实时事件广播容量（测试用）。
    #[cfg(feature = "process-exec")]
    pub fn with_backend_capacity(backend: Box<dyn SandboxBackend>, capacity: usize) -> Self {
        let (live, _) = broadcast::channel(capacity);
        Self {
            inner: Arc::new(TaskManagerInner {
                state: Mutex::new(TaskManagerState::new()),
                backend: Some(Arc::from(backend)),
                live,
            }),
        }
    }

    /// 订阅实时事件流（`AgentEvent::Task(...)`）。
    ///
    /// 收到 `RecvError::Lagged` 表示错过事件：先 `snapshot()` 重建视图，
    /// 再用 `events_since` / `output_since` 续读增量。
    pub fn subscribe(&self) -> broadcast::Receiver<AgentEvent> {
        self.inner.live.subscribe()
    }

    /// 注册任务（状态 Queued，不发事件；`start` 后才进入可重放事件流）。
    pub fn register(
        &self,
        task_kind: TaskKind,
        parent_task_id: Option<BackgroundTaskId>,
    ) -> Result<BackgroundTaskId, TaskManagerError> {
        self.lock().insert_queued(task_kind, parent_task_id)
    }

    /// 开始任务：Queued → Running，发出 `TaskEvent::Started`。
    pub fn start(&self, task_id: &BackgroundTaskId) -> Result<TaskEvent, TaskManagerError> {
        let mut state = self.lock();
        let from = state
            .status(task_id)
            .ok_or_else(|| TaskManagerError::UnknownTask(task_id.clone()))?;
        if from != TaskStatus::Queued {
            return Err(TaskManagerError::InvalidTransition {
                task_id: task_id.clone(),
                from,
                to: TaskStatus::Running,
            });
        }
        let record = state.tasks.get(task_id).expect("status checked above");
        let event = TaskEvent::Started {
            task_id: task_id.clone(),
            task_kind: record.snapshot.task_kind,
            parent_task_id: record.snapshot.parent_task_id.clone(),
        };
        let _ = record;
        state.apply(&event)?;
        drop(state);
        self.broadcast(AgentEvent::Task(event.clone()));
        Ok(event)
    }

    /// 挂起任务：Running → Suspended，发出 `TaskEvent::Suspended`。
    ///
    /// 进程类任务为逻辑挂起（输出继续缓冲）；OS 级暂停由 adapter 落地。
    pub fn suspend(&self, task_id: &BackgroundTaskId) -> Result<TaskEvent, TaskManagerError> {
        self.transition(TaskEvent::Suspended {
            task_id: task_id.clone(),
        })
    }

    /// 恢复任务：Suspended → Running，发出 `TaskEvent::Resumed`。
    pub fn resume(&self, task_id: &BackgroundTaskId) -> Result<TaskEvent, TaskManagerError> {
        self.transition(TaskEvent::Resumed {
            task_id: task_id.clone(),
        })
    }

    /// 完成任务：Running / Suspended → Completed | Failed，发出
    /// `TaskEvent::Finished`。Canceled 必须走 [`TaskManager::cancel`]。
    pub fn finish(
        &self,
        task_id: &BackgroundTaskId,
        status: TaskStatus,
        detail: Option<String>,
    ) -> Result<TaskEvent, TaskManagerError> {
        if !matches!(status, TaskStatus::Completed | TaskStatus::Failed) {
            return Err(TaskManagerError::InvalidFinishedStatus(status));
        }
        let mut state = self.lock();
        let from = state
            .status(task_id)
            .ok_or_else(|| TaskManagerError::UnknownTask(task_id.clone()))?;
        if !matches!(from, TaskStatus::Running | TaskStatus::Suspended) {
            return Err(TaskManagerError::InvalidTransition {
                task_id: task_id.clone(),
                from,
                to: status,
            });
        }
        let event = TaskEvent::Finished {
            task_id: task_id.clone(),
            status,
            detail,
        };
        state.apply(&event)?;
        drop(state);
        self.broadcast(AgentEvent::Task(event.clone()));
        Ok(event)
    }

    /// 取消任务并按 parent_task_id 链传播到全部后代（无孤儿）。
    ///
    /// - Running / Suspended 任务发出 `Finished{status: Canceled}`；
    /// - Queued 任务静默移除（从未持久化，不发事件）；
    /// - 已终态任务跳过；
    /// - 同时触发 domain 取消令牌；`process-exec` 档再取消 exec 令牌
    ///   （经 ProcessRuntime 终止进程树）。
    pub fn cancel(&self, task_id: &BackgroundTaskId) -> Result<Vec<TaskEvent>, TaskManagerError> {
        let mut state = self.lock();
        if state.status(task_id).is_none() {
            return Err(TaskManagerError::UnknownTask(task_id.clone()));
        }
        let subtree = state.subtree(task_id);
        let mut events = Vec::new();
        let mut tokens: Vec<CancellationToken> = Vec::new();
        #[cfg(feature = "process-exec")]
        let mut exec_tokens: Vec<pawork_exec::CancellationToken> = Vec::new();
        for id in subtree {
            let status = state.status(&id);
            let Some(status) = status else {
                continue;
            };
            match status {
                TaskStatus::Queued => {
                    state.remove_queued(&id);
                }
                TaskStatus::Running | TaskStatus::Suspended => {
                    if let Some(token) = state.cancel_token(&id) {
                        tokens.push(token);
                    }
                    #[cfg(feature = "process-exec")]
                    if let Some(token) = state.exec_cancel_token(&id) {
                        exec_tokens.push(token);
                    }
                    let detail = if id == *task_id {
                        Some("canceled by user".to_string())
                    } else {
                        Some("canceled via parent task".to_string())
                    };
                    let event = TaskEvent::Finished {
                        task_id: id.clone(),
                        status: TaskStatus::Canceled,
                        detail,
                    };
                    state.apply(&event)?;
                    events.push(event);
                }
                _ => {}
            }
        }
        drop(state);
        for token in tokens {
            token.cancel();
        }
        #[cfg(feature = "process-exec")]
        for token in exec_tokens {
            token.cancel();
        }
        for event in &events {
            self.broadcast(AgentEvent::Task(event.clone()));
        }
        Ok(events)
    }

    /// 注册并完整执行一个进程类任务（process kind 的唯一执行路径）。
    ///
    /// 流程：注册（Queued）→ 经注入的 `SandboxBackend::spawn` 启动（policy 由
    /// 调用方提供，task-manager 不自定义）→ `start` 发出 Started → 后台驱动
    /// 转发输出并把退出结果折叠为 Completed / Failed / Canceled。
    ///
    /// spawn 失败时清理 Queued 记录并返回错误，不留下幽灵任务。
    ///
    /// 取消令牌桥：domain token 留在 TaskRecord（现有 `cancel()` 路径）；
    /// exec token 传给 `SandboxBackend::spawn`。`cancel()` 时两者都 `cancel()`。
    #[cfg(feature = "process-exec")]
    pub async fn start_process(
        &self,
        spec: SandboxProcessSpec,
        policy: SandboxPolicy,
        parent_task_id: Option<BackgroundTaskId>,
    ) -> Result<BackgroundTaskId, TaskManagerError> {
        let backend = self.inner.backend.clone().ok_or_else(|| {
            TaskManagerError::Sandbox(pawork_exec::SandboxError::BackendUnavailable(
                "task manager has no sandbox backend",
            ))
        })?;
        let task_id = self.register(TaskKind::Process, parent_task_id)?;
        let exec_cancel = pawork_exec::CancellationToken::new();
        self.lock()
            .set_exec_cancel_token(&task_id, exec_cancel.clone());
        let max_output_bytes = spec.command.max_output_bytes;
        let process = match backend.spawn(spec, policy, exec_cancel.clone()).await {
            Ok(process) => process,
            Err(error) => {
                self.lock().remove_queued(&task_id);
                return Err(TaskManagerError::Sandbox(error));
            }
        };
        self.start(&task_id)?;
        let driver = self.clone();
        let driver_task_id = task_id.clone();
        tokio::spawn(async move {
            driver
                .drive_process(driver_task_id, process, exec_cancel, max_output_bytes)
                .await;
        });
        Ok(task_id)
    }

    /// 只读：单个任务快照。
    pub fn task(&self, task_id: &BackgroundTaskId) -> Option<TaskSnapshot> {
        self.lock().task(task_id)
    }

    /// 只读：全部任务快照。
    pub fn tasks(&self) -> Vec<TaskSnapshot> {
        self.lock().tasks()
    }

    /// 只读：任务视图 + 事件日志（断连恢复输入）。
    pub fn snapshot(&self) -> TaskManagerSnapshot {
        self.lock().snapshot()
    }

    /// 只读：完整事件日志。
    pub fn event_log(&self) -> Vec<TaskEvent> {
        self.lock().event_log()
    }

    /// 只读：`seq` 之后的增量事件。
    pub fn events_since(&self, seq: u64) -> Vec<TaskEvent> {
        self.lock().events_since(seq)
    }

    /// 只读：任务完整输出缓冲。
    #[cfg(feature = "process-exec")]
    pub fn output(&self, task_id: &BackgroundTaskId) -> Vec<OutputEvent> {
        self.lock().output(task_id)
    }

    /// 只读：任务输出流 `seq` 之后的增量输出。
    #[cfg(feature = "process-exec")]
    pub fn output_since(&self, task_id: &BackgroundTaskId, seq: u64) -> Vec<OutputEvent> {
        self.lock().output_since(task_id, seq)
    }

    /// 重放：把事件序列折叠进状态，重建任务视图（断连 / 重启恢复入口）。
    ///
    /// 重放只重建状态与日志，不重复广播；事件本身应由调用方持久化
    /// （经 `pawork_domain::AgentEvent::Task` 写入 session-store）。
    pub fn replay(
        &self,
        events: impl IntoIterator<Item = TaskEvent>,
    ) -> Result<usize, TaskManagerError> {
        let mut state = self.lock();
        let mut count = 0;
        for event in events {
            state.apply(&event)?;
            count += 1;
        }
        Ok(count)
    }

    fn transition(&self, event: TaskEvent) -> Result<TaskEvent, TaskManagerError> {
        let mut state = self.lock();
        state.apply(&event)?;
        drop(state);
        self.broadcast(AgentEvent::Task(event.clone()));
        Ok(event)
    }

    fn broadcast(&self, event: AgentEvent) {
        let _ = self.inner.live.send(event);
    }

    fn lock(&self) -> MutexGuard<'_, TaskManagerState> {
        self.inner
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// 进程驱动：转发输出到任务缓冲；流关闭后把退出结果折叠为终态。
    #[cfg(feature = "process-exec")]
    async fn drive_process(
        &self,
        task_id: BackgroundTaskId,
        mut process: SandboxProcess,
        cancel: pawork_exec::CancellationToken,
        max_output_bytes: u64,
    ) {
        let mut exit_code: Option<i32> = None;
        let mut truncated = false;
        while let Some(event) = process.events.recv().await {
            if let pawork_exec::ProcessEvent::Exit {
                code,
                truncated: tr,
            } = &event
            {
                exit_code = *code;
                truncated = *tr;
            }
            self.lock().append_output(&task_id, event, max_output_bytes);
        }

        {
            let state = self.lock();
            let Some(record) = state.tasks.get(&task_id) else {
                return;
            };
            if is_terminal_status(record.snapshot.status) {
                // 已被 cancel（Finished{Canceled} 已发出）或已 finish，避免重复事件。
                return;
            }
        }
        let (status, detail) = if cancel.is_cancelled() {
            (
                TaskStatus::Canceled,
                Some("canceled while running".to_string()),
            )
        } else if exit_code == Some(0) {
            (TaskStatus::Completed, None)
        } else {
            (
                TaskStatus::Failed,
                Some(format!(
                    "process exited abnormally (code={exit_code:?}, truncated={truncated})"
                )),
            )
        };
        // 与并发 cancel 竞争时 finish 可能已被取消事件占用，忽略其错误。
        let _ = self.finish(&task_id, status, detail);
    }
}

impl Default for TaskManager {
    fn default() -> Self {
        Self::new()
    }
}
