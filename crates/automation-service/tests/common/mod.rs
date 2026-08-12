//! 测试公共辅助：记录派发的 mock dispatcher 与带 stub 后端的 TaskManager。

use std::sync::Mutex;

use agent_domain::{AutomationId, BackgroundTaskId, CancellationToken};
use async_trait::async_trait;
use process_runtime::ProcessRuntime;
use sandbox_runtime::{
    SandboxBackend, SandboxError, SandboxPolicy, SandboxProcess, SandboxProcessSpec,
};
use task_manager::TaskManager;

use automation_service::{AutomationAction, AutomationDispatcher, AutomationError};

/// 记录每次派发的 mock dispatcher，返回稳定的 `rec_task_<n>` 任务 ID。
pub struct RecordingDispatcher {
    calls: Mutex<Vec<(AutomationId, AutomationAction)>>,
}

impl Default for RecordingDispatcher {
    fn default() -> Self {
        Self::new()
    }
}

impl RecordingDispatcher {
    pub fn new() -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
        }
    }

    #[allow(dead_code)]
    pub fn calls(&self) -> Vec<(AutomationId, AutomationAction)> {
        self.calls.lock().unwrap().clone()
    }
}

impl AutomationDispatcher for RecordingDispatcher {
    fn dispatch(
        &self,
        automation_id: &AutomationId,
        action: &AutomationAction,
    ) -> Result<BackgroundTaskId, AutomationError> {
        let mut calls = self.calls.lock().unwrap();
        let n = calls.len();
        calls.push((automation_id.clone(), action.clone()));
        Ok(BackgroundTaskId::new(format!("rec_task_{n}")))
    }
}

/// 永远拒绝 spawn 的 stub 后端；automation 派发只走 register + start，不会 spawn。
struct StubBackend;

#[async_trait]
impl SandboxBackend for StubBackend {
    fn id(&self) -> &'static str {
        "stub"
    }
    fn available(&self) -> bool {
        true
    }
    async fn spawn(
        &self,
        _spec: SandboxProcessSpec,
        _policy: SandboxPolicy,
        _cancel: CancellationToken,
    ) -> Result<SandboxProcess, SandboxError> {
        Err(SandboxError::Denied("stub denies spawn".into()))
    }
}

/// 带 stub 后端的 TaskManager（automation → task-manager 集成测试用）。
pub fn manager_with_recording_backend() -> TaskManager {
    TaskManager::with_capacity(Box::new(StubBackend), ProcessRuntime::new(), 64)
}
