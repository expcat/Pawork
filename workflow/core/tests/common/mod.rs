//! 集成测试公共辅助：mock 沙箱后端与常用构造。

use std::sync::{Arc, Mutex};

use pawork_exec::CancellationToken;
use async_trait::async_trait;
use pawork_exec::{
    SandboxBackend, SandboxError, SandboxPolicy, SandboxProcess, SandboxProcessSpec,
};
use pawork_workflow::task::TaskManager;

/// 记录每次 spawn 请求并一律拒绝的 mock 后端（无法构造真实
/// `SandboxProcess`，用于 policy 透传与失败清理断言）。
#[derive(Clone, Default)]
pub struct RecordingBackend {
    calls: Arc<Mutex<Vec<(SandboxProcessSpec, SandboxPolicy)>>>,
}

impl RecordingBackend {
    pub fn new() -> Self {
        Self::default()
    }

    #[allow(dead_code)] // 仅 process_and_policy 测试二进制使用。
    pub fn calls(&self) -> Vec<(SandboxProcessSpec, SandboxPolicy)> {
        self.calls.lock().unwrap().clone()
    }
}

#[async_trait]
impl SandboxBackend for RecordingBackend {
    fn id(&self) -> &'static str {
        "recording_mock"
    }

    fn available(&self) -> bool {
        true
    }

    async fn spawn(
        &self,
        spec: SandboxProcessSpec,
        policy: SandboxPolicy,
        _cancel: CancellationToken,
    ) -> Result<SandboxProcess, SandboxError> {
        self.calls.lock().unwrap().push((spec, policy));
        Err(SandboxError::Denied("mock denies all spawns".into()))
    }
}

/// 带 RecordingBackend 的 TaskManager（状态机 / 重放类测试用）。
pub fn manager_with_recording_backend() -> (TaskManager, RecordingBackend) {
    manager_with_recording_backend_capacity(256)
}

/// 带 RecordingBackend 与自定义广播容量的 TaskManager。
pub fn manager_with_recording_backend_capacity(capacity: usize) -> (TaskManager, RecordingBackend) {
    let backend = RecordingBackend::new();
    let manager = TaskManager::with_backend_capacity(Box::new(backend.clone()), capacity);
    (manager, backend)
}
