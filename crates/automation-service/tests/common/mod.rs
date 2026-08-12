//! 测试公共辅助：记录派发的 mock dispatcher。

use std::sync::Mutex;

use agent_domain::{AutomationId, BackgroundTaskId};

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
