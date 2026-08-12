//! Monitor 聚合状态与纯函数折叠（event-sourcing，P16-6）。
//!
//! [`MonitorServiceState`] 保存 monitor 视图与 in-memory 事件日志，是重放 /
//! 恢复的唯一入口：[`MonitorServiceState::apply`] 把一个 canonical
//! [`MonitorEvent`] 折叠进状态，事件序列可无损重建 monitor 视图。命令方法
//! （见 [`crate::MonitorService`]）先 apply 再广播，保证状态变化可持久化可重放。

use std::collections::BTreeMap;

use agent_domain::{MonitorEvent, MonitorId, MonitorSourceKind, WorkspaceId};
use serde::{Deserialize, Serialize};

/// Monitor 生命周期状态（事件折叠得到）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MonitorStatus {
    /// 默认值：已知配置但尚未发出 Started（重放中瞬态）。
    #[default]
    Registered,
    Running,
    Stopped,
}

/// 单个 monitor 的只读快照（serde 可序列化，用于 snapshot / 重连恢复）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MonitorRecord {
    pub monitor_id: MonitorId,
    pub source: MonitorSourceKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<WorkspaceId>,
    pub status: MonitorStatus,
    /// 累计命中次数（Triggered 折叠累加）。
    pub trigger_count: u64,
    /// 最近一次命中 detail。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_detail: Option<String>,
    /// Stopped 原因。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<String>,
}

/// 监视服务的整体快照：monitor 视图 + 完整事件日志（重放输入）。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MonitorServiceSnapshot {
    pub monitors: Vec<MonitorRecord>,
    pub events: Vec<MonitorEvent>,
}

/// 监视服务聚合状态：monitor 表 + 事件日志（只追加）。
#[derive(Clone, Debug, Default)]
pub struct MonitorServiceState {
    monitors: BTreeMap<MonitorId, MonitorRecord>,
    log: Vec<MonitorEvent>,
}

impl MonitorServiceState {
    /// 空状态。
    pub fn new() -> Self {
        Self::default()
    }

    /// 纯函数折叠：把 canonical 事件应用到当前状态并追加日志。
    ///
    /// `Started` 幂等（已存在则刷新为 Running）；`Triggered` / `Stopped`
    /// 对未知 monitor 防御性记日志不报错（保证重放健壮）。
    pub fn apply(&mut self, event: &MonitorEvent) {
        match event {
            MonitorEvent::Started {
                monitor_id,
                source,
                workspace_id,
            } => {
                let record =
                    self.monitors
                        .entry(monitor_id.clone())
                        .or_insert_with(|| MonitorRecord {
                            monitor_id: monitor_id.clone(),
                            source: *source,
                            workspace_id: workspace_id.clone(),
                            status: MonitorStatus::Registered,
                            trigger_count: 0,
                            last_detail: None,
                            stop_reason: None,
                        });
                record.source = *source;
                record.workspace_id = workspace_id.clone();
                record.status = MonitorStatus::Running;
                record.stop_reason = None;
            }
            MonitorEvent::Triggered { monitor_id, detail } => {
                if let Some(record) = self.monitors.get_mut(monitor_id) {
                    record.trigger_count = record.trigger_count.saturating_add(1);
                    record.last_detail = Some(detail.clone());
                }
            }
            MonitorEvent::Stopped { monitor_id, reason } => {
                if let Some(record) = self.monitors.get_mut(monitor_id) {
                    record.status = MonitorStatus::Stopped;
                    record.stop_reason = reason.clone();
                }
            }
        }
        self.log.push(event.clone());
    }

    /// 重放：把事件序列折叠进状态，返回折叠条数。
    pub fn replay(&mut self, events: impl IntoIterator<Item = MonitorEvent>) -> usize {
        let mut count = 0;
        for event in events {
            self.apply(&event);
            count += 1;
        }
        count
    }

    /// 只读：单个 monitor 快照。
    pub fn record(&self, monitor_id: &MonitorId) -> Option<&MonitorRecord> {
        self.monitors.get(monitor_id)
    }

    /// 只读：全部 monitor 快照（按 ID 排序，确定性输出）。
    pub fn records(&self) -> Vec<MonitorRecord> {
        self.monitors.values().cloned().collect()
    }

    /// 只读：完整事件日志。
    pub fn event_log(&self) -> Vec<MonitorEvent> {
        self.log.clone()
    }

    /// 只读：monitor 视图 + 完整事件日志（断连恢复输入）。
    pub fn snapshot(&self) -> MonitorServiceSnapshot {
        MonitorServiceSnapshot {
            monitors: self.records(),
            events: self.log.clone(),
        }
    }

    /// 是否无任何 monitor。
    pub fn is_empty(&self) -> bool {
        self.monitors.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn started(id: &str, source: MonitorSourceKind) -> MonitorEvent {
        MonitorEvent::Started {
            monitor_id: MonitorId::new(id),
            source,
            workspace_id: None,
        }
    }

    #[test]
    fn apply_started_then_triggered_then_stopped() {
        let mut state = MonitorServiceState::new();
        state.apply(&started("m1", MonitorSourceKind::FileChange));
        state.apply(&MonitorEvent::Triggered {
            monitor_id: MonitorId::new("m1"),
            detail: "file changed: /a".into(),
        });
        state.apply(&MonitorEvent::Triggered {
            monitor_id: MonitorId::new("m1"),
            detail: "file changed: /b".into(),
        });
        state.apply(&MonitorEvent::Stopped {
            monitor_id: MonitorId::new("m1"),
            reason: Some("done".into()),
        });

        let rec = state.record(&MonitorId::new("m1")).unwrap();
        assert_eq!(rec.status, MonitorStatus::Stopped);
        assert_eq!(rec.trigger_count, 2);
        assert_eq!(rec.last_detail.as_deref(), Some("file changed: /b"));
        assert_eq!(rec.stop_reason.as_deref(), Some("done"));
        assert_eq!(state.event_log().len(), 4);
    }

    #[test]
    fn replay_reconstructs_view() {
        let events = vec![
            started("m1", MonitorSourceKind::RegexMatch),
            MonitorEvent::Triggered {
                monitor_id: MonitorId::new("m1"),
                detail: "regex matched: x".into(),
            },
            MonitorEvent::Stopped {
                monitor_id: MonitorId::new("m1"),
                reason: None,
            },
        ];
        let mut state = MonitorServiceState::new();
        let count = state.replay(events.clone());
        assert_eq!(count, 3);
        let rec = state.record(&MonitorId::new("m1")).unwrap();
        assert_eq!(rec.status, MonitorStatus::Stopped);
        assert_eq!(rec.trigger_count, 1);
        assert_eq!(state.event_log(), events);
    }

    #[test]
    fn defensive_apply_on_unknown_monitor() {
        let mut state = MonitorServiceState::new();
        // 对未 Started 的 monitor 触发 / 停止：记日志但不 panic、不创建记录。
        state.apply(&MonitorEvent::Triggered {
            monitor_id: MonitorId::new("ghost"),
            detail: "x".into(),
        });
        state.apply(&MonitorEvent::Stopped {
            monitor_id: MonitorId::new("ghost"),
            reason: None,
        });
        assert!(state.record(&MonitorId::new("ghost")).is_none());
        assert_eq!(state.event_log().len(), 2);
    }
}
