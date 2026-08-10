//! 任务分解 / 依赖图（P12-2）。
//!
//! [`TaskGraph`] 维护任务依赖 DAG：拒绝环、拒绝跨租户依赖，提供
//! ready / assign / start / complete / fail / cancel / retry 转换与就绪查询。
//! 内部状态由 `Arc<Mutex<_>>` 保护，锁不跨 `.await` 持有（本模块无 IO）。

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::{Arc, Mutex};

use agent_domain::{AgentId, TenantId};
use serde::{Deserialize, Serialize};

/// 类型安全的任务标识。
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct TaskId(String);

impl TaskId {
    /// 从任意可转换为 `String` 的值构造。
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// 借用内部字符串。
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for TaskId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// 任务状态。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskState {
    /// 已创建（重试复位后的状态）。
    Created,
    /// 依赖全部完成，等待指派。
    Ready,
    /// 已指派给 agent。
    Assigned,
    /// 执行中。
    Running,
    /// 依赖未完成，被阻塞。
    Blocked,
    /// 完成（终态）。
    Completed,
    /// 失败（终态）。
    Failed,
    /// 取消（终态）。
    Cancelled,
}

impl TaskState {
    /// 是否为终态。
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }
}

/// 图中的一个任务。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentTask {
    /// 任务标识。
    pub task_id: TaskId,
    /// 所属租户（依赖必须同租户）。
    pub tenant_id: TenantId,
    /// 负责 agent。
    pub owner: AgentId,
    /// 任务描述。
    pub description: String,
    /// 依赖的任务。
    pub depends_on: Vec<TaskId>,
    /// 已重试次数。
    pub retry_count: u32,
    /// 最大重试次数。
    pub max_retries: u32,
    /// 当前状态。
    pub state: TaskState,
}

/// 任务图错误。
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum TaskGraphError {
    /// 任务标识重复。
    #[error("duplicate task: {0}")]
    DuplicateTask(TaskId),
    /// 任务不存在。
    #[error("unknown task: {0}")]
    UnknownTask(TaskId),
    /// 依赖构成环。
    #[error("dependency cycle detected")]
    CycleDetected,
    /// 跨租户依赖。
    #[error("cross-tenant dependency: task tenant {task_tenant}, dep tenant {dep_tenant}")]
    CrossTenantDependency {
        /// 任务租户。
        task_tenant: String,
        /// 依赖租户。
        dep_tenant: String,
    },
    /// 依赖的任务不存在。
    #[error("unknown dependency: {0}")]
    UnknownDependency(TaskId),
    /// 非法状态转换。
    #[error("illegal state transition from {from:?}")]
    IllegalState {
        /// 来源状态。
        from: TaskState,
    },
}

/// 线程安全的任务依赖图。
#[derive(Clone, Debug, Default)]
pub struct TaskGraph {
    tasks: Arc<Mutex<BTreeMap<TaskId, AgentTask>>>,
}

impl TaskGraph {
    /// 新建空图。
    pub fn new() -> Self {
        Self::default()
    }

    /// 添加任务。
    ///
    /// 拒绝：重复 `task_id`、已知依赖跨租户、依赖成环。允许前向引用
    /// （依赖任务尚未插入）；该依赖就位后由 `ready_tasks` 统一调度。
    /// 初始状态由依赖决定：依赖全部完成（或无依赖）→ `Ready`，否则 `Blocked`。
    pub fn add_task(&self, task: AgentTask) -> Result<(), TaskGraphError> {
        let mut tasks = self
            .tasks
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if tasks.contains_key(&task.task_id) {
            return Err(TaskGraphError::DuplicateTask(task.task_id));
        }
        for dep in &task.depends_on {
            if let Some(dep_task) = tasks.get(dep) {
                if dep_task.tenant_id != task.tenant_id {
                    return Err(TaskGraphError::CrossTenantDependency {
                        task_tenant: task.tenant_id.to_string(),
                        dep_tenant: dep_task.tenant_id.to_string(),
                    });
                }
            }
        }
        // 环检测：现有边 + 新任务的出边。
        let mut deps: BTreeMap<TaskId, Vec<TaskId>> = tasks
            .iter()
            .map(|(id, t)| (id.clone(), t.depends_on.clone()))
            .collect();
        deps.insert(task.task_id.clone(), task.depends_on.clone());
        if Self::detect_cycle(&deps) {
            return Err(TaskGraphError::CycleDetected);
        }

        let ready = task.depends_on.iter().all(|dep| {
            tasks
                .get(dep)
                .is_some_and(|t| t.state == TaskState::Completed)
        });
        let mut task = task;
        task.state = if ready {
            TaskState::Ready
        } else {
            TaskState::Blocked
        };
        tasks.insert(task.task_id.clone(), task);
        Ok(())
    }

    /// Blocked → Ready（依赖已全部完成时才有意义；由 [`Self::ready_tasks`] 发现）。
    pub fn mark_ready(&self, task_id: &TaskId) -> Result<(), TaskGraphError> {
        let mut tasks = self
            .tasks
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let task = tasks
            .get_mut(task_id)
            .ok_or_else(|| TaskGraphError::UnknownTask(task_id.clone()))?;
        match task.state {
            TaskState::Created | TaskState::Blocked => {
                task.state = TaskState::Ready;
                Ok(())
            }
            from => Err(TaskGraphError::IllegalState { from }),
        }
    }

    /// Ready → Assigned。
    pub fn assign(&self, task_id: &TaskId) -> Result<(), TaskGraphError> {
        self.set_state(task_id, TaskState::Ready, TaskState::Assigned)
    }

    /// Assigned → Running。
    pub fn start(&self, task_id: &TaskId) -> Result<(), TaskGraphError> {
        self.set_state(task_id, TaskState::Assigned, TaskState::Running)
    }

    /// Running → Completed。幂等：已完成任务再次 complete 返回 `Ok`。
    pub fn complete(&self, task_id: &TaskId) -> Result<(), TaskGraphError> {
        let mut tasks = self
            .tasks
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let task = tasks
            .get_mut(task_id)
            .ok_or_else(|| TaskGraphError::UnknownTask(task_id.clone()))?;
        match task.state {
            TaskState::Completed => Ok(()),
            TaskState::Running => {
                task.state = TaskState::Completed;
                Ok(())
            }
            from => Err(TaskGraphError::IllegalState { from }),
        }
    }

    /// Running → Failed。
    pub fn fail(&self, task_id: &TaskId) -> Result<(), TaskGraphError> {
        self.set_state(task_id, TaskState::Running, TaskState::Failed)
    }

    /// 任意非终态 → Cancelled。
    pub fn cancel(&self, task_id: &TaskId) -> Result<(), TaskGraphError> {
        let mut tasks = self
            .tasks
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let task = tasks
            .get_mut(task_id)
            .ok_or_else(|| TaskGraphError::UnknownTask(task_id.clone()))?;
        if task.state.is_terminal() {
            return Err(TaskGraphError::IllegalState { from: task.state });
        }
        task.state = TaskState::Cancelled;
        Ok(())
    }

    /// 返回当前为 `Blocked` 且依赖全部完成的任务（调用方随后 `mark_ready`）。
    pub fn ready_tasks(&self) -> Vec<TaskId> {
        let tasks = self
            .tasks
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        tasks
            .iter()
            .filter(|(_, task)| {
                task.state == TaskState::Blocked
                    && task.depends_on.iter().all(|dep| {
                        tasks
                            .get(dep)
                            .is_some_and(|t| t.state == TaskState::Completed)
                    })
            })
            .map(|(id, _)| id.clone())
            .collect()
    }

    /// 重试失败任务：`Failed → Created` 并递增重试计数，返回本次尝试序号（1 起）。
    ///
    /// 任务失败（可重试）与 provider/account 失败由调用方区分：
    /// 本方法只在显式调用 `retry()` 时复位，绝不自动重试。
    pub fn retry(&self, task_id: &TaskId) -> Result<u32, TaskGraphError> {
        let mut tasks = self
            .tasks
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let task = tasks
            .get_mut(task_id)
            .ok_or_else(|| TaskGraphError::UnknownTask(task_id.clone()))?;
        if task.state != TaskState::Failed {
            return Err(TaskGraphError::IllegalState { from: task.state });
        }
        if task.retry_count >= task.max_retries {
            return Err(TaskGraphError::IllegalState { from: task.state });
        }
        task.retry_count += 1;
        task.state = TaskState::Created;
        Ok(task.retry_count)
    }

    /// 查询任务所属租户。
    pub fn tenant_of(&self, task_id: &TaskId) -> Option<TenantId> {
        let tasks = self
            .tasks
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        tasks.get(task_id).map(|task| task.tenant_id.clone())
    }

    /// 内部查询任务状态（测试与图诊断用）。
    pub fn state_of(&self, task_id: &TaskId) -> Option<TaskState> {
        let tasks = self
            .tasks
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        tasks.get(task_id).map(|task| task.state)
    }

    /// 简单转换辅助：`from → to`。
    fn set_state(
        &self,
        task_id: &TaskId,
        from: TaskState,
        to: TaskState,
    ) -> Result<(), TaskGraphError> {
        let mut tasks = self
            .tasks
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let task = tasks
            .get_mut(task_id)
            .ok_or_else(|| TaskGraphError::UnknownTask(task_id.clone()))?;
        if task.state != from {
            return Err(TaskGraphError::IllegalState { from: task.state });
        }
        task.state = to;
        Ok(())
    }

    /// 环检测辅助：对 `deps` 表示的边集做 DFS，存在环返回 `true`。
    ///
    /// `deps[t]` 列出任务 `t` 依赖的任务（`t -> dep` 边）。用于 `add_task`
    /// 在插入前验证新边不会成环。
    pub fn detect_cycle(deps: &BTreeMap<TaskId, Vec<TaskId>>) -> bool {
        #[derive(Clone, Copy, PartialEq)]
        enum Mark {
            Visiting,
            Visited,
        }
        let mut marks: BTreeMap<&TaskId, Mark> = BTreeMap::new();
        fn visit<'a>(
            id: &'a TaskId,
            deps: &'a BTreeMap<TaskId, Vec<TaskId>>,
            marks: &mut BTreeMap<&'a TaskId, Mark>,
        ) -> bool {
            match marks.get(id) {
                Some(Mark::Visiting) => return true,
                Some(Mark::Visited) => return false,
                None => {}
            }
            marks.insert(id, Mark::Visiting);
            if let Some(list) = deps.get(id) {
                for dep in list {
                    if visit(dep, deps, marks) {
                        return true;
                    }
                }
            }
            marks.insert(id, Mark::Visited);
            false
        }
        let ids: BTreeSet<&TaskId> = deps.keys().collect();
        for id in ids {
            if visit(id, deps, &mut marks) {
                return true;
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(id: &str, deps: &[&str]) -> AgentTask {
        AgentTask {
            task_id: TaskId::new(id),
            tenant_id: TenantId::new("tenant-a"),
            owner: AgentId::new("agent-a"),
            description: format!("task {id}"),
            depends_on: deps.iter().map(|d| TaskId::new(*d)).collect(),
            retry_count: 0,
            max_retries: 2,
            state: TaskState::Created,
        }
    }

    #[test]
    fn dag_orders_correctly() {
        let graph = TaskGraph::new();
        graph.add_task(task("build", &["lint"])).unwrap();
        graph.add_task(task("lint", &[])).unwrap();
        // 新任务依赖未完成 → Blocked。
        assert_eq!(
            graph.state_of(&TaskId::new("build")),
            Some(TaskState::Blocked)
        );
        assert_eq!(graph.state_of(&TaskId::new("lint")), Some(TaskState::Ready));

        // 无依赖任务：Ready → assign → start → complete。
        graph.assign(&TaskId::new("lint")).unwrap();
        graph.start(&TaskId::new("lint")).unwrap();
        graph.complete(&TaskId::new("lint")).unwrap();
        assert_eq!(
            graph.state_of(&TaskId::new("lint")),
            Some(TaskState::Completed)
        );
        // 依赖完成后，被阻塞任务进入就绪集合。
        let ready = graph.ready_tasks();
        assert!(ready.contains(&TaskId::new("build")));
        graph.mark_ready(&TaskId::new("build")).unwrap();
        assert_eq!(
            graph.state_of(&TaskId::new("build")),
            Some(TaskState::Ready)
        );
        graph.assign(&TaskId::new("build")).unwrap();
        graph.start(&TaskId::new("build")).unwrap();
        graph.complete(&TaskId::new("build")).unwrap();
        assert!(graph.ready_tasks().is_empty());
    }

    #[test]
    fn cycle_rejected() {
        let graph = TaskGraph::new();
        graph.add_task(task("a", &["b"])).unwrap();
        graph.add_task(task("b", &["c"])).unwrap();
        // 插入 c -> a 将成环：a -> b -> c -> a。
        let err = graph.add_task(task("c", &["a"])).unwrap_err();
        assert_eq!(err, TaskGraphError::CycleDetected);
        // 图未被污染。
        assert!(graph.tenant_of(&TaskId::new("c")).is_none());
        assert!(graph.state_of(&TaskId::new("a")).is_some());

        // 自依赖同样拒绝。
        let err = graph.add_task(task("self", &["self"])).unwrap_err();
        assert_eq!(err, TaskGraphError::CycleDetected);
    }

    #[test]
    fn cross_tenant_dependency_rejected() {
        let graph = TaskGraph::new();
        graph.add_task(task("a", &[])).unwrap();
        let mut foreign = task("b", &["a"]);
        foreign.tenant_id = TenantId::new("tenant-b");
        let err = graph.add_task(foreign).unwrap_err();
        assert!(matches!(
            err,
            TaskGraphError::CrossTenantDependency {
                ref task_tenant,
                ref dep_tenant,
            } if task_tenant == "tenant-b" && dep_tenant == "tenant-a"
        ));
    }

    #[test]
    fn ready_after_deps_complete() {
        let graph = TaskGraph::new();
        graph.add_task(task("a", &[])).unwrap();
        graph.add_task(task("b", &["a"])).unwrap();
        assert_eq!(graph.state_of(&TaskId::new("b")), Some(TaskState::Blocked));

        graph.assign(&TaskId::new("a")).unwrap();
        graph.start(&TaskId::new("a")).unwrap();
        graph.complete(&TaskId::new("a")).unwrap();

        assert_eq!(
            graph.ready_tasks(),
            vec![TaskId::new("b")],
            "依赖全部完成后 Blocked 任务应出现在 ready_tasks"
        );
        graph.mark_ready(&TaskId::new("b")).unwrap();
        assert_eq!(graph.state_of(&TaskId::new("b")), Some(TaskState::Ready));

        // 任务在依赖已完成的时刻插入 → 直接 Ready。
        graph.add_task(task("c", &["a"])).unwrap();
        assert_eq!(graph.state_of(&TaskId::new("c")), Some(TaskState::Ready));
    }

    #[test]
    fn retry_increments_attempt() {
        let graph = TaskGraph::new();
        graph.add_task(task("a", &[])).unwrap();
        graph.assign(&TaskId::new("a")).unwrap();
        graph.start(&TaskId::new("a")).unwrap();
        graph.fail(&TaskId::new("a")).unwrap();
        assert_eq!(graph.state_of(&TaskId::new("a")), Some(TaskState::Failed));

        let attempt = graph.retry(&TaskId::new("a")).unwrap();
        assert_eq!(attempt, 1, "第一次重试应返回 attempt=1");
        assert_eq!(graph.state_of(&TaskId::new("a")), Some(TaskState::Created));

        // 复位后可重新走就绪流程。
        graph.mark_ready(&TaskId::new("a")).unwrap();
        graph.assign(&TaskId::new("a")).unwrap();
        graph.start(&TaskId::new("a")).unwrap();
        graph.fail(&TaskId::new("a")).unwrap();
        let attempt = graph.retry(&TaskId::new("a")).unwrap();
        assert_eq!(attempt, 2);

        // 超过 max_retries 后拒绝重试。
        graph.mark_ready(&TaskId::new("a")).unwrap();
        graph.assign(&TaskId::new("a")).unwrap();
        graph.start(&TaskId::new("a")).unwrap();
        graph.fail(&TaskId::new("a")).unwrap();
        let err = graph.retry(&TaskId::new("a")).unwrap_err();
        assert!(matches!(err, TaskGraphError::IllegalState { .. }));

        // 非 Failed 状态不可重试。
        graph.add_task(task("b", &[])).unwrap();
        let err = graph.retry(&TaskId::new("b")).unwrap_err();
        assert!(matches!(err, TaskGraphError::IllegalState { .. }));
    }

    #[test]
    fn idempotent_complete() {
        let graph = TaskGraph::new();
        graph.add_task(task("a", &[])).unwrap();
        graph.assign(&TaskId::new("a")).unwrap();
        graph.start(&TaskId::new("a")).unwrap();
        graph.complete(&TaskId::new("a")).unwrap();
        // 第二次 complete 幂等成功。
        graph.complete(&TaskId::new("a")).unwrap();
        assert_eq!(
            graph.state_of(&TaskId::new("a")),
            Some(TaskState::Completed)
        );
        // 未运行的任务不可 complete。
        graph.add_task(task("b", &[])).unwrap();
        let err = graph.complete(&TaskId::new("b")).unwrap_err();
        assert!(matches!(err, TaskGraphError::IllegalState { .. }));
    }

    #[test]
    fn duplicate_task_rejected() {
        let graph = TaskGraph::new();
        graph.add_task(task("a", &[])).unwrap();
        assert_eq!(
            graph.add_task(task("a", &[])).unwrap_err(),
            TaskGraphError::DuplicateTask(TaskId::new("a"))
        );
    }
}
