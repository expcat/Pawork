//! 任务聚合状态与纯函数折叠（event-sourcing）。
//!
//! [`TaskManagerState`] 保存任务表与 in-memory 事件日志，是「重放/恢复」的
//! 唯一入口：`apply` 把一个 canonical [`TaskEvent`] 折叠进状态，事件序列可
//! 无损重建任务视图。命令方法（见 [`crate::task::TaskManager`]）先校验
//! 状态机合法性，再 apply 并返回事件供调用方持久化。

use std::collections::BTreeMap;

use pawork_domain::{BackgroundTaskId, CancellationToken, TaskEvent, TaskKind, TaskStatus};
use serde::{Deserialize, Serialize};

use crate::task::error::TaskManagerError;

/// 任务是否处于终态（Completed / Failed / Canceled）。
pub fn is_terminal_status(status: TaskStatus) -> bool {
    matches!(
        status,
        TaskStatus::Completed | TaskStatus::Failed | TaskStatus::Canceled
    )
}

/// 任务是否仍可转移（Queued / Running / Suspended）。
pub fn is_active_status(status: TaskStatus) -> bool {
    matches!(
        status,
        TaskStatus::Queued | TaskStatus::Running | TaskStatus::Suspended
    )
}

/// 任务的只读快照（serde 可序列化，用于 snapshot / 重连恢复）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskSnapshot {
    /// 统一任务 ID。
    pub task_id: BackgroundTaskId,
    /// 任务种类。
    pub task_kind: TaskKind,
    /// 父任务（取消按此链传播）。
    pub parent_task_id: Option<BackgroundTaskId>,
    /// 当前状态。
    pub status: TaskStatus,
    /// 终态补充说明（如退出码 / 取消原因）。
    pub detail: Option<String>,
    /// 该任务输出流的下一个游标（重连后从 `output_since(output_seq)` 续读增量）。
    /// 默认档无输出缓冲，恒为 0。
    pub output_seq: u64,
    /// 已缓冲输出字节数（上限为 CommandSpec::max_output_bytes）。
    /// 默认档无输出缓冲，恒为 0。
    pub output_bytes: u64,
}

/// 任务管理器的整体快照：任务视图 + 完整事件日志（重放输入）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskManagerSnapshot {
    /// 当前任务视图。
    pub tasks: Vec<TaskSnapshot>,
    /// 已发出（且已折叠进 state）的 canonical 事件序列，按序重放可重建视图。
    pub events: Vec<TaskEvent>,
}

/// 任务运行期记录：可序列化快照 + 非序列化运行态（取消令牌、输出缓冲）。
#[derive(Clone, Debug)]
pub(crate) struct TaskRecord {
    pub(crate) snapshot: TaskSnapshot,
    /// 进程类任务取消时触发进程树终止；其他 kind 由 adapter 自行消费。
    pub(crate) cancel_token: CancellationToken,
}

impl TaskRecord {
    pub(crate) fn new(
        task_id: BackgroundTaskId,
        task_kind: TaskKind,
        parent_task_id: Option<BackgroundTaskId>,
        status: TaskStatus,
    ) -> Self {
        Self {
            snapshot: TaskSnapshot {
                task_id,
                task_kind,
                parent_task_id,
                status,
                detail: None,
                output_seq: 0,
                output_bytes: 0,
            },
            cancel_token: CancellationToken::new(),
        }
    }
}

/// 后台任务聚合状态：任务表 + 事件日志。
///
/// `apply` 是唯一的折叠入口；命令方法通过它落状态。日志只追加，
/// `events_since(seq)` 用日志下标直接切片（seq 与下标一一对应）。
#[derive(Clone, Debug, Default)]
pub struct TaskManagerState {
    pub(crate) tasks: BTreeMap<BackgroundTaskId, TaskRecord>,
    pub(crate) log: Vec<TaskEvent>,
    pub(crate) next_task_seq: u64,
}

impl TaskManagerState {
    /// 空状态。
    pub fn new() -> Self {
        Self::default()
    }

    /// 纯函数折叠：把 canonical 事件应用到当前状态并追加日志。
    ///
    /// `Started` 幂等（已存在则刷新状态为 Running）；`Suspended` / `Resumed`
    /// / `Finished` 校验前置状态，非法转移或未知任务返回错误。
    pub fn apply(&mut self, event: &TaskEvent) -> Result<(), TaskManagerError> {
        self.note_allocated_id(event_task_id(event));
        match event {
            TaskEvent::Started {
                task_id,
                task_kind,
                parent_task_id,
            } => {
                let record = self.tasks.entry(task_id.clone()).or_insert_with(|| {
                    TaskRecord::new(
                        task_id.clone(),
                        *task_kind,
                        parent_task_id.clone(),
                        TaskStatus::Running,
                    )
                });
                record.snapshot.task_kind = *task_kind;
                record.snapshot.parent_task_id = parent_task_id.clone();
                record.snapshot.status = TaskStatus::Running;
                record.snapshot.detail = None;
            }
            TaskEvent::Suspended { task_id } => {
                self.transition(task_id, TaskStatus::Running, TaskStatus::Suspended, None)?;
            }
            TaskEvent::Resumed { task_id } => {
                self.transition(task_id, TaskStatus::Suspended, TaskStatus::Running, None)?;
            }
            TaskEvent::Finished {
                task_id,
                status,
                detail,
            } => {
                if !is_terminal_status(*status) {
                    return Err(TaskManagerError::InvalidFinishedStatus(*status));
                }
                let record = self
                    .tasks
                    .get_mut(task_id)
                    .ok_or_else(|| TaskManagerError::UnknownTask(task_id.clone()))?;
                let from = record.snapshot.status;
                if !matches!(from, TaskStatus::Running | TaskStatus::Suspended) {
                    return Err(TaskManagerError::InvalidTransition {
                        task_id: task_id.clone(),
                        from,
                        to: *status,
                    });
                }
                record.snapshot.status = *status;
                record.snapshot.detail = detail.clone();
            }
        }
        self.log.push(event.clone());
        Ok(())
    }

    fn transition(
        &mut self,
        task_id: &BackgroundTaskId,
        from: TaskStatus,
        to: TaskStatus,
        detail: Option<String>,
    ) -> Result<(), TaskManagerError> {
        let record = self
            .tasks
            .get_mut(task_id)
            .ok_or_else(|| TaskManagerError::UnknownTask(task_id.clone()))?;
        if record.snapshot.status != from {
            return Err(TaskManagerError::InvalidTransition {
                task_id: task_id.clone(),
                from: record.snapshot.status,
                to,
            });
        }
        record.snapshot.status = to;
        record.snapshot.detail = detail;
        Ok(())
    }

    /// 只读：单个任务快照。
    pub fn task(&self, task_id: &BackgroundTaskId) -> Option<TaskSnapshot> {
        self.tasks.get(task_id).map(|r| r.snapshot.clone())
    }

    /// 只读：全部任务快照（按 ID 排序，确定性输出）。
    pub fn tasks(&self) -> Vec<TaskSnapshot> {
        self.tasks.values().map(|r| r.snapshot.clone()).collect()
    }

    /// 只读：任务视图 + 完整事件日志。
    pub fn snapshot(&self) -> TaskManagerSnapshot {
        TaskManagerSnapshot {
            tasks: self.tasks(),
            events: self.log.clone(),
        }
    }

    /// 只读：完整事件日志。
    pub fn event_log(&self) -> Vec<TaskEvent> {
        self.log.clone()
    }

    /// 只读：`seq` 之后的增量事件（重连恢复用）。
    pub fn events_since(&self, seq: u64) -> Vec<TaskEvent> {
        self.log.get(seq as usize..).unwrap_or_default().to_vec()
    }

    /// 命令辅助：注册（Queued，不发事件，重放不可见——queued 属于持久化前瞬态）。
    pub(crate) fn insert_queued(
        &mut self,
        task_kind: TaskKind,
        parent_task_id: Option<BackgroundTaskId>,
    ) -> Result<BackgroundTaskId, TaskManagerError> {
        if let Some(parent) = &parent_task_id {
            if !self.tasks.contains_key(parent) {
                return Err(TaskManagerError::UnknownParent(parent.clone()));
            }
        }
        let task_id = self.allocate_task_id();
        self.tasks.insert(
            task_id.clone(),
            TaskRecord::new(
                task_id.clone(),
                task_kind,
                parent_task_id,
                TaskStatus::Queued,
            ),
        );
        Ok(task_id)
    }

    /// 命令辅助：移除未开始的 Queued 任务（spawn 失败清理 / 取消 queued）。
    pub(crate) fn remove_queued(&mut self, task_id: &BackgroundTaskId) -> bool {
        let removed = self
            .tasks
            .get(task_id)
            .is_some_and(|r| r.snapshot.status == TaskStatus::Queued);
        if removed {
            self.tasks.remove(task_id);
        }
        removed
    }

    /// 命令辅助：读任务状态。
    pub(crate) fn status(&self, task_id: &BackgroundTaskId) -> Option<TaskStatus> {
        self.tasks.get(task_id).map(|r| r.snapshot.status)
    }

    /// 命令辅助：读任务取消令牌（domain；现有 cancel() 路径）。
    pub(crate) fn cancel_token(&self, task_id: &BackgroundTaskId) -> Option<CancellationToken> {
        self.tasks.get(task_id).map(|r| r.cancel_token.clone())
    }


    /// 命令辅助：收集 `root` 及其全部后代（沿 parent_task_id 链，含 root）。
    pub(crate) fn subtree(&self, root: &BackgroundTaskId) -> Vec<BackgroundTaskId> {
        let mut children: BTreeMap<BackgroundTaskId, Vec<BackgroundTaskId>> = BTreeMap::new();
        for (task_id, record) in &self.tasks {
            if let Some(parent) = &record.snapshot.parent_task_id {
                children
                    .entry(parent.clone())
                    .or_default()
                    .push(task_id.clone());
            }
        }
        let mut ids = Vec::new();
        let mut queue = vec![root.clone()];
        while let Some(current) = queue.pop() {
            ids.push(current.clone());
            if let Some(kids) = children.get(&current) {
                queue.extend(kids.iter().cloned());
            }
        }
        ids
    }

    fn allocate_task_id(&mut self) -> BackgroundTaskId {
        loop {
            let task_id = BackgroundTaskId::new(format!("task_{}", self.next_task_seq));
            self.next_task_seq = self.next_task_seq.saturating_add(1);
            if !self.tasks.contains_key(&task_id) {
                return task_id;
            }
        }
    }

    fn note_allocated_id(&mut self, task_id: &BackgroundTaskId) {
        let Some(suffix) = task_id.as_str().strip_prefix("task_") else {
            return;
        };
        if let Ok(n) = suffix.parse::<u64>() {
            self.next_task_seq = self.next_task_seq.max(n.saturating_add(1));
        }
    }
}

fn event_task_id(event: &TaskEvent) -> &BackgroundTaskId {
    match event {
        TaskEvent::Started { task_id, .. }
        | TaskEvent::Suspended { task_id }
        | TaskEvent::Resumed { task_id }
        | TaskEvent::Finished { task_id, .. } => task_id,
    }
}

