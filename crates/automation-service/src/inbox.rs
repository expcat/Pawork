//! Result Inbox：每次执行的产出归档与检索（进程内内存结构）。
//!
//! Inbox 项携带最终状态（Succeeded / Failed）与时间戳；canonical 事件
//! `AutomationEvent::ResultArchived` 不携带状态（轻量事实），状态是 inbox 本地
//! 视图。检索支持按 automation / 状态 / 时间区间过滤。

use std::collections::BTreeMap;

use agent_domain::{ArtifactId, AutomationId, BackgroundTaskId, RunId};
use serde::{Deserialize, Serialize};

/// Inbox 项的最终状态。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InboxStatus {
    /// 进行中（已派发，尚未记录结果）。
    #[default]
    Running,
    /// 执行成功。
    Succeeded,
    /// 执行失败（计入失败退避）。
    Failed,
}

/// 一条 inbox 记录。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InboxItem {
    pub artifact_id: ArtifactId,
    pub automation_id: AutomationId,
    pub task_id: BackgroundTaskId,
    /// 记录时刻（Unix 秒）。
    pub recorded_at: u64,
    pub status: InboxStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<RunId>,
}

/// Inbox 检索条件（所有字段可选）。
#[derive(Clone, Debug, Default)]
pub struct InboxQuery<'a> {
    pub automation_id: Option<&'a AutomationId>,
    pub status: Option<InboxStatus>,
    /// `recorded_at >= since`。
    pub since: Option<u64>,
    /// `recorded_at <= until`。
    pub until: Option<u64>,
}

/// 进程内 result inbox：按时间顺序保存，支持过滤检索。
#[derive(Clone, Debug, Default)]
pub struct ResultInbox {
    items: Vec<InboxItem>,
    by_automation: BTreeMap<AutomationId, usize>,
}

impl ResultInbox {
    pub fn new() -> Self {
        Self::default()
    }

    /// 归档一条结果。同 `(automation_id, task_id)` 已存在则覆盖状态（幂等）。
    pub fn archive(&mut self, item: InboxItem) {
        let existing = self.items.iter().position(|existing| {
            existing.automation_id == item.automation_id && existing.task_id == item.task_id
        });
        if let Some(i) = existing {
            self.items[i] = item;
            return;
        }
        *self.by_automation.entry(item.automation_id.clone()).or_insert(0) += 1;
        self.items.push(item);
    }

    /// 按条件检索，按 `recorded_at` 升序返回。
    pub fn search(&self, query: InboxQuery<'_>) -> Vec<InboxItem> {
        let mut hits: Vec<InboxItem> = self
            .items
            .iter()
            .filter(|item| {
                if let Some(id) = query.automation_id {
                    if &item.automation_id != id {
                        return false;
                    }
                }
                if let Some(status) = query.status {
                    if item.status != status {
                        return false;
                    }
                }
                if let Some(since) = query.since {
                    if item.recorded_at < since {
                        return false;
                    }
                }
                if let Some(until) = query.until {
                    if item.recorded_at > until {
                        return false;
                    }
                }
                true
            })
            .cloned()
            .collect();
        hits.sort_by_key(|item| item.recorded_at);
        hits
    }

    /// 全部条目（按时间升序的拷贝）。
    pub fn items(&self) -> Vec<InboxItem> {
        let mut all = self.items.clone();
        all.sort_by_key(|item| item.recorded_at);
        all
    }

    /// 某 automation 的结果条数。
    pub fn count_for(&self, automation_id: &AutomationId) -> usize {
        self.by_automation.get(automation_id).copied().unwrap_or(0)
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}
