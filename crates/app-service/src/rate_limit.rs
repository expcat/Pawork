//! 按 stream 的事件限流合并（P13-1）。
//!
//! 在时间窗内合并同一 stream（Run）的 `assistant_delta` / `thinking_delta` /
//! `tool_output` 增量：窗口到期或缓冲超限时以「每条 key 一条合并事件」发出，
//! 避免高频 delta 淹没客户端。非合并事件（状态变更等）走有界直通队列，超限时
//! 丢弃最旧并计数。纯同步实现（`Instant` 驱动），便于独立测试。

use std::collections::{BTreeMap, VecDeque};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use agent_domain::{ArtifactId, MessageId, RunId, ToolCallId};
use core_api::{AppEvent, AppEventEnvelope};

/// 默认合并时间窗（30ms）。
pub const DEFAULT_RATE_LIMIT_WINDOW: Duration = Duration::from_millis(30);
/// 默认缓冲上限（合并增量 + 直通事件的条目数）。
pub const DEFAULT_RATE_LIMIT_BUFFER: usize = 1024;

/// 可合并的增量事件类别。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DeltaKind {
    Assistant,
    Thinking,
    ToolOutput,
}

impl DeltaKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Assistant => "assistant",
            Self::Thinking => "thinking",
            Self::ToolOutput => "tool_output",
        }
    }
}

/// 限流器统计。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RateLimiterStats {
    /// 已发出的合并/直通事件数。
    pub flushed_events: u64,
    /// 因缓冲上限被丢弃的事件数。
    pub dropped_events: u64,
    /// 当前待合并增量条数。
    pub pending_deltas: usize,
    /// 当前直通队列长度。
    pub pending_pass_through: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct DeltaKey {
    run_id: String,
    kind: DeltaKind,
    id: String,
}

#[derive(Clone, Debug)]
struct PendingDelta {
    run_id: RunId,
    envelope: AppEventEnvelope,
    delta: String,
    truncated: bool,
    artifact_id: Option<ArtifactId>,
    /// 该 key 已合并的增量 push 数（用于缓冲上限判定）。
    pushes: usize,
}

struct Inner {
    window_start: Instant,
    deltas: BTreeMap<DeltaKey, PendingDelta>,
    pass_through: VecDeque<AppEventEnvelope>,
    /// 缓冲中增量 push 总数（同 key 合并也计数）。
    delta_pushes: usize,
    flushed_events: u64,
    dropped_events: u64,
}

/// 按 stream 时间窗合并 delta 事件的限流器。
pub struct RateLimiter {
    window: Duration,
    max_buffered: usize,
    inner: Mutex<Inner>,
}

impl RateLimiter {
    pub fn new(window: Duration, max_buffered: usize) -> Self {
        Self {
            window,
            max_buffered: max_buffered.max(1),
            inner: Mutex::new(Inner {
                window_start: Instant::now(),
                deltas: BTreeMap::new(),
                pass_through: VecDeque::new(),
                delta_pushes: 0,
                flushed_events: 0,
                dropped_events: 0,
            }),
        }
    }

    /// 推入一条事件，返回此刻应发出的合并结果（窗口到期或缓冲超限时触发）。
    pub fn push(&self, envelope: AppEventEnvelope) -> Vec<AppEventEnvelope> {
        let mut inner = lock(&self.inner);
        let mut out = Vec::new();
        if inner.window_start.elapsed() >= self.window {
            out.extend(flush_locked(&mut inner));
        }

        match delta_of(&envelope) {
            Some((run_id, kind, id, delta, truncated, artifact_id)) => {
                let key = DeltaKey {
                    run_id: run_id.as_str().to_string(),
                    kind,
                    id,
                };
                match inner.deltas.get_mut(&key) {
                    Some(pending) => {
                        pending.delta.push_str(&delta);
                        pending.truncated |= truncated;
                        if artifact_id.is_some() {
                            pending.artifact_id = artifact_id;
                        }
                        pending.envelope = envelope;
                        pending.pushes += 1;
                        inner.delta_pushes += 1;
                    }
                    None => {
                        // 新 key 且增量 push 数已达缓冲上限：先淘汰最旧 key 并发出
                        // 其合并结果，保证缓冲有界（确定性：按 key 序淘汰）。
                        if inner.delta_pushes >= self.max_buffered {
                            let oldest_key = inner.deltas.keys().next().cloned();
                            if let Some(oldest_key) = oldest_key {
                                if let Some(pending) = inner.deltas.remove(&oldest_key) {
                                    inner.delta_pushes =
                                        inner.delta_pushes.saturating_sub(pending.pushes);
                                    out.push(merged_envelope(oldest_key, pending));
                                    inner.flushed_events += 1;
                                }
                            }
                        }
                        inner.deltas.insert(
                            key,
                            PendingDelta {
                                run_id,
                                envelope,
                                delta,
                                truncated,
                                artifact_id,
                                pushes: 1,
                            },
                        );
                        inner.delta_pushes += 1;
                    }
                }
            }
            None => {
                if inner.pass_through.len() >= self.max_buffered {
                    inner.pass_through.pop_front();
                    inner.dropped_events += 1;
                }
                inner.pass_through.push_back(envelope);
            }
        }
        out
    }

    /// 立即发出全部缓冲事件（窗口未到期也可强制冲刷）。
    pub fn flush(&self) -> Vec<AppEventEnvelope> {
        let mut inner = lock(&self.inner);
        flush_locked(&mut inner)
    }

    pub fn stats(&self) -> RateLimiterStats {
        let inner = lock(&self.inner);
        RateLimiterStats {
            flushed_events: inner.flushed_events,
            dropped_events: inner.dropped_events,
            pending_deltas: inner.deltas.len(),
            pending_pass_through: inner.pass_through.len(),
        }
    }
}

impl Default for RateLimiter {
    fn default() -> Self {
        Self::new(DEFAULT_RATE_LIMIT_WINDOW, DEFAULT_RATE_LIMIT_BUFFER)
    }
}

/// 提取可合并增量；其余事件返回 `None` 走直通队列。
fn delta_of(
    envelope: &AppEventEnvelope,
) -> Option<(RunId, DeltaKind, String, String, bool, Option<ArtifactId>)> {
    match &envelope.payload {
        AppEvent::AssistantDelta {
            run_id,
            message_id,
            delta,
        } => Some((
            run_id.clone(),
            DeltaKind::Assistant,
            message_id.as_str().to_string(),
            delta.clone(),
            false,
            None,
        )),
        AppEvent::ThinkingDelta {
            run_id,
            message_id,
            delta,
        } => Some((
            run_id.clone(),
            DeltaKind::Thinking,
            message_id.as_str().to_string(),
            delta.clone(),
            false,
            None,
        )),
        AppEvent::ToolOutput {
            run_id,
            tool_call_id,
            delta,
            truncated,
            artifact_id,
        } => Some((
            run_id.clone(),
            DeltaKind::ToolOutput,
            tool_call_id.as_str().to_string(),
            delta.clone(),
            *truncated,
            artifact_id.clone(),
        )),
        _ => None,
    }
}

fn flush_locked(inner: &mut Inner) -> Vec<AppEventEnvelope> {
    let mut out: Vec<AppEventEnvelope> = std::mem::take(&mut inner.deltas)
        .into_iter()
        .map(|(key, pending)| merged_envelope(key, pending))
        .collect();
    out.extend(inner.pass_through.drain(..));
    inner.flushed_events += out.len() as u64;
    inner.delta_pushes = 0;
    inner.window_start = Instant::now();
    out
}

fn merged_envelope(key: DeltaKey, pending: PendingDelta) -> AppEventEnvelope {
    let payload = match key.kind {
        DeltaKind::Assistant => AppEvent::AssistantDelta {
            run_id: pending.run_id.clone(),
            message_id: MessageId::from(key.id),
            delta: pending.delta,
        },
        DeltaKind::Thinking => AppEvent::ThinkingDelta {
            run_id: pending.run_id.clone(),
            message_id: MessageId::from(key.id),
            delta: pending.delta,
        },
        DeltaKind::ToolOutput => AppEvent::ToolOutput {
            run_id: pending.run_id.clone(),
            tool_call_id: ToolCallId::from(key.id),
            delta: pending.delta,
            truncated: pending.truncated,
            artifact_id: pending.artifact_id,
        },
    };
    let mut envelope = pending.envelope;
    envelope.payload = payload;
    envelope
}

fn lock(inner: &Mutex<Inner>) -> std::sync::MutexGuard<'_, Inner> {
    inner
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_domain::{CommandId, CoreInstanceId, EventId, Timestamp};
    use core_api::{EventSource, EventStream, GlobalSequence, API_VERSION};

    fn delta_event(run_id: &str, message_id: &str, delta: &str, sequence: u64) -> AppEventEnvelope {
        AppEventEnvelope {
            api_version: API_VERSION,
            instance_id: CoreInstanceId::from("instance-1"),
            event_id: EventId::from(format!("evt-{sequence}")),
            global_sequence: GlobalSequence(sequence),
            stream: EventStream::Run(RunId::from(run_id)),
            stream_sequence: sequence,
            timestamp: Timestamp::from_unix_millis(sequence),
            source: EventSource::Command {
                command_id: CommandId::from("cmd-1"),
                source: core_api::CommandSource::Automation,
            },
            payload: AppEvent::AssistantDelta {
                run_id: RunId::from(run_id),
                message_id: MessageId::from(message_id),
                delta: delta.into(),
            },
        }
    }

    fn tool_output_event(
        run_id: &str,
        tool_call_id: &str,
        delta: &str,
        sequence: u64,
    ) -> AppEventEnvelope {
        let mut envelope = delta_event(run_id, tool_call_id, delta, sequence);
        envelope.payload = AppEvent::ToolOutput {
            run_id: RunId::from(run_id),
            tool_call_id: ToolCallId::from(tool_call_id),
            delta: delta.into(),
            truncated: false,
            artifact_id: None,
        };
        envelope
    }

    #[test]
    fn deltas_within_window_merge_into_single_event() {
        let limiter = RateLimiter::new(Duration::from_secs(60), 16);
        assert!(limiter
            .push(delta_event("run-1", "msg-1", "a", 1))
            .is_empty());
        assert!(limiter
            .push(delta_event("run-1", "msg-1", "b", 2))
            .is_empty());
        assert!(limiter
            .push(delta_event("run-1", "msg-1", "c", 3))
            .is_empty());

        let flushed = limiter.flush();
        assert_eq!(flushed.len(), 1, "同 key 增量合并为一条");
        match &flushed[0].payload {
            AppEvent::AssistantDelta { delta, .. } => assert_eq!(delta, "abc"),
            other => panic!("unexpected payload: {other:?}"),
        }
        assert_eq!(limiter.stats().flushed_events, 1);
    }

    #[test]
    fn window_expiry_flushes_before_accepting_new_delta() {
        let limiter = RateLimiter::new(Duration::from_millis(30), 16);
        assert!(limiter
            .push(delta_event("run-1", "msg-1", "a", 1))
            .is_empty());
        std::thread::sleep(Duration::from_millis(60));
        let flushed = limiter.push(delta_event("run-1", "msg-1", "b", 2));
        assert_eq!(flushed.len(), 1, "窗口到期先冲刷旧增量");
        match &flushed[0].payload {
            AppEvent::AssistantDelta { delta, .. } => assert_eq!(delta, "a"),
            other => panic!("unexpected payload: {other:?}"),
        }
        let rest = limiter.flush();
        assert_eq!(rest.len(), 1);
        match &rest[0].payload {
            AppEvent::AssistantDelta { delta, .. } => assert_eq!(delta, "b"),
            other => panic!("unexpected payload: {other:?}"),
        }
    }

    #[test]
    fn different_kinds_and_keys_stay_separate() {
        let limiter = RateLimiter::new(Duration::from_secs(60), 16);
        limiter.push(delta_event("run-1", "msg-1", "hi", 1));
        limiter.push(tool_output_event("run-1", "tool-1", "out", 2));
        limiter.push(delta_event("run-1", "msg-2", "second", 3));
        let flushed = limiter.flush();
        assert_eq!(flushed.len(), 3);
        let kinds: Vec<&str> = flushed
            .iter()
            .map(|event| match &event.payload {
                AppEvent::AssistantDelta { .. } => "assistant",
                AppEvent::ToolOutput { .. } => "tool_output",
                other => panic!("unexpected payload: {other:?}"),
            })
            .collect();
        assert!(kinds.contains(&"assistant"));
        assert!(kinds.contains(&"tool_output"));
    }

    #[test]
    fn bounded_deltas_flush_early_when_capacity_exceeded() {
        let limiter = RateLimiter::new(Duration::from_secs(60), 2);
        assert!(limiter
            .push(delta_event("run-1", "msg-1", "a", 1))
            .is_empty());
        assert!(limiter
            .push(delta_event("run-1", "msg-1", "b", 2))
            .is_empty());
        let flushed = limiter.push(delta_event("run-1", "msg-2", "c", 3));
        assert_eq!(flushed.len(), 1, "缓冲超限触发早期冲刷");
        match &flushed[0].payload {
            AppEvent::AssistantDelta { delta, .. } => assert_eq!(delta, "ab"),
            other => panic!("unexpected payload: {other:?}"),
        }
    }

    #[test]
    fn pass_through_queue_is_bounded_and_drops_oldest() {
        let limiter = RateLimiter::new(Duration::from_secs(60), 1);
        let changed = |sequence: u64| {
            let mut envelope = delta_event("run-1", "msg-1", "x", sequence);
            envelope.payload = AppEvent::RunChanged {
                run_id: RunId::from("run-1"),
                state: core_api::RunState::StreamingResponse,
            };
            envelope
        };
        assert!(limiter.push(changed(1)).is_empty());
        assert!(limiter.push(changed(2)).is_empty());
        let stats = limiter.stats();
        assert_eq!(stats.dropped_events, 1);
        assert_eq!(stats.pending_pass_through, 1);
        let flushed = limiter.flush();
        assert_eq!(flushed.len(), 1);
        match &flushed[0].payload {
            AppEvent::RunChanged { .. } => {}
            other => panic!("unexpected payload: {other:?}"),
        }
    }
}
