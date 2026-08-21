//! S11 波 D：后台任务可见面。默认纯状态机，快照落在实例目录 `tasks.json`。

use std::fs;
use std::path::{Path, PathBuf};

use pawork_domain::{BackgroundTaskId, SessionId, TaskKind, TaskStatus};
use pawork_workflow::task::{TaskManager, TaskManagerSnapshot, TaskSnapshot};

use crate::AppError;

impl crate::AppCore {
    pub fn tasks_list(&self) -> Vec<TaskSnapshot> {
        self.tasks.tasks_list()
    }

    pub fn tasks_status(&self, spec: &str) -> Result<TaskSnapshot, AppError> {
        self.tasks.tasks_status(spec)
    }

    pub fn tasks_register(&self, kind: TaskKind) -> Result<BackgroundTaskId, AppError> {
        self.tasks.tasks_register(kind)
    }

    pub fn tasks_cancel(&self, spec: &str) -> Result<Vec<BackgroundTaskId>, AppError> {
        self.tasks.tasks_cancel(spec)
    }

    pub(crate) fn tasks_start_agent(
        &self,
        session_id: Option<&SessionId>,
    ) -> Result<BackgroundTaskId, AppError> {
        self.tasks.tasks_start_agent(session_id)
    }

    pub(crate) fn tasks_finish(
        &self,
        task_id: &BackgroundTaskId,
        status: TaskStatus,
        detail: Option<String>,
    ) -> Result<(), AppError> {
        self.tasks.tasks_finish(task_id, status, detail)
    }

    pub(crate) fn open_tasks(&mut self, path: PathBuf) -> Result<(), AppError> {
        self.tasks.open_tasks(path)
    }
}

pub fn parse_task_kind(value: &str) -> Result<TaskKind, AppError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "agent" => Ok(TaskKind::Agent),
        "automation" => Ok(TaskKind::Automation),
        "monitor" => Ok(TaskKind::Monitor),
        "process" => Ok(TaskKind::Process),
        other => Err(AppError::Task(format!(
            "unknown task kind `{other}` (agent|automation|monitor|process)"
        ))),
    }
}

pub(crate) fn load_task_manager(path: &Path) -> Result<TaskManager, AppError> {
    if !path.exists() {
        return Ok(TaskManager::new());
    }
    let bytes = fs::read(path)?;
    if bytes.is_empty() {
        return Ok(TaskManager::new());
    }
    let snapshot: TaskManagerSnapshot = serde_json::from_slice(&bytes)
        .map_err(|error| AppError::Task(format!("tasks snapshot corrupt: {error}")))?;
    let manager = TaskManager::new();
    manager
        .replay(snapshot.events)
        .map_err(|error| AppError::Task(error.to_string()))?;
    Ok(manager)
}

pub(crate) fn save_task_manager(
    path: &Path,
    snapshot: &TaskManagerSnapshot,
) -> Result<(), AppError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_vec_pretty(snapshot)
        .map_err(|error| AppError::Task(format!("serialize tasks: {error}")))?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, json)?;
    if path.exists() {
        fs::remove_file(path)?;
    }
    fs::rename(&tmp, path)?;
    Ok(())
}
