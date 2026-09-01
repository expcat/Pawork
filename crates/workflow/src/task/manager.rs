//! 后台任务管理器：统一注册 / 状态机 / 事件广播。
//!
//! 本构建是纯状态机，不持有 `SandboxBackend`。本模块只编排事件转发与状态机，
//! 不直接启动子进程、不自行清理进程树、不自定义 sandbox policy。

use std::sync::{Arc, Mutex, MutexGuard};

use pawork_domain::AgentEvent;
use pawork_domain::{BackgroundTaskId, CancellationToken, TaskEvent, TaskKind, TaskStatus};
use tokio::sync::broadcast;

use crate::task::error::TaskManagerError;
use crate::task::state::{TaskManagerSnapshot, TaskManagerState, TaskSnapshot};

/// 实时事件广播默认容量；超出的订阅者会收到 `RecvError::Lagged`，
/// 此时应通过 `snapshot()` + `events_since()` 重连恢复。
const DEFAULT_BROADCAST_CAPACITY: usize = 256;

struct TaskManagerInner {
    state: Mutex<TaskManagerState>,
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
    /// - 同时触发 domain 取消令牌。
    pub fn cancel(&self, task_id: &BackgroundTaskId) -> Result<Vec<TaskEvent>, TaskManagerError> {
        let mut state = self.lock();
        if state.status(task_id).is_none() {
            return Err(TaskManagerError::UnknownTask(task_id.clone()));
        }
        let subtree = state.subtree(task_id);
        let mut events = Vec::new();
        let mut tokens: Vec<CancellationToken> = Vec::new();
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
        for event in &events {
            self.broadcast(AgentEvent::Task(event.clone()));
        }
        Ok(events)
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
}

impl Default for TaskManager {
    fn default() -> Self {
        Self::new()
    }
}
