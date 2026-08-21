//! Task 领域服务：后台任务注册 / 状态机与 tasks.json 快照持久化。

use std::path::PathBuf;

use pawork_domain::{BackgroundTaskId, SessionId, TaskKind, TaskStatus};
use pawork_workflow::task::{TaskManager, TaskSnapshot};

use crate::tasks_host::{load_task_manager, save_task_manager};
use crate::AppError;

pub(crate) struct TaskService {
    pub(crate) tasks: TaskManager,
    pub(crate) tasks_path: Option<PathBuf>,
}

impl TaskService {
    pub(crate) fn new() -> Self {
        Self {
            tasks: TaskManager::new(),
            tasks_path: None,
        }
    }

    pub fn tasks_list(&self) -> Vec<TaskSnapshot> {
        self.tasks.tasks()
    }

    pub fn tasks_status(&self, spec: &str) -> Result<TaskSnapshot, AppError> {
        Ok(self.resolve_task(spec)?.1)
    }

    pub fn tasks_register(&self, kind: TaskKind) -> Result<BackgroundTaskId, AppError> {
        let id = self
            .tasks
            .register(kind, None)
            .map_err(|error| AppError::Task(error.to_string()))?;
        self.tasks
            .start(&id)
            .map_err(|error| AppError::Task(error.to_string()))?;
        self.persist_tasks()?;
        Ok(id)
    }

    pub fn tasks_cancel(&self, spec: &str) -> Result<Vec<BackgroundTaskId>, AppError> {
        let (id, _) = self.resolve_task(spec)?;
        let events = self
            .tasks
            .cancel(&id)
            .map_err(|error| AppError::Task(error.to_string()))?;
        self.persist_tasks()?;
        Ok(events
            .into_iter()
            .map(|event| match event {
                pawork_domain::TaskEvent::Finished { task_id, .. } => task_id,
                pawork_domain::TaskEvent::Started { task_id, .. } => task_id,
                pawork_domain::TaskEvent::Suspended { task_id } => task_id,
                pawork_domain::TaskEvent::Resumed { task_id } => task_id,
            })
            .collect())
    }

    pub(crate) fn tasks_start_agent(
        &self,
        _session_id: Option<&SessionId>,
    ) -> Result<BackgroundTaskId, AppError> {
        let id = self
            .tasks
            .register(TaskKind::Agent, None)
            .map_err(|error| AppError::Task(error.to_string()))?;
        self.tasks
            .start(&id)
            .map_err(|error| AppError::Task(error.to_string()))?;
        let _ = self.persist_tasks();
        Ok(id)
    }

    pub(crate) fn tasks_finish(
        &self,
        task_id: &BackgroundTaskId,
        status: TaskStatus,
        detail: Option<String>,
    ) -> Result<(), AppError> {
        self.tasks
            .finish(task_id, status, detail)
            .map_err(|error| AppError::Task(error.to_string()))?;
        let _ = self.persist_tasks();
        Ok(())
    }

    pub(crate) fn open_tasks(&mut self, path: PathBuf) -> Result<(), AppError> {
        self.tasks = load_task_manager(&path)?;
        self.tasks_path = Some(path);
        Ok(())
    }

    fn persist_tasks(&self) -> Result<(), AppError> {
        let Some(path) = &self.tasks_path else {
            return Ok(());
        };
        save_task_manager(path, &self.tasks.snapshot())
    }

    fn resolve_task(&self, spec: &str) -> Result<(BackgroundTaskId, TaskSnapshot), AppError> {
        let spec = spec.trim();
        if spec.is_empty() {
            return Err(AppError::Task("task id is empty".into()));
        }
        let tasks = self.tasks.tasks();
        if let Some(task) = tasks.iter().find(|task| task.task_id.as_str() == spec) {
            return Ok((task.task_id.clone(), task.clone()));
        }
        let matches: Vec<_> = tasks
            .iter()
            .filter(|task| task.task_id.as_str().starts_with(spec))
            .cloned()
            .collect();
        match matches.as_slice() {
            [task] => Ok((task.task_id.clone(), task.clone())),
            [] => Err(AppError::Task(format!("task not found: {spec}"))),
            many => Err(AppError::Task(format!(
                "ambiguous task `{spec}` matches: {}",
                many.iter()
                    .map(|task| task.task_id.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn tasks_register_list_and_cancel() {
        let (core, _dir) = crate::testsupport::mock_core(Vec::new()).await;
        let id = core
            .tasks_register(pawork_domain::TaskKind::Automation)
            .expect("register");
        let listed = core.tasks_list();
        assert!(listed.iter().any(|task| task.task_id == id));
        let cancelled = core.tasks_cancel(id.as_str()).expect("cancel");
        assert!(cancelled.contains(&id));
        let status = core.tasks_status(id.as_str()).expect("status");
        assert_eq!(status.status, pawork_domain::TaskStatus::Canceled);
        core.shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    async fn tasks_persist_and_allocate_new_ids() {
        let (mut core, dir) = crate::testsupport::mock_core(Vec::new()).await;
        core.open_control_plane(dir.path()).expect("control");
        let first = core
            .tasks_register(pawork_domain::TaskKind::Agent)
            .expect("first");
        core.tasks_finish(&first, pawork_domain::TaskStatus::Completed, None)
            .expect("finish");
        core.shutdown().await.expect("shutdown first");

        let (mut core, _dir) = crate::testsupport::mock_core(Vec::new()).await;
        core.open_control_plane(dir.path()).expect("reload");
        let second = core
            .tasks_register(pawork_domain::TaskKind::Automation)
            .expect("second");
        assert_ne!(second, first);
        let listed = core.tasks_list();
        assert!(listed.iter().any(|task| task.task_id == first
            && task.status == pawork_domain::TaskStatus::Completed));
        assert!(listed.iter().any(|task| task.task_id == second));
        core.shutdown().await.expect("shutdown");
    }
}
