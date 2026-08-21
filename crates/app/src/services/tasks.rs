//! Task 领域服务：后台任务注册 / 状态机与 tasks.json 快照持久化。

use std::path::PathBuf;
use std::sync::Mutex;

use pawork_domain::{BackgroundTaskId, DegradeEvent, DegradeKind, DegradeSeverity, SessionId, TaskKind, TaskStatus};
use pawork_workflow::task::{TaskManager, TaskSnapshot};
use serde_json::json;

use crate::tasks_host::{load_task_manager, save_task_manager};
use crate::AppError;

pub(crate) struct TaskService {
    pub(crate) tasks: TaskManager,
    pub(crate) tasks_path: Option<PathBuf>,
    last_degrade: Mutex<Option<DegradeEvent>>,
}

impl TaskService {
    pub(crate) fn new() -> Self {
        Self {
            tasks: TaskManager::new(),
            tasks_path: None,
            last_degrade: Mutex::new(None),
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
        if let Err(error) = self.persist_tasks() {
            report_tasks_persist_failure(None, &error);
        }
        Ok(id)
    }

    pub(crate) fn tasks_finish(
        &self,
        task_id: &BackgroundTaskId,
        status: TaskStatus,
        detail: Option<String>,
    ) -> Result<(), AppError> {
        let degrade = self.tasks_finish_with_degrade(task_id, status, detail)?;
        *self.last_degrade.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = degrade.clone();
        if let Some(degrade) = degrade {
            tracing::error!(
                code = %degrade.code(),
                task_id = %task_id.as_str(),
                "tasks snapshot persist failed"
            );
        }
        Ok(())
    }

    pub(crate) fn take_last_degrade(&self) -> Option<DegradeEvent> {
        self.last_degrade
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
    }

    /// Finish a task and, on persist failure, return the degrade event so a run
    /// sink can persist `AgentEvent::Diagnostic`. No-sink callers log instead.
    pub(crate) fn tasks_finish_with_degrade(
        &self,
        task_id: &BackgroundTaskId,
        status: TaskStatus,
        detail: Option<String>,
    ) -> Result<Option<DegradeEvent>, AppError> {
        self.tasks
            .finish(task_id, status, detail.clone())
            .map_err(|error| AppError::Task(error.to_string()))?;
        match self.persist_tasks() {
            Ok(()) => Ok(None),
            Err(error) => Ok(Some(tasks_finish_failed_event(
                Some(task_id.as_str()),
                &error,
            ))),
        }
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

fn tasks_finish_failed_event(task_id: Option<&str>, error: &AppError) -> DegradeEvent {
    let mut details = json!({ "error": error.to_string() });
    if let Some(task_id) = task_id {
        details["task_id"] = json!(task_id);
    }
    DegradeEvent::new(
        DegradeKind::TasksFinishFailed,
        DegradeSeverity::Error,
        "tasks snapshot persist failed",
        details,
    )
}

fn report_tasks_persist_failure(task_id: Option<&str>, error: &AppError) {
    let event = tasks_finish_failed_event(task_id, error);
    tracing::error!(
        code = %event.code(),
        task_id = task_id.unwrap_or("-"),
        error = %error,
        "tasks snapshot persist failed"
    );
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

    #[tokio::test]
    async fn persist_tasks_failure_emits_degrade_event() {
        let (mut core, dir) = crate::testsupport::mock_core(Vec::new()).await;
        core.open_control_plane(dir.path()).expect("control");
        let id = core
            .tasks_register(pawork_domain::TaskKind::Agent)
            .expect("register");
        // Replace tasks.json with a directory so save_task_manager cannot write.
        let tasks_path = dir.path().join("tasks.json");
        std::fs::remove_file(&tasks_path).ok();
        std::fs::create_dir_all(&tasks_path).expect("block persist path");
        let degrade = core
            .tasks
            .tasks_finish_with_degrade(&id, pawork_domain::TaskStatus::Completed, None)
            .expect("finish succeeds even if persist fails")
            .expect("persist failure must yield DegradeEvent");
        assert_eq!(degrade.code(), "degrade.tasks_finish_failed");
        assert_eq!(degrade.kind, pawork_domain::DegradeKind::TasksFinishFailed);
        assert_eq!(degrade.severity, pawork_domain::DegradeSeverity::Error);
        assert_eq!(degrade.details["task_id"], serde_json::json!(id.as_str()));
        assert!(degrade.details.get("error").is_some());
        core.shutdown().await.expect("shutdown");
    }
}
