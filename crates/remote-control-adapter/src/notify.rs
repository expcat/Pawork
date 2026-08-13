//! 通知推送：有界环形缓冲 + event_id 去重 + 按序 replay（gap 显式）。
//!
//! 只有两类 canonical 事件映射为通知，保证推送量有界且与远程决策相关：
//!
//! - `RunChanged` 终态 → [`NotificationPayload::RunFinished`]；
//! - `ToolApprovalRequired` → [`NotificationPayload::ApprovalRequested`]。
//!
//! 通知序列号由本模块单调分配（首条为 1）。replay 请求的起点早于环形缓冲
//! 保留窗口时返回显式 [`ReplayGap`]（requested_from 与 earliest_available），
//! 客户端可据此降级为查询重建状态——不静默丢事件。

use std::collections::{HashSet, VecDeque};
use std::fmt;
use std::sync::{Arc, Mutex};

use agent_domain::{RunId, ToolCallId};
use core_api::{AppEvent, AppEventEnvelope, RunState};
use serde::{Deserialize, Serialize};

/// 默认通知环形缓冲容量。
pub const DEFAULT_NOTIFICATION_CAPACITY: usize = 2048;
/// 默认去重集合容量（event_id）。
pub const DEFAULT_DEDUP_CAPACITY: usize = 4096;

/// 通知载荷（受限映射）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum NotificationPayload {
    /// Run 到达终态（Completed/Failed/Cancelled/Interrupted）。
    RunFinished { run_id: RunId, state: RunState },
    /// 工具调用等待审批。
    ApprovalRequested {
        run_id: RunId,
        tool_call_id: ToolCallId,
        reason: String,
    },
}

/// 一条已编号通知。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Notification {
    /// 本日志分配的单调序列号（首条为 1）。
    pub seq: u64,
    /// 源 canonical event_id（去重键）。
    pub event_id: String,
    /// 源事件时间戳（Unix 毫秒）。
    pub occurred_at_ms: u64,
    pub payload: NotificationPayload,
}

/// 重放窗口缺口：请求起点已被环形缓冲淘汰。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplayGap {
    pub requested_from: u64,
    pub earliest_available: u64,
}

impl fmt::Display for ReplayGap {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "replay requested from seq {} but earliest available is seq {}",
            self.requested_from, self.earliest_available
        )
    }
}

fn is_terminal(state: &RunState) -> bool {
    matches!(
        state,
        RunState::Completed | RunState::Failed | RunState::Cancelled | RunState::Interrupted
    )
}

struct Inner {
    ring: VecDeque<Notification>,
    seen: HashSet<String>,
    seen_order: VecDeque<String>,
    next_seq: u64,
}

/// 通知日志（克隆廉价，内部共享同一状态）。
#[derive(Clone)]
pub struct NotificationLog {
    capacity: usize,
    dedup_capacity: usize,
    inner: Arc<Mutex<Inner>>,
}

impl NotificationLog {
    pub fn new() -> Self {
        Self::with_capacity(DEFAULT_NOTIFICATION_CAPACITY, DEFAULT_DEDUP_CAPACITY)
    }

    pub fn with_capacity(capacity: usize, dedup_capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            dedup_capacity: dedup_capacity.max(1),
            inner: Arc::new(Mutex::new(Inner {
                ring: VecDeque::with_capacity(capacity.max(1)),
                seen: HashSet::new(),
                seen_order: VecDeque::new(),
                next_seq: 1,
            })),
        }
    }

    /// 映射 canonical 事件为通知。非映射变体与重复 event_id 返回 `None`。
    pub fn push_mapped(&self, envelope: AppEventEnvelope) -> Option<Notification> {
        let payload = match &envelope.payload {
            AppEvent::RunChanged { run_id, state } if is_terminal(state) => {
                NotificationPayload::RunFinished {
                    run_id: run_id.clone(),
                    state: state.clone(),
                }
            }
            AppEvent::ToolApprovalRequired {
                run_id,
                tool_call_id,
                reason,
            } => NotificationPayload::ApprovalRequested {
                run_id: run_id.clone(),
                tool_call_id: tool_call_id.clone(),
                reason: reason.clone(),
            },
            _ => return None,
        };
        let event_id = envelope.event_id.as_str().to_string();
        let mut inner = lock(&self.inner);
        if inner.seen.contains(&event_id) {
            return None;
        }
        inner.seen.insert(event_id.clone());
        inner.seen_order.push_back(event_id.clone());
        while inner.seen_order.len() > self.dedup_capacity {
            if let Some(oldest) = inner.seen_order.pop_front() {
                inner.seen.remove(&oldest);
            }
        }
        let notification = Notification {
            seq: inner.next_seq,
            event_id,
            occurred_at_ms: envelope.timestamp.as_unix_millis(),
            payload,
        };
        inner.next_seq += 1;
        if inner.ring.len() == self.capacity {
            inner.ring.pop_front();
        }
        inner.ring.push_back(notification.clone());
        Some(notification)
    }

    /// 按序重放 `[from_seq, +∞)` 内现存通知；窗口已被淘汰时返回显式 gap。
    pub fn replay(&self, from_seq: u64) -> Result<Vec<Notification>, ReplayGap> {
        let inner = lock(&self.inner);
        match inner.ring.front() {
            Some(first) if from_seq < first.seq => Err(ReplayGap {
                requested_from: from_seq,
                earliest_available: first.seq,
            }),
            Some(_) => Ok(inner
                .ring
                .iter()
                .filter(|notification| notification.seq >= from_seq)
                .cloned()
                .collect()),
            // 空日志：没有任何可重放内容（也不存在已淘汰历史）。
            None => Ok(Vec::new()),
        }
    }

    /// 最新已分配序列号；尚无通知时为 `None`。
    pub fn latest_seq(&self) -> Option<u64> {
        let inner = lock(&self.inner);
        inner.next_seq.checked_sub(1)
    }

    /// 环形缓冲中最旧的通知序列号。
    pub fn earliest_seq(&self) -> Option<u64> {
        lock(&self.inner)
            .ring
            .front()
            .map(|notification| notification.seq)
    }

    pub fn len(&self) -> usize {
        lock(&self.inner).ring.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl Default for NotificationLog {
    fn default() -> Self {
        Self::new()
    }
}

fn lock(inner: &Arc<Mutex<Inner>>) -> std::sync::MutexGuard<'_, Inner> {
    inner
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_domain::{CoreInstanceId, EventId, MessageId, Timestamp};
    use core_api::{EventSource, EventStream, GlobalSequence, API_VERSION};

    fn envelope(event_id: &str, payload: AppEvent) -> AppEventEnvelope {
        AppEventEnvelope {
            api_version: API_VERSION,
            instance_id: CoreInstanceId::from("instance"),
            event_id: EventId::from(event_id),
            global_sequence: GlobalSequence(1),
            stream: EventStream::Global,
            stream_sequence: 1,
            timestamp: Timestamp::from_unix_millis(42),
            source: EventSource::Core,
            payload,
        }
    }

    fn run_changed(event_id: &str, state: RunState) -> AppEventEnvelope {
        envelope(
            event_id,
            AppEvent::RunChanged {
                run_id: RunId::from("run-1"),
                state,
            },
        )
    }

    #[test]
    fn maps_only_terminal_run_states_and_approvals() {
        let log = NotificationLog::new();
        assert!(log
            .push_mapped(run_changed("e1", RunState::StreamingResponse))
            .is_none());
        assert!(log
            .push_mapped(run_changed("e2", RunState::WaitingForApproval))
            .is_none());
        let finished = log
            .push_mapped(run_changed("e3", RunState::Completed))
            .expect("terminal mapped");
        assert_eq!(finished.seq, 1);
        assert!(matches!(
            finished.payload,
            NotificationPayload::RunFinished { .. }
        ));
        let approval = log
            .push_mapped(envelope(
                "e4",
                AppEvent::ToolApprovalRequired {
                    run_id: RunId::from("run-1"),
                    tool_call_id: ToolCallId::from("call-1"),
                    reason: "needs approval".into(),
                },
            ))
            .expect("approval mapped");
        assert_eq!(approval.seq, 2);
        assert!(matches!(
            approval.payload,
            NotificationPayload::ApprovalRequested { .. }
        ));
        // 其他事件一律不映射。
        assert!(log
            .push_mapped(envelope(
                "e5",
                AppEvent::AssistantDelta {
                    run_id: RunId::from("run-1"),
                    message_id: MessageId::from("m"),
                    delta: "x".into(),
                },
            ))
            .is_none());
    }

    #[test]
    fn dedups_by_event_id() {
        let log = NotificationLog::new();
        assert!(log
            .push_mapped(run_changed("same", RunState::Failed))
            .is_some());
        assert!(log
            .push_mapped(run_changed("same", RunState::Failed))
            .is_none());
        assert_eq!(log.len(), 1);
        assert_eq!(log.latest_seq(), Some(1));
    }

    #[test]
    fn replay_is_ordered_and_exact_within_window() {
        let log = NotificationLog::with_capacity(8, 64);
        for index in 1..=5 {
            log.push_mapped(run_changed(&format!("e{index}"), RunState::Completed));
        }
        let all = log.replay(1).expect("replay");
        let seqs: Vec<u64> = all.iter().map(|notification| notification.seq).collect();
        assert_eq!(seqs, vec![1, 2, 3, 4, 5]);
        let tail = log.replay(3).expect("replay tail");
        assert_eq!(tail.first().expect("first").seq, 3);
        assert_eq!(tail.len(), 3);
        // 超出最新序列：空结果（无历史可补）。
        assert!(log.replay(99).expect("future replay").is_empty());
    }

    #[test]
    fn replay_gap_is_explicit_when_window_evicted() {
        let log = NotificationLog::with_capacity(3, 64);
        for index in 1..=6 {
            log.push_mapped(run_changed(&format!("e{index}"), RunState::Completed));
        }
        assert_eq!(log.earliest_seq(), Some(4));
        let gap = log.replay(1).expect_err("gap expected");
        assert_eq!(
            gap,
            ReplayGap {
                requested_from: 1,
                earliest_available: 4
            }
        );
        // gap 后按 earliest_available 重放仍可成功。
        assert_eq!(log.replay(gap.earliest_available).expect("replay").len(), 3);
    }
}
