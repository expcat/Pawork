//! 声明式监视循环服务（P16-6）。
//!
//! [`MonitorService`] 注册 monitors，命中产出 canonical [`MonitorEvent::Triggered`]
//! 事件，可作为 P16-5 automation 的 `event` 触发器来源；同时是 P17-2 Plugin
//! Package Monitors 声明的唯一运行时执行点（package manifest 只声明配置，
//! 进入本服务执行）。
//!
//! ## 进程统一所有权（硬约束）
//!
//! monitor 若需启动子进程，**禁止**直接 `tokio::process::Command` /
//! `std::process::Command`；必须经注入的 [`TaskManager`]（其内部已走
//! SandboxBackend -> ProcessRuntime）。monitor-service 不自复制进程树清理、
//! 不自定 sandbox policy。tests/service.rs 断言本 crate 源码无直连 Command。
//!
//! ## 断连续存与重放
//!
//! monitor 注册到 task-manager 为 [`TaskKind::Monitor`]；视图可经 task-manager
//! 的 snapshot+replay 恢复，monitor-service 自身的状态折叠（[`apply`]
//! [`MonitorEvent`]）亦提供独立重放入口。

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use agent_domain::{BackgroundTaskId, MonitorEvent, MonitorId, TaskKind, TaskStatus};
use agent_events::AgentEvent;
use task_manager::TaskManager;
use tokio::sync::broadcast;

use crate::config::{Monitor, Observation};
use crate::error::MonitorServiceError;
use crate::evaluate::evaluate as evaluate_config;
use crate::state::{MonitorRecord, MonitorServiceSnapshot, MonitorServiceState};

/// 实时事件广播默认容量；超出后订阅者收到 `Lagged`，应 snapshot+replay 重连。
const DEFAULT_BROADCAST_CAPACITY: usize = 256;

struct MonitorServiceInner {
    /// 已注册 monitor 的声明式配置（evaluate 查找用，in-memory）。
    configs: Mutex<BTreeMap<MonitorId, Monitor>>,
    /// 事件折叠状态（snapshot / replay 入口）。
    state: Mutex<MonitorServiceState>,
    /// monitor_id -> task-manager 后台任务（断连续存车辆，可选）。
    tasks: Mutex<BTreeMap<MonitorId, BackgroundTaskId>>,
    /// 注入的 task-manager；缺失时 monitor-service 仍可独立做 evaluate / 重放。
    task_manager: Option<TaskManager>,
    /// 实时事件流（`AgentEvent::Monitor(...)`）。
    live: broadcast::Sender<AgentEvent>,
}

/// 声明式监视循环服务（可克隆，内部 Arc 共享）。
#[derive(Clone)]
pub struct MonitorService {
    inner: Arc<MonitorServiceInner>,
}

impl MonitorService {
    fn build(capacity: usize, task_manager: Option<TaskManager>) -> Self {
        let (live, _) = broadcast::channel(capacity.max(1));
        Self {
            inner: Arc::new(MonitorServiceInner {
                configs: Mutex::new(BTreeMap::new()),
                state: Mutex::new(MonitorServiceState::new()),
                tasks: Mutex::new(BTreeMap::new()),
                task_manager,
                live,
            }),
        }
    }

    /// 构造无 task-manager 的服务（纯 evaluate / 重放用）。
    pub fn new() -> Self {
        Self::build(DEFAULT_BROADCAST_CAPACITY, None)
    }

    /// 同 [`MonitorService::new`]，可自定义实时事件广播容量（测试用）。
    pub fn with_capacity(capacity: usize) -> Self {
        Self::build(capacity, None)
    }

    /// 注入 task-manager：monitor 将注册为 `TaskKind::Monitor`，复用其断连续存。
    pub fn with_task_manager(task_manager: TaskManager) -> Self {
        Self::build(DEFAULT_BROADCAST_CAPACITY, Some(task_manager))
    }

    /// 注入 task-manager 并指定广播容量。
    pub fn with_task_manager_capacity(task_manager: TaskManager, capacity: usize) -> Self {
        Self::build(capacity, Some(task_manager))
    }

    /// 订阅实时事件流（`AgentEvent::Monitor(...)`）。
    pub fn subscribe(&self) -> broadcast::Receiver<AgentEvent> {
        self.inner.live.subscribe()
    }

    /// 注册 monitor：校验配置自洽，存 in-memory 配置；若注入了 task-manager，
    /// 同步注册为 `TaskKind::Monitor` 后台任务（状态 Queued）。
    pub fn register(
        &self,
        monitor: Monitor,
        parent_task_id: Option<BackgroundTaskId>,
    ) -> Result<MonitorId, MonitorServiceError> {
        monitor
            .config
            .validate()
            .map_err(MonitorServiceError::InvalidConfig)?;
        let id = monitor.monitor_id.clone();
        // 持有配置锁完成「查重 → task 注册 → 配置插入」，确保并发重复注册在
        // 创建第二个 task 前被拒绝。task 注册失败时配置仍不落地。
        let mut configs = self.inner.configs.lock().unwrap();
        if configs.contains_key(&id) {
            return Err(MonitorServiceError::AlreadyRegistered(id));
        }
        if let Some(task_manager) = &self.inner.task_manager {
            let task_id = task_manager.register(TaskKind::Monitor, parent_task_id)?;
            self.inner.tasks.lock().unwrap().insert(id.clone(), task_id);
        }
        configs.insert(id.clone(), monitor);
        Ok(id)
    }

    /// 启动 monitor：发出 `MonitorEvent::Started`；若注入了 task-manager，
    /// 同步 `start` 对应后台任务（Queued -> Running）。
    pub fn start(&self, monitor_id: &MonitorId) -> Result<MonitorEvent, MonitorServiceError> {
        let monitor = self
            .config(monitor_id)
            .ok_or_else(|| MonitorServiceError::UnknownMonitor(monitor_id.clone()))?;
        // 先推进 task-manager 镜像再发 Started：task start 失败时不广播权威事件、
        // 不推进 monitor 状态（避免「先广播 Started 再 task start 失败」的分叉）。
        if let Some(task_manager) = &self.inner.task_manager {
            if let Some(task_id) = self.task_id_of(monitor_id) {
                task_manager.start(&task_id)?;
            }
        }
        let event = MonitorEvent::Started {
            monitor_id: monitor_id.clone(),
            source: monitor.source,
            workspace_id: monitor.workspace_id.clone(),
        };
        self.apply_and_broadcast(event.clone());
        Ok(event)
    }

    /// 确定性触发判定入口：查找 monitor 配置并以注入的观测样本做纯函数判定；
    /// 命中则发出 `MonitorEvent::Triggered` 并返回 detail。可作为 automation
    /// `event` 触发器来源（Triggered 事件经 broadcast 与持久化可被消费）。
    pub fn evaluate(
        &self,
        monitor_id: &MonitorId,
        observation: &Observation,
    ) -> Result<Option<String>, MonitorServiceError> {
        let monitor = self
            .config(monitor_id)
            .ok_or_else(|| MonitorServiceError::UnknownMonitor(monitor_id.clone()))?;
        let detail = evaluate_config(&monitor.config, observation);
        if let Some(detail) = &detail {
            let event = MonitorEvent::Triggered {
                monitor_id: monitor_id.clone(),
                detail: detail.clone(),
            };
            self.apply_and_broadcast(event);
        }
        Ok(detail)
    }

    /// 停止 monitor：发出 `MonitorEvent::Stopped`；若注入了 task-manager，
    /// best-effort 把对应后台任务 finish 为 Completed（任务簿记，失败不回滚
    /// monitor 域的权威 Stopped 事件）。
    pub fn stop(
        &self,
        monitor_id: &MonitorId,
        reason: Option<String>,
    ) -> Result<MonitorEvent, MonitorServiceError> {
        if self.config(monitor_id).is_none() {
            return Err(MonitorServiceError::UnknownMonitor(monitor_id.clone()));
        }
        let event = MonitorEvent::Stopped {
            monitor_id: monitor_id.clone(),
            reason: reason.clone(),
        };
        self.apply_and_broadcast(event.clone());
        if let Some(task_manager) = &self.inner.task_manager {
            if let Some(task_id) = self.task_id_of(monitor_id) {
                let _ = task_manager.finish(&task_id, TaskStatus::Completed, reason);
            }
        }
        Ok(event)
    }

    /// 注销 monitor：从配置表与 task 映射中移除，发出
    /// `MonitorEvent::Unregistered` 从视图抹掉记录，并 best-effort 经
    /// task-manager `cancel` 终止后台任务（Queued 静默移除；Running/Suspended
    /// 发 Canceled；已 Completed 的 cancel 是 no-op）。未知 id 一律 fail-closed
    /// 为 [`MonitorServiceError::UnknownMonitor`]。成功后同一 id 可再次
    /// [`register`]，重新 `start` 时累计字段从零开始。
    pub fn unregister(
        &self,
        monitor_id: &MonitorId,
    ) -> Result<MonitorEvent, MonitorServiceError> {
        let mut configs = self.inner.configs.lock().unwrap();
        if configs.remove(monitor_id).is_none() {
            return Err(MonitorServiceError::UnknownMonitor(monitor_id.clone()));
        }
        let task_id = self.inner.tasks.lock().unwrap().remove(monitor_id);
        drop(configs);
        let event = MonitorEvent::Unregistered {
            monitor_id: monitor_id.clone(),
        };
        self.apply_and_broadcast(event.clone());
        if let (Some(task_manager), Some(task_id)) = (&self.inner.task_manager, task_id) {
            let _ = task_manager.cancel(&task_id);
        }
        Ok(event)
    }

    /// 重放 canonical 事件序列，重建 monitor 视图（断连 / 重启恢复入口）。
    /// 不重复广播；事件本身应由调用方持久化（经 `AgentEvent::Monitor`）。
    pub fn replay(&self, events: impl IntoIterator<Item = MonitorEvent>) -> usize {
        self.inner.state.lock().unwrap().replay(events)
    }

    /// 只读：整体快照（monitor 视图 + 事件日志）。
    pub fn snapshot(&self) -> MonitorServiceSnapshot {
        self.inner.state.lock().unwrap().snapshot()
    }

    /// 只读：单个 monitor 快照。
    pub fn record(&self, monitor_id: &MonitorId) -> Option<MonitorRecord> {
        self.inner.state.lock().unwrap().record(monitor_id).cloned()
    }

    /// 只读：全部 monitor 快照。
    pub fn records(&self) -> Vec<MonitorRecord> {
        self.inner.state.lock().unwrap().records()
    }

    /// 只读：完整事件日志。
    pub fn event_log(&self) -> Vec<MonitorEvent> {
        self.inner.state.lock().unwrap().event_log()
    }

    /// 只读：monitor 的声明式配置（in-memory）。
    pub fn config(&self, monitor_id: &MonitorId) -> Option<Monitor> {
        self.inner.configs.lock().unwrap().get(monitor_id).cloned()
    }

    /// 只读：monitor 对应的 task-manager 后台任务 ID（若注册过）。
    pub fn monitor_task_id(&self, monitor_id: &MonitorId) -> Option<BackgroundTaskId> {
        self.task_id_of(monitor_id)
    }

    /// 只读：注入的 task-manager 引用（若有）。
    pub fn task_manager(&self) -> Option<&TaskManager> {
        self.inner.task_manager.as_ref()
    }

    fn task_id_of(&self, monitor_id: &MonitorId) -> Option<BackgroundTaskId> {
        self.inner.tasks.lock().unwrap().get(monitor_id).cloned()
    }

    fn apply_and_broadcast(&self, event: MonitorEvent) {
        self.inner.state.lock().unwrap().apply(&event);
        // 广播失败仅表示无订阅者，忽略。
        let _ = self.inner.live.send(AgentEvent::Monitor(event));
    }
}

impl Default for MonitorService {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::MonitorConfig;
    use agent_domain::MonitorSourceKind;

    fn port_monitor(id: &str) -> Monitor {
        Monitor::new(
            id,
            MonitorConfig::PortState {
                host: "127.0.0.1".into(),
                port: 8080,
            },
        )
    }

    #[test]
    fn lifecycle_emits_started_triggered_stopped() {
        let svc = MonitorService::new();
        let id = svc.register(port_monitor("m1"), None).unwrap();
        let started = svc.start(&id).unwrap();
        assert!(matches!(
            started,
            MonitorEvent::Started {
                source: MonitorSourceKind::PortState,
                ..
            }
        ));

        let detail = svc
            .evaluate(
                &id,
                &Observation::PortState {
                    host: "127.0.0.1".into(),
                    port: 8080,
                    open: true,
                },
            )
            .unwrap();
        assert_eq!(detail.as_deref(), Some("port 8080 open"));

        let rec = svc.record(&id).unwrap();
        assert_eq!(rec.trigger_count, 1);
        assert_eq!(rec.last_detail.as_deref(), Some("port 8080 open"));

        let stopped = svc.stop(&id, Some("done".into())).unwrap();
        assert!(matches!(stopped, MonitorEvent::Stopped { .. }));
        assert_eq!(svc.event_log().len(), 3);
    }

    #[test]
    fn replay_rebuilds_view_without_rebroadcast() {
        let svc = MonitorService::new();
        let id = svc.register(port_monitor("m1"), None).unwrap();
        svc.start(&id).unwrap();
        svc.evaluate(
            &id,
            &Observation::PortState {
                host: "127.0.0.1".into(),
                port: 8080,
                open: true,
            },
        )
        .unwrap();
        let log = svc.event_log();

        let svc2 = MonitorService::new();
        let count = svc2.replay(log);
        assert_eq!(count, 2);
        let rec = svc2.record(&id).unwrap();
        assert_eq!(rec.trigger_count, 1);
        assert_eq!(rec.last_detail.as_deref(), Some("port 8080 open"));
    }

    #[test]
    fn evaluate_unknown_monitor_errors() {
        let svc = MonitorService::new();
        let err = svc
            .evaluate(
                &MonitorId::new("ghost"),
                &Observation::PortState {
                    host: "h".into(),
                    port: 1,
                    open: true,
                },
            )
            .unwrap_err();
        assert!(matches!(err, MonitorServiceError::UnknownMonitor(_)));
    }

    #[test]
    fn register_rejects_invalid_config() {
        let svc = MonitorService::new();
        let bad = Monitor::new(
            "m",
            MonitorConfig::ProcessExit {
                pid: None,
                task_id: None,
            },
        );
        assert!(svc.register(bad, None).is_err());
    }
}
