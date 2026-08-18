//! Automation 聚合状态与 event-sourcing 折叠（`apply` / `replay`）。
//!
//! [`apply`] 是恢复入口：把一个 canonical [`AutomationEvent`] 纯函数式折叠进
//! [`AutomationState`]。事件被视为已校验的「事实」，`apply` 不再重复命令面校验
//! （校验由 [`crate::automation::AutomationEngine`] 完成）；崩溃后重放事件序列即可重建
//! 已注册集合、触发计数、归档结果与挂起状态。
//!
//! 注意：canonical 事件只携带触发器 *种类*（[`AutomationTriggerKind`]）与轻量
//! 事实，完整配置（cron 表达式 / 间隔 / 模式 / 动作）与 inbox *状态*、失败计数
//! 不在事件中——它们是命令侧视图，由 engine 持有。

use std::collections::BTreeMap;

use pawork_domain::{
    ArtifactId, AutomationEvent, AutomationId, AutomationTriggerKind, BackgroundTaskId, RunId,
};
use serde::{Deserialize, Serialize};

/// 一条已归档的结果（对应 `AutomationEvent::ResultArchived`）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArchivedResult {
    pub automation_id: AutomationId,
    pub artifact_id: ArtifactId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<RunId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<BackgroundTaskId>,
}

/// 单条 automation 的事件溯源视图。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AutomationView {
    /// 是否已注册（`Registered` 出现过）。
    pub registered: bool,
    pub trigger: Option<AutomationTriggerKind>,
    /// 触发次数（`Triggered` 计数）。
    pub fired: u64,
    /// 最近一次派发的 background task。
    pub last_task: Option<BackgroundTaskId>,
    /// 挂起原因（`Suspended` 设置；`Registered` 重置）。
    pub suspended_reason: Option<String>,
}

/// Automation 聚合状态：注册表 + 触发计数 + 归档结果 + 事件日志。
#[derive(Clone, Debug, Default)]
pub struct AutomationState {
    views: BTreeMap<AutomationId, AutomationView>,
    archived: Vec<ArchivedResult>,
    log: Vec<AutomationEvent>,
}

impl AutomationState {
    pub fn new() -> Self {
        Self::default()
    }

    /// 纯函数折叠：把 canonical 事件应用到当前状态并追加日志。
    pub fn apply(&mut self, event: &AutomationEvent) {
        match event {
            AutomationEvent::Registered {
                automation_id,
                trigger,
            } => {
                let view = self.views.entry(automation_id.clone()).or_default();
                view.registered = true;
                view.trigger = Some(*trigger);
                view.suspended_reason = None;
            }
            AutomationEvent::Triggered {
                automation_id,
                task_id,
            } => {
                let view = self.views.entry(automation_id.clone()).or_default();
                view.registered = true;
                view.fired = view.fired.saturating_add(1);
                view.last_task = Some(task_id.clone());
            }
            AutomationEvent::ResultArchived {
                automation_id,
                artifact_id,
                run_id,
                task_id,
            } => {
                self.archived.push(ArchivedResult {
                    automation_id: automation_id.clone(),
                    artifact_id: artifact_id.clone(),
                    run_id: run_id.clone(),
                    task_id: task_id.clone(),
                });
            }
            AutomationEvent::Suspended {
                automation_id,
                reason,
            } => {
                let view = self.views.entry(automation_id.clone()).or_default();
                view.suspended_reason = Some(reason.clone());
            }
        }
        self.log.push(event.clone());
    }

    /// 只读：单条 automation 视图。
    pub fn view(&self, id: &AutomationId) -> Option<&AutomationView> {
        self.views.get(id)
    }

    /// 只读：全部已注册 automation 的视图（按 ID 排序）。
    pub fn views(&self) -> Vec<&AutomationView> {
        self.views.values().collect()
    }

    /// 只读：已注册的 automation ID（按 ID 排序）。
    pub fn registered_ids(&self) -> Vec<AutomationId> {
        self.views
            .iter()
            .filter(|(_, v)| v.registered)
            .map(|(id, _)| id.clone())
            .collect()
    }

    /// 只读：某 automation 的触发次数。
    pub fn fired_count(&self, id: &AutomationId) -> u64 {
        self.views.get(id).map(|v| v.fired).unwrap_or(0)
    }

    /// 只读：`task_id` 是否由该 automation 的 canonical `Triggered` 事件产生。
    pub fn was_triggered_task(
        &self,
        automation_id: &AutomationId,
        task_id: &BackgroundTaskId,
    ) -> bool {
        self.log.iter().any(|event| {
            matches!(
                event,
                AutomationEvent::Triggered {
                    automation_id: owner,
                    task_id: triggered,
                } if owner == automation_id && triggered == task_id
            )
        })
    }

    /// 只读：归档结果列表。
    pub fn archived(&self) -> &[ArchivedResult] {
        &self.archived
    }

    /// 同一 `(automation_id, task_id)` 是否已有归档（S13-F28 幂等键）。
    pub fn has_archived_task(
        &self,
        automation_id: &AutomationId,
        task_id: &BackgroundTaskId,
    ) -> bool {
        self.archived.iter().any(|item| {
            item.automation_id == *automation_id && item.task_id.as_ref() == Some(task_id)
        })
    }

    /// 只读：某 automation 是否处于挂起态。
    pub fn is_suspended(&self, id: &AutomationId) -> bool {
        self.views
            .get(id)
            .and_then(|v| v.suspended_reason.as_ref())
            .is_some()
    }

    /// 只读：完整事件日志（重放输入 / 持久化镜像）。
    pub fn event_log(&self) -> &[AutomationEvent] {
        &self.log
    }

    /// 只读：`seq` 之后的增量事件（重连续读）。
    pub fn events_since(&self, seq: usize) -> Vec<AutomationEvent> {
        self.log.get(seq..).unwrap_or_default().to_vec()
    }
}

/// 从事件序列重放重建 [`AutomationState`]（逐步 [`AutomationState::apply`]）。
pub fn replay<'a>(events: impl IntoIterator<Item = &'a AutomationEvent>) -> AutomationState {
    let mut state = AutomationState::new();
    for event in events {
        state.apply(event);
    }
    state
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apply_registered_records_kind_and_clears_suspend() {
        let mut state = AutomationState::new();
        state.apply(&AutomationEvent::Suspended {
            automation_id: AutomationId::from("a1"),
            reason: "boom".into(),
        });
        state.apply(&AutomationEvent::Registered {
            automation_id: AutomationId::from("a1"),
            trigger: AutomationTriggerKind::Cron,
        });
        let view = state.view(&AutomationId::from("a1")).unwrap();
        assert!(view.registered);
        assert_eq!(view.trigger, Some(AutomationTriggerKind::Cron));
        assert!(view.suspended_reason.is_none(), "Registered clears suspend");
    }

    #[test]
    fn replay_matches_live_apply() {
        let id = AutomationId::from("a1");
        let events = vec![
            AutomationEvent::Registered {
                automation_id: id.clone(),
                trigger: AutomationTriggerKind::Interval,
            },
            AutomationEvent::Triggered {
                automation_id: id.clone(),
                task_id: BackgroundTaskId::from("t1"),
            },
            AutomationEvent::Triggered {
                automation_id: id.clone(),
                task_id: BackgroundTaskId::from("t2"),
            },
            AutomationEvent::ResultArchived {
                automation_id: id.clone(),
                artifact_id: ArtifactId::from("art1"),
                run_id: None,
                task_id: Some(BackgroundTaskId::from("t1")),
            },
        ];

        // 逐步 apply。
        let mut live = AutomationState::new();
        for event in &events {
            live.apply(event);
        }
        // 一次性 replay。
        let replayed = replay(events.iter());

        assert_eq!(live.fired_count(&id), 2);
        assert_eq!(replayed.fired_count(&id), 2);
        assert_eq!(
            live.view(&id).unwrap().last_task,
            replayed.view(&id).unwrap().last_task
        );
        assert_eq!(live.archived().len(), replayed.archived().len());
        assert_eq!(live.event_log().len(), replayed.event_log().len());
    }
}
