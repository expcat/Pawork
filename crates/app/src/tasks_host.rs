//! S11 波 D：后台任务可见面。默认纯状态机，快照落在实例目录 `tasks.json`。

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

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
        cancel: &pawork_domain::CancellationToken,
    ) -> Result<BackgroundTaskId, AppError> {
        self.tasks.tasks_start_agent(session_id, cancel)
    }

    pub(crate) fn tasks_finish_from_run(
        &self,
        task_id: &BackgroundTaskId,
        status: TaskStatus,
        detail: Option<String>,
    ) -> Result<(), AppError> {
        self.tasks.tasks_finish_from_run(task_id, status, detail)
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
    // R-03：独占创建的独立临时文件 + rename 直接原子替换——不先删原文件，
    // 替换失败或中途崩溃时上一份快照仍可读；并发保存不共享同一临时路径
    // （与 auth file_backend 的写入形态一致）。
    static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);
    let mut attempt = 0u32;
    let tmp = loop {
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("tasks.json");
        let candidate = path.with_file_name(format!(
            ".{name}.{}.{}.tmp",
            std::process::id(),
            TMP_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(mut file) => {
                let written = file.write_all(&json).and_then(|()| file.sync_all());
                drop(file);
                if let Err(error) = written {
                    let _ = fs::remove_file(&candidate);
                    return Err(error.into());
                }
                break candidate;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists && attempt + 1 < 16 => {
                attempt += 1;
            }
            Err(error) => return Err(error.into()),
        }
    };
    fs::rename(&tmp, path).map_err(|error| {
        let _ = fs::remove_file(&tmp);
        AppError::from(error)
    })
}
