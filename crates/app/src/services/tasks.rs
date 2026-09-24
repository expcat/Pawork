//! Task 领域服务：后台任务注册 / 状态机与 tasks.json 快照持久化。

use std::path::PathBuf;
use std::sync::Mutex;

use pawork_domain::{
    BackgroundTaskId, CancellationToken, DegradeEvent, DegradeKind, DegradeSeverity, SessionId,
    TaskKind, TaskStatus,
};
use pawork_workflow::task::{is_terminal_status, TaskManager, TaskSnapshot};
use serde_json::json;

use crate::tasks_host::{load_task_manager, save_task_manager};
use crate::AppError;

pub(crate) struct TaskService {
    pub(crate) tasks: TaskManager,
    pub(crate) tasks_path: Option<PathBuf>,
    last_degrade: Mutex<Option<DegradeEvent>>,
    /// R-03：取快照到提交串行化，避免并发保存让旧快照后写覆盖新快照。
    persist_lock: Mutex<()>,
}

impl TaskService {
    pub(crate) fn new() -> Self {
        Self {
            tasks: TaskManager::new(),
            tasks_path: None,
            last_degrade: Mutex::new(None),
            persist_lock: Mutex::new(()),
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
        cancel: &CancellationToken,
    ) -> Result<BackgroundTaskId, AppError> {
        // R-05：Agent 任务共享真实 run 的取消令牌——tasks cancel 经
        // TaskManager::cancel 触发同一令牌，停止真实执行体而不是只改状态。
        let id = self
            .tasks
            .register_with_cancel_token(TaskKind::Agent, None, cancel.clone())
            .map_err(|error| AppError::Task(error.to_string()))?;
        self.tasks
            .start(&id)
            .map_err(|error| AppError::Task(error.to_string()))?;
        if let Err(error) = self.persist_tasks() {
            report_tasks_persist_failure(None, &error);
        }
        Ok(id)
    }

    /// R-05：run 终态是 Agent 任务终态的唯一事实源。Canceled 走 cancel
    /// 状态机（finish 只接受 Completed/Failed）；任务已终态（如
    /// tasks_cancel 先行收口）时不重复写——终态仅落一次。
    pub(crate) fn tasks_finish_from_run(
        &self,
        task_id: &BackgroundTaskId,
        status: TaskStatus,
        detail: Option<String>,
    ) -> Result<(), AppError> {
        if status == TaskStatus::Canceled {
            // cancel 幂等：Running 落 Finished{Canceled}，已终态跳过。
            self.tasks
                .cancel(task_id)
                .map_err(|error| AppError::Task(error.to_string()))?;
        } else if let Err(error) = self.tasks.finish(task_id, status, detail) {
            let already_terminal = self
                .tasks
                .task(task_id)
                .is_some_and(|snapshot| is_terminal_status(snapshot.status));
            if !already_terminal {
                return Err(AppError::Task(error.to_string()));
            }
        }
        let degrade = match self.persist_tasks() {
            Ok(()) => None,
            Err(error) => Some(tasks_finish_failed_event(Some(task_id.as_str()), &error)),
        };
        self.note_finish_degrade(task_id, degrade);
        Ok(())
    }

    fn note_finish_degrade(&self, task_id: &BackgroundTaskId, degrade: Option<DegradeEvent>) {
        *self
            .last_degrade
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = degrade.clone();
        if let Some(degrade) = degrade {
            tracing::error!(
                code = %degrade.code(),
                task_id = %task_id.as_str(),
                "tasks snapshot persist failed"
            );
        }
    }

    pub(crate) fn take_last_degrade(&self) -> Option<DegradeEvent> {
        self.last_degrade
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
    }

    pub(crate) fn open_tasks(&mut self, path: PathBuf) -> Result<(), AppError> {
        self.tasks = load_task_manager(&path)?;
        self.tasks_path = Some(path);
        Ok(())
    }

    /// R-23：孤儿任务收口。仅当宿主取得实例清扫权（无其他活跃宿主）
    /// 时由调用方触发：重放快照中仍为 Running/Suspended 的任务，其执行体
    /// 已随上一进程退出，收为 Failed 并持久化——快照不得继续声称运行中。
    /// 终态任务跳过（重复恢复幂等）；单个失败只 warn，不阻断启动。
    pub(crate) fn seal_orphaned_active_tasks(&self) {
        let orphaned: Vec<BackgroundTaskId> = self
            .tasks
            .tasks()
            .into_iter()
            .filter(|task| matches!(task.status, TaskStatus::Running | TaskStatus::Suspended))
            .map(|task| task.task_id)
            .collect();
        if orphaned.is_empty() {
            return;
        }
        let mut sealed = 0usize;
        for task_id in &orphaned {
            match self.tasks.finish(
                task_id,
                TaskStatus::Failed,
                Some("interrupted: owner process exited before task terminal".into()),
            ) {
                Ok(_) => sealed += 1,
                Err(error) => {
                    tracing::warn!(task_id = %task_id.as_str(), error = %error, "orphan task seal failed")
                }
            }
        }
        if sealed > 0 {
            if let Err(error) = self.persist_tasks() {
                report_tasks_persist_failure(None, &error);
            }
        }
    }

    fn persist_tasks(&self) -> Result<(), AppError> {
        let Some(path) = &self.tasks_path else {
            return Ok(());
        };
        let _guard = self
            .persist_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
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
        core.tasks_finish_from_run(&first, pawork_domain::TaskStatus::Completed, None)
            .expect("finish");
        core.shutdown().await.expect("shutdown first");

        let (mut core, _dir) = crate::testsupport::mock_core(Vec::new()).await;
        core.open_control_plane(dir.path()).expect("reload");
        let second = core
            .tasks_register(pawork_domain::TaskKind::Automation)
            .expect("second");
        assert_ne!(second, first);
        let listed = core.tasks_list();
        assert!(listed.iter().any(
            |task| task.task_id == first && task.status == pawork_domain::TaskStatus::Completed
        ));
        assert!(listed.iter().any(|task| task.task_id == second));
        core.shutdown().await.expect("shutdown");
    }

    /// R-03：并发完成多个任务后重载，快照必须包含全部终态（旧实现的
    /// 固定 json.tmp 会被并发写争用，新实现串行化快照到提交）。
    #[tokio::test]
    async fn concurrent_task_finishes_all_persist() {
        let (mut core, dir) = crate::testsupport::mock_core(Vec::new()).await;
        core.open_control_plane(dir.path()).expect("control");
        let core = std::sync::Arc::new(core);
        let mut ids = Vec::new();
        for _ in 0..8 {
            ids.push(
                core.tasks_register(pawork_domain::TaskKind::Agent)
                    .expect("register"),
            );
        }
        let mut handles = Vec::new();
        for id in ids.clone() {
            let core = std::sync::Arc::clone(&core);
            handles.push(std::thread::spawn(move || {
                core.tasks_finish_from_run(&id, pawork_domain::TaskStatus::Completed, None)
                    .expect("finish");
            }));
        }
        for handle in handles {
            handle.join().expect("join");
        }
        assert!(
            core.tasks.take_last_degrade().is_none(),
            "all persists must succeed under concurrency"
        );

        let reloaded =
            crate::tasks_host::load_task_manager(&dir.path().join("tasks.json")).expect("reload");
        let tasks = reloaded.tasks();
        for id in &ids {
            let task = tasks
                .iter()
                .find(|task| &task.task_id == id)
                .expect("task persisted");
            assert_eq!(task.status, pawork_domain::TaskStatus::Completed);
        }
    }

    /// R-03：替换失败时目标路径内容不被破坏，且不留本次的临时文件。
    #[tokio::test]
    async fn failed_save_preserves_target_and_leaves_no_temp() {
        let (mut core, dir) = crate::testsupport::mock_core(Vec::new()).await;
        core.open_control_plane(dir.path()).expect("control");
        core.tasks_register(pawork_domain::TaskKind::Agent)
            .expect("register first");

        // 用一个非空目录顶住 tasks.json，使 rename 替换失败。
        let tasks_path = dir.path().join("tasks.json");
        std::fs::remove_file(&tasks_path).expect("remove snapshot file");
        std::fs::create_dir(&tasks_path).expect("block with directory");
        std::fs::write(tasks_path.join("marker"), b"previous").expect("marker");

        let error = core
            .tasks_register(pawork_domain::TaskKind::Automation)
            .expect_err("persist failure must surface");
        let _ = error;

        assert_eq!(
            std::fs::read(tasks_path.join("marker")).expect("marker survives"),
            b"previous",
            "failed replace must not damage the previous path contents"
        );
        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .expect("read dir")
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(
            leftovers.is_empty(),
            "failed save must clean up its own temp file: {leftovers:?}"
        );
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
        core.tasks
            .tasks_finish_from_run(&id, pawork_domain::TaskStatus::Completed, None)
            .expect("finish succeeds even if persist fails");
        let degrade = core
            .tasks
            .take_last_degrade()
            .expect("persist failure must yield DegradeEvent");
        assert_eq!(degrade.code(), "degrade.tasks_finish_failed");
        assert_eq!(degrade.kind, pawork_domain::DegradeKind::TasksFinishFailed);
        assert_eq!(degrade.severity, pawork_domain::DegradeSeverity::Error);
        assert_eq!(degrade.details["task_id"], serde_json::json!(id.as_str()));
        assert!(degrade.details.get("error").is_some());
        core.shutdown().await.expect("shutdown");
    }

    /// R-23：孤儿任务按所有权收口——活跃宿主存在时旁观者不误扫；
    /// 所有者退出后新宿主收为 Failed（interrupted），重复恢复幂等。
    #[tokio::test]
    async fn orphan_tasks_sealed_only_after_owner_exits() {
        use pawork_auth::{MemoryBackend, SecretBackend};
        use pawork_workspace::config::PaworkConfig;

        let dir = tempfile::tempdir().expect("tempdir");
        let backend: std::sync::Arc<dyn SecretBackend> = std::sync::Arc::new(MemoryBackend::new());
        let new_core = || {
            let backend = backend.clone();
            async move {
                crate::AppCore::from_config_inner(
                    PaworkConfig::default(),
                    None,
                    None,
                    backend,
                    true,
                )
                .await
                .expect("core")
            }
        };

        // 活跃宿主：Executor 所有权下登记 Running Agent 任务，不落终态即退出。
        let mut host = new_core().await;
        host.set_instance_ownership(
            crate::instance_lock::acquire_instance_ownership(
                dir.path(),
                crate::InstanceRole::Executor,
            )
            .expect("host ownership"),
        );
        // 与生产 load_with 同序：Run 与 Task 恢复全部完成后才释放清扫互斥。
        host.open_store(dir.path().join("session.db"))
            .await
            .expect("host store");
        assert!(
            pawork_auth::try_acquire_file_lock(&dir.path().join("sweep.lock"))
                .expect("probe sweep lock")
                .is_none(),
            "Task 恢复前不得让其他执行宿主开始登记"
        );
        host.open_control_plane(dir.path()).expect("host control");
        assert!(
            pawork_auth::try_acquire_file_lock(&dir.path().join("sweep.lock"))
                .expect("probe released sweep lock")
                .is_some()
        );
        let task_id = host
            .tasks_register(pawork_domain::TaskKind::Agent)
            .expect("register");
        assert_eq!(
            host.tasks_status(task_id.as_str()).expect("status").status,
            pawork_domain::TaskStatus::Running
        );

        // 旁观 Executor：宿主仍活跃时取得所有权但无清扫权，不得收口任务。
        let mut bystander = new_core().await;
        bystander.set_instance_ownership(
            crate::instance_lock::acquire_instance_ownership(
                dir.path(),
                crate::InstanceRole::Executor,
            )
            .expect("bystander ownership"),
        );
        bystander
            .open_control_plane(dir.path())
            .expect("bystander control");
        assert_eq!(
            bystander
                .tasks_status(task_id.as_str())
                .expect("status")
                .status,
            pawork_domain::TaskStatus::Running,
            "活跃宿主存在时旁观者不得收口其任务"
        );
        drop(bystander);

        // 所有者退出（登记锁随 drop 释放）后，新所有者收口为
        // Failed(interrupted)；再来一个所有者重复恢复幂等。
        drop(host);
        for _ in 0..2 {
            let mut recovery = new_core().await;
            recovery.set_instance_ownership(
                crate::instance_lock::acquire_instance_ownership(
                    dir.path(),
                    crate::InstanceRole::Executor,
                )
                .expect("recovery ownership"),
            );
            recovery
                .open_control_plane(dir.path())
                .expect("recovery control");
            let status = recovery.tasks_status(task_id.as_str()).expect("status");
            assert_eq!(status.status, pawork_domain::TaskStatus::Failed);
            assert!(
                status
                    .detail
                    .as_deref()
                    .is_some_and(|detail| detail.contains("interrupted")),
                "orphan seal must record interrupted detail: {:?}",
                status.detail
            );
            drop(recovery);
        }
    }
}
