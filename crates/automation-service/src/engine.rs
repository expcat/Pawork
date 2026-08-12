//! 触发器引擎：注册 / 判定到期 / 派发 / 结果归档 / 失败退避。
//!
//! [`AutomationEngine`] 持有已注册 automation 的完整配置（命令侧）与调度状态，
//! 以注入的 `now`（Unix 秒）做**确定性**判定——不依赖真实 tokio timer，便于测试。
//! 触发与结果以 canonical [`AutomationEvent`] 产出（可持久化、可重放）；失败计数、
//! inbox 状态、下次触发时刻是命令侧视图，不在事件中。
//!
//! 外部触发器只经认证 adapter 转为 canonical 载荷字符串后进入 [`Self::match_event`]，
//! engine 不含任何平台名称分支。

use std::collections::BTreeMap;

use agent_domain::{
    ArtifactId, AutomationEvent, AutomationId, AutomationTriggerKind, BackgroundTaskId, RunId,
};
use parking_lot::Mutex;
use regex::Regex;

use crate::automation::{Automation, AutomationAction, AutomationTrigger};
use crate::cron::CronSchedule;
use crate::dispatcher::{AutomationDispatcher, DispatchOutcome};
use crate::error::AutomationError;
use crate::inbox::{InboxItem, InboxQuery, InboxStatus, ResultInbox};
use crate::state::AutomationState;

/// 引擎配置。
#[derive(Clone, Debug)]
pub struct EngineConfig {
    /// 连续失败达到该阈值后发 `Suspended` 暂停并告警。
    pub failure_threshold: u32,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            failure_threshold: 3,
        }
    }
}

/// 注册后的 automation：完整配置 + 预编译的 cron / 正则。
struct RegisteredAutomation {
    automation: Automation,
    cron_schedule: Option<CronSchedule>,
    event_regex: Option<Regex>,
}

/// 调度状态（命令侧）：下次触发时刻、连续失败计数。
///
/// 触发计数不在此重复：唯一权威是 [`AutomationState::fired_count`]（canonical
/// `Triggered` 事件折叠），避免命令侧与事件侧双份计数漂移。
struct ScheduleState {
    next_at: Option<u64>,
    failure_streak: u32,
}

/// automation 的查询面快照。
#[derive(Clone, Debug)]
pub struct AutomationSnapshot {
    pub automation_id: AutomationId,
    pub trigger: AutomationTrigger,
    pub action: AutomationAction,
    pub trigger_kind: AutomationTriggerKind,
    pub next_at: Option<u64>,
    pub fired_count: u64,
    pub failure_streak: u32,
    pub suspended: bool,
}

struct EngineInner {
    dispatcher: Box<dyn AutomationDispatcher>,
    configs: BTreeMap<AutomationId, RegisteredAutomation>,
    schedules: BTreeMap<AutomationId, ScheduleState>,
    state: AutomationState,
    inbox: ResultInbox,
    config: EngineConfig,
}

/// 触发器引擎（进程内、确定性、可重放）。
pub struct AutomationEngine {
    inner: Mutex<EngineInner>,
}

impl AutomationEngine {
    /// 构造：注入 dispatcher 与配置。
    pub fn new(dispatcher: Box<dyn AutomationDispatcher>, config: EngineConfig) -> Self {
        Self {
            inner: Mutex::new(EngineInner {
                dispatcher,
                configs: BTreeMap::new(),
                schedules: BTreeMap::new(),
                state: AutomationState::new(),
                inbox: ResultInbox::new(),
                config,
            }),
        }
    }

    /// 注册一条 automation；返回已 apply 的 [`AutomationEvent::Registered`]。
    ///
    /// 校验 cron 表达式与 event 正则；初始化下次触发时刻。
    pub fn register(
        &self,
        automation: Automation,
        now: u64,
    ) -> Result<AutomationEvent, AutomationError> {
        let mut guard = self.inner.lock();
        let inner = &mut *guard;
        let id = automation.automation_id.clone();
        if inner.configs.contains_key(&id) {
            return Err(AutomationError::AlreadyRegistered(id));
        }

        let (cron_schedule, event_regex) = compile_trigger(&automation.trigger)?;
        let next_at = compute_next_at(&automation.trigger, now, false, cron_schedule.as_ref());
        let event = AutomationEvent::Registered {
            automation_id: id.clone(),
            trigger: automation.trigger.kind(),
        };
        inner.configs.insert(
            id.clone(),
            RegisteredAutomation {
                automation,
                cron_schedule,
                event_regex,
            },
        );
        inner.schedules.insert(
            id.clone(),
            ScheduleState {
                next_at,
                failure_streak: 0,
            },
        );
        inner.state.apply(&event);
        Ok(event)
    }

    /// 判定到期：返回 `next_at <= now` 且未挂起的时间驱动 automation（按 ID 排序）。
    ///
    /// 纯读、确定性，不派发也不改状态。
    pub fn check_due(&self, now: u64) -> Vec<AutomationId> {
        let guard = self.inner.lock();
        let inner = &*guard;
        inner
            .schedules
            .iter()
            .filter(|(id, sched)| {
                sched.next_at.is_some_and(|at| at <= now) && !inner.state.is_suspended(id)
            })
            .map(|(id, _)| id.clone())
            .collect()
    }

    /// 触发单条 automation：经 dispatcher 派发为 background task，发出 `Triggered`，
    /// 更新下次触发时刻。once 触发器触发后下次时刻置 `None`。
    pub fn fire(
        &self,
        automation_id: &AutomationId,
        now: u64,
    ) -> Result<DispatchOutcome, AutomationError> {
        let mut guard = self.inner.lock();
        let inner = &mut *guard;

        if inner.state.is_suspended(automation_id) {
            let reason = inner
                .state
                .view(automation_id)
                .and_then(|v| v.suspended_reason.clone())
                .unwrap_or_else(|| "suspended".to_string());
            return Err(AutomationError::Suspended(automation_id.clone(), reason));
        }

        let registered = inner
            .configs
            .get(automation_id)
            .ok_or_else(|| AutomationError::NotRegistered(automation_id.clone()))?;

        let fired_count = inner.state.fired_count(automation_id);
        if matches!(
            registered.automation.trigger,
            AutomationTrigger::Once { .. }
        ) && fired_count > 0
        {
            return Err(AutomationError::OnceAlreadyFired(automation_id.clone()));
        }

        // 克隆派发与重算所需数据，结束 `registered` 借用，避免与后续可变借用冲突。
        let action = registered.automation.action.clone();
        let trigger = registered.automation.trigger.clone();
        let cron = registered.cron_schedule.clone();

        let task_id = inner.dispatcher.dispatch(automation_id, &action)?;

        let event = AutomationEvent::Triggered {
            automation_id: automation_id.clone(),
            task_id: task_id.clone(),
        };

        if let Some(sched) = inner.schedules.get_mut(automation_id) {
            let once_already = matches!(trigger, AutomationTrigger::Once { .. });
            sched.next_at = compute_next_at(&trigger, now, once_already, cron.as_ref());
        }

        // canonical 状态折叠 Triggered，触发计数以事件溯源为准。
        inner.state.apply(&event);
        Ok(DispatchOutcome {
            automation_id: automation_id.clone(),
            task_id,
            fired_at: now,
        })
    }

    /// 触发全部到期 automation，按 ID 排序返回每条结果（失败不中断后续）。
    pub fn dispatch_due(&self, now: u64) -> Vec<Result<DispatchOutcome, AutomationError>> {
        let due = self.check_due(now);
        due.into_iter().map(|id| self.fire(&id, now)).collect()
    }

    /// 记录一次执行结果：归档进 inbox，发出 `ResultArchived`；连续失败达阈值则
    /// 发出 `Suspended` 暂停并告警（不静默吞错）。返回全部发出的事件。
    pub fn record_result(
        &self,
        automation_id: &AutomationId,
        task_id: &BackgroundTaskId,
        artifact_id: ArtifactId,
        run_id: Option<RunId>,
        status: InboxStatus,
        now: u64,
    ) -> Result<Vec<AutomationEvent>, AutomationError> {
        let mut guard = self.inner.lock();
        let inner = &mut *guard;
        if !inner.configs.contains_key(automation_id) {
            return Err(AutomationError::NotRegistered(automation_id.clone()));
        }
        if inner.state.fired_count(automation_id) == 0 {
            return Err(AutomationError::NoFiredTask(automation_id.clone()));
        }
        if !inner.state.was_triggered_task(automation_id, task_id) {
            return Err(AutomationError::TaskNotTriggeredByAutomation {
                automation_id: automation_id.clone(),
                task_id: task_id.clone(),
            });
        }

        let mut emitted = Vec::new();

        let archived = AutomationEvent::ResultArchived {
            automation_id: automation_id.clone(),
            artifact_id: artifact_id.clone(),
            run_id: run_id.clone(),
        };
        inner.state.apply(&archived);
        emitted.push(archived);

        inner.inbox.archive(InboxItem {
            artifact_id,
            automation_id: automation_id.clone(),
            task_id: task_id.clone(),
            recorded_at: now,
            status,
            run_id,
        });

        // 先读后写：分离 sched 借用与 state/inbox 借用。
        let threshold = inner.config.failure_threshold;
        let prev_streak = inner
            .schedules
            .get(automation_id)
            .map(|s| s.failure_streak)
            .unwrap_or(0);
        let new_streak = match status {
            InboxStatus::Failed => prev_streak.saturating_add(1),
            InboxStatus::Succeeded => 0,
            InboxStatus::Running => prev_streak,
        };
        let should_suspend = matches!(status, InboxStatus::Failed) && new_streak >= threshold;

        if should_suspend {
            let suspended = AutomationEvent::Suspended {
                automation_id: automation_id.clone(),
                reason: format!("suspended after {new_streak} consecutive failures"),
            };
            inner.state.apply(&suspended);
            emitted.push(suspended);
        }

        if let Some(sched) = inner.schedules.get_mut(automation_id) {
            sched.failure_streak = new_streak;
            if should_suspend {
                sched.next_at = None;
            }
        }

        Ok(emitted)
    }

    /// event 触发器匹配：返回正则命中 `payload` 且未挂起的 automation（按 ID 排序）。
    pub fn match_event(&self, payload: &str) -> Vec<AutomationId> {
        let guard = self.inner.lock();
        let inner = &*guard;
        inner
            .configs
            .iter()
            .filter(|(id, registered)| {
                !inner.state.is_suspended(id)
                    && registered
                        .event_regex
                        .as_ref()
                        .is_some_and(|re| re.is_match(payload))
            })
            .map(|(id, _)| id.clone())
            .collect()
    }

    /// 对匹配的 event 触发器全部派发；返回每条结果（按 ID 排序）。
    pub fn dispatch_event(
        &self,
        payload: &str,
        now: u64,
    ) -> Vec<Result<DispatchOutcome, AutomationError>> {
        let matched = self.match_event(payload);
        matched.into_iter().map(|id| self.fire(&id, now)).collect()
    }

    /// 手动挂起 automation（发 `Suspended`，停止后续触发）。
    pub fn suspend(
        &self,
        automation_id: &AutomationId,
        reason: String,
    ) -> Result<AutomationEvent, AutomationError> {
        let mut guard = self.inner.lock();
        let inner = &mut *guard;
        if !inner.configs.contains_key(automation_id) {
            return Err(AutomationError::NotRegistered(automation_id.clone()));
        }
        let event = AutomationEvent::Suspended {
            automation_id: automation_id.clone(),
            reason,
        };
        inner.state.apply(&event);
        if let Some(sched) = inner.schedules.get_mut(automation_id) {
            sched.next_at = None;
        }
        Ok(event)
    }

    /// 恢复 automation：canonical schema 无 Resume 变体，故以重新发出 `Registered`
    /// （幂等、清挂起）表达恢复，并从 `now` 重算下次触发时刻。
    pub fn resume(
        &self,
        automation_id: &AutomationId,
        now: u64,
    ) -> Result<AutomationEvent, AutomationError> {
        let mut guard = self.inner.lock();
        let inner = &mut *guard;
        let registered = inner
            .configs
            .get(automation_id)
            .ok_or_else(|| AutomationError::NotRegistered(automation_id.clone()))?;
        let trigger = registered.automation.trigger.clone();
        let cron = registered.cron_schedule.clone();
        let fired_count = inner.state.fired_count(automation_id);

        let event = AutomationEvent::Registered {
            automation_id: automation_id.clone(),
            trigger: trigger.kind(),
        };
        inner.state.apply(&event);

        if let Some(sched) = inner.schedules.get_mut(automation_id) {
            let once_already = matches!(trigger, AutomationTrigger::Once { .. }) && fired_count > 0;
            sched.next_at = compute_next_at(&trigger, now, once_already, cron.as_ref());
            sched.failure_streak = 0;
        }
        Ok(event)
    }

    // —— 查询面 ——

    /// 单条 automation 快照。
    pub fn automation_snapshot(&self, id: &AutomationId) -> Option<AutomationSnapshot> {
        let guard = self.inner.lock();
        let inner = &*guard;
        let registered = inner.configs.get(id)?;
        let sched = inner.schedules.get(id);
        let fired_count = inner.state.fired_count(id);
        Some(AutomationSnapshot {
            automation_id: id.clone(),
            trigger: registered.automation.trigger.clone(),
            action: registered.automation.action.clone(),
            trigger_kind: registered.automation.trigger.kind(),
            next_at: sched.and_then(|s| s.next_at),
            fired_count,
            failure_streak: sched.map(|s| s.failure_streak).unwrap_or(0),
            suspended: inner.state.is_suspended(id),
        })
    }

    /// 全部 automation 快照（按 ID 排序）。
    pub fn automation_snapshots(&self) -> Vec<AutomationSnapshot> {
        let guard = self.inner.lock();
        let inner = &*guard;
        inner
            .configs
            .iter()
            .map(|(id, registered)| {
                let sched = inner.schedules.get(id);
                let fired_count = inner.state.fired_count(id);
                AutomationSnapshot {
                    automation_id: id.clone(),
                    trigger: registered.automation.trigger.clone(),
                    action: registered.automation.action.clone(),
                    trigger_kind: registered.automation.trigger.kind(),
                    next_at: sched.and_then(|s| s.next_at),
                    fired_count,
                    failure_streak: sched.map(|s| s.failure_streak).unwrap_or(0),
                    suspended: inner.state.is_suspended(id),
                }
            })
            .collect()
    }

    /// 事件溯源状态镜像（用于重放一致性断言）。
    pub fn state(&self) -> AutomationState {
        self.inner.lock().state.clone()
    }

    /// 全部已发出事件（按序）。
    pub fn events(&self) -> Vec<AutomationEvent> {
        self.inner.lock().state.event_log().to_vec()
    }

    /// inbox 检索（按 `recorded_at` 升序）。
    pub fn search_inbox(&self, query: InboxQuery<'_>) -> Vec<InboxItem> {
        self.inner.lock().inbox.search(query)
    }

    /// inbox 全部条目（按时间升序）。
    pub fn inbox_items(&self) -> Vec<InboxItem> {
        self.inner.lock().inbox.items()
    }
}

/// 校验并预编译触发器配置（cron 表达式 / event 正则）。
fn compile_trigger(
    trigger: &AutomationTrigger,
) -> Result<(Option<CronSchedule>, Option<Regex>), AutomationError> {
    match trigger {
        AutomationTrigger::Cron { expr } => {
            let schedule =
                CronSchedule::parse(expr).map_err(|detail| AutomationError::InvalidCron {
                    expr: expr.clone(),
                    detail,
                })?;
            Ok((Some(schedule), None))
        }
        AutomationTrigger::Event { pattern } => {
            let regex = Regex::new(pattern)
                .map_err(|_| AutomationError::InvalidEventPattern(pattern.clone()))?;
            Ok((None, Some(regex)))
        }
        _ => Ok((None, None)),
    }
}

/// 计算下次触发时刻（Unix 秒）。
fn compute_next_at(
    trigger: &AutomationTrigger,
    from: u64,
    once_already_fired: bool,
    cron: Option<&CronSchedule>,
) -> Option<u64> {
    match trigger {
        AutomationTrigger::Cron { .. } => cron.and_then(|schedule| schedule.next_fire(from)),
        AutomationTrigger::Interval { secs } => Some(from.saturating_add(*secs)),
        AutomationTrigger::Once { delay_secs } => {
            if once_already_fired {
                None
            } else {
                Some(from.saturating_add(*delay_secs))
            }
        }
        AutomationTrigger::Event { .. } => None,
    }
}
