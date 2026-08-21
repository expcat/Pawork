//! Event Hub：Core Event 的统一扇出中心。
//!
//! - **全局序列**：`AtomicU64` 单调分配；[`EventHub::publish`] 强制重写
//!   `global_sequence`，保证跨 run / 跨 stream 的事件全局连续。
//! - **ring buffer**：保留最近 `capacity`（默认 4096）条事件，支持
//!   [`EventHub::earliest_available`] / [`EventHub::current`] /
//!   [`EventHub::replay`]，供 GUI 重连恢复与 CLI watch 使用。
//! - **有界广播订阅**：基于 `tokio::sync::broadcast`，慢消费者被标记
//!   [`HubError::Lagged`] 而不会阻塞发布者。

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use pawork_domain::{DegradeEvent, DegradeKind, DegradeSeverity, EventId};
use pawork_engine::now_timestamp;
use pawork_protocol::{
    AppEvent, AppEventEnvelope, EventSource, EventStream, GlobalSequence,
    API_VERSION,
};
use serde_json::json;
use thiserror::Error;
use tokio::sync::broadcast;

/// 默认 ring buffer / 广播容量。
pub const DEFAULT_HUB_CAPACITY: usize = 4096;

/// Event Hub 错误。
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum HubError {
    /// 订阅者消费过慢，错过了 {missed} 条事件（继续轮询可拿到最新事件）。
    #[error("subscriber lagged behind by {missed} events")]
    Lagged { missed: u64 },
    /// 广播通道已关闭（所有 Sender 被释放）。
    #[error("event hub channel is closed")]
    Closed,
    /// replay 请求的起始序列已超出 ring buffer 保留范围。
    #[error("replay range starts at {requested_from:?} but earliest available is {earliest_available:?}")]
    ReplayUnavailable {
        requested_from: GlobalSequence,
        earliest_available: GlobalSequence,
    },
    /// 同步 try_recv 时当前没有可用事件。
    #[error("no event currently available")]
    Empty,
}

/// 事件 Hub：全局序列 + ring buffer + 有界广播订阅。
///
/// 克隆廉价（内部 `Arc` 语义由调用方持有）；`publish` 可从任意线程调用。
pub struct EventHub {
    capacity: usize,
    /// 下一个待分配的全局序列（`fetch_add` 前值 +1 为本次序列，首条为 1）。
    next_sequence: AtomicU64,
    ring: Mutex<VecDeque<AppEventEnvelope>>,
    sender: broadcast::Sender<AppEventEnvelope>,
}

impl EventHub {
    /// 以默认容量（4096）创建 Hub。
    pub fn new() -> Self {
        Self::with_capacity(DEFAULT_HUB_CAPACITY)
    }

    /// 以指定容量创建 Hub（ring buffer 与广播缓冲同容量，下限 1）。
    pub fn with_capacity(capacity: usize) -> Self {
        let capacity = capacity.max(1);
        let (sender, _) = broadcast::channel(capacity);
        Self {
            capacity,
            next_sequence: AtomicU64::new(0),
            ring: Mutex::new(VecDeque::with_capacity(capacity)),
            sender,
        }
    }

    /// ring buffer / 广播容量。
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// 发布一条事件：强制重写 `global_sequence` 保证全局连续，写入 ring buffer
    /// 并广播给所有订阅者。返回成功投递的订阅者数（无订阅者时为 0）。
    pub fn publish(&self, envelope: AppEventEnvelope) -> usize {
        self.publish_with_envelope(envelope).1
    }

    /// Assign a contiguous global_sequence, retain the event, and broadcast it.
    /// Returns the sequenced envelope plus the number of live subscribers that received it.
    pub(crate) fn publish_with_envelope(&self, mut envelope: AppEventEnvelope) -> (AppEventEnvelope, usize) {
        let sequence = self.next_sequence.fetch_add(1, Ordering::SeqCst) + 1;
        envelope.global_sequence = GlobalSequence(sequence);
        {
            let mut ring = lock(&self.ring);
            if ring.len() == self.capacity {
                ring.pop_front();
            }
            ring.push_back(envelope.clone());
        }
        let delivered = self.sender.send(envelope.clone()).unwrap_or_default();
        (envelope, delivered)
    }

    /// Publish a `degrade.event_stream_lagged` Diagnostic frame onto the hub.
    /// Used when a subscriber reports Lagged or a bus publish cannot be delivered.
    pub fn publish_lagged_degrade(
        &self,
        instance_id: pawork_domain::CoreInstanceId,
        missed: Option<u64>,
        client_id: Option<&str>,
    ) -> usize {
        self.publish_lagged_degrade_envelope(instance_id, missed, client_id).1
    }

    /// Same as [`EventHub::publish_lagged_degrade`], returning the sequenced envelope
    /// and the number of live subscribers that received the broadcast.
    pub fn publish_lagged_degrade_envelope(
        &self,
        instance_id: pawork_domain::CoreInstanceId,
        missed: Option<u64>,
        client_id: Option<&str>,
    ) -> (AppEventEnvelope, usize) {
        let mut details = json!({});
        if let Some(missed) = missed {
            details["missed"] = json!(missed);
        }
        if let Some(client_id) = client_id {
            details["client_id"] = json!(client_id);
        }
        let degrade = DegradeEvent::new(
            DegradeKind::EventStreamLagged,
            DegradeSeverity::Warning,
            "event stream subscriber lagged",
            details,
        );
        let envelope = AppEventEnvelope {
            api_version: API_VERSION,
            instance_id,
            event_id: EventId::from("degrade-event-stream-lagged"),
            global_sequence: GlobalSequence(0),
            stream: EventStream::Global,
            stream_sequence: 0,
            timestamp: now_timestamp(),
            source: EventSource::Core,
            payload: AppEvent::from(&degrade),
        };
        let (envelope, delivered) = self.publish_with_envelope(envelope);
        if delivered == 0 {
            tracing::warn!(
                code = "degrade.event_stream_lagged",
                missed,
                client_id,
                "event stream lagged publish delivered to zero subscribers"
            );
        }
        (envelope, delivered)
    }

    /// 最新已发布序列（尚无事件时为 0）。
    pub fn current(&self) -> GlobalSequence {
        GlobalSequence(self.next_sequence.load(Ordering::SeqCst))
    }

    /// ring buffer 中最旧的可用序列；空 Hub 为 `None`。
    pub fn earliest_available(&self) -> Option<GlobalSequence> {
        lock(&self.ring)
            .front()
            .map(|envelope| envelope.global_sequence)
    }

    /// 按全局序列窗口取回事件：`[from, to]`，`to` 缺省为当前最新序列。
    ///
    /// 请求的起始序列早于 [`EventHub::earliest_available`]（已被 ring 淘汰）时
    /// 返回 [`HubError::ReplayUnavailable`]。
    pub fn replay(
        &self,
        from: GlobalSequence,
        to: Option<GlobalSequence>,
    ) -> Result<Vec<AppEventEnvelope>, HubError> {
        let to = to.unwrap_or_else(|| self.current());
        let ring = lock(&self.ring);
        match ring.front() {
            None => Ok(Vec::new()),
            Some(earliest) if from < earliest.global_sequence => Err(HubError::ReplayUnavailable {
                requested_from: from,
                earliest_available: earliest.global_sequence,
            }),
            Some(_) => Ok(ring
                .iter()
                .filter(|envelope| {
                    envelope.global_sequence >= from && envelope.global_sequence <= to
                })
                .cloned()
                .collect()),
        }
    }

    /// 订阅事件流。新订阅者只能收到订阅之后发布的事件（慢消费见
    /// [`HubError::Lagged`]；补历史请用 [`EventHub::replay`]）。
    pub fn subscribe(&self) -> HubSubscription {
        HubSubscription {
            receiver: self.sender.subscribe(),
        }
    }

    /// 取出底层 `broadcast::Receiver`，供仍消费 tokio broadcast 的调用方
    ///（`GuiEventBus::subscribe` / `GuiHostAdapter::subscribe_events`）。
    pub fn subscribe_receiver(&self) -> broadcast::Receiver<AppEventEnvelope> {
        self.sender.subscribe()
    }
}

impl Default for EventHub {
    fn default() -> Self {
        Self::new()
    }
}

/// 事件订阅句柄（有界缓冲）。
pub struct HubSubscription {
    receiver: broadcast::Receiver<AppEventEnvelope>,
}

impl HubSubscription {
    /// 异步接收下一条事件。
    pub async fn recv(&mut self) -> Result<AppEventEnvelope, HubError> {
        match self.receiver.recv().await {
            Ok(event) => Ok(event),
            Err(broadcast::error::RecvError::Lagged(missed)) => Err(HubError::Lagged { missed }),
            Err(broadcast::error::RecvError::Closed) => Err(HubError::Closed),
        }
    }

    /// 非阻塞接收：无可用事件返回 [`HubError::Empty`]。
    pub fn try_recv(&mut self) -> Result<AppEventEnvelope, HubError> {
        match self.receiver.try_recv() {
            Ok(event) => Ok(event),
            Err(broadcast::error::TryRecvError::Empty) => Err(HubError::Empty),
            Err(broadcast::error::TryRecvError::Lagged(missed)) => Err(HubError::Lagged { missed }),
            Err(broadcast::error::TryRecvError::Closed) => Err(HubError::Closed),
        }
    }
}

fn lock<T>(inner: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    inner
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use pawork_domain::{CoreInstanceId, EventId, RunId, Timestamp};
    use pawork_protocol::{AppEvent, DiagnosticLevel, EventSource, EventStream, RunState, API_VERSION};

    fn envelope(sequence: u64) -> AppEventEnvelope {
        AppEventEnvelope {
            api_version: API_VERSION,
            instance_id: CoreInstanceId::from("instance-1"),
            event_id: EventId::from(format!("input-{sequence}")),
            global_sequence: GlobalSequence(sequence),
            stream: EventStream::Run(RunId::from("run-1")),
            stream_sequence: sequence,
            timestamp: Timestamp::from_unix_millis(sequence),
            source: EventSource::Core,
            payload: AppEvent::RunChanged {
                run_id: RunId::from("run-1"),
                state: RunState::StreamingResponse,
            },
        }
    }

    #[test]
    fn publish_rewrites_global_sequence_to_be_contiguous() {
        let hub = EventHub::new();
        // 上游输入序列带空洞；Hub 必须强制重写为连续 global_sequence。
        hub.publish(envelope(100));
        hub.publish(envelope(200));
        hub.publish(envelope(400));
        assert_eq!(hub.current(), GlobalSequence(3));
        assert_eq!(hub.earliest_available(), Some(GlobalSequence(1)));
        let events = hub.replay(GlobalSequence(1), None).expect("replay");
        let sequences: Vec<u64> = events.iter().map(|event| event.global_sequence.0).collect();
        assert_eq!(sequences, vec![1, 2, 3]);
        // 连续性不变量：全局序列强制重写后相邻连续（stream 序列由上游负责）。
        for pair in events.windows(2) {
            assert!(
                pair[1]
                    .global_sequence
                    .is_immediately_after(pair[0].global_sequence),
                "hub must rewrite global sequence to be contiguous"
            );
        }
    }

    #[test]
    fn earliest_and_current_track_published_range() {
        let hub = EventHub::new();
        assert_eq!(hub.current(), GlobalSequence(0));
        assert_eq!(hub.earliest_available(), None);
        hub.publish(envelope(1));
        hub.publish(envelope(2));
        assert_eq!(hub.current(), GlobalSequence(2));
        assert_eq!(hub.earliest_available(), Some(GlobalSequence(1)));
    }

    #[test]
    fn replay_returns_exact_window_and_errors_before_earliest() {
        let hub = EventHub::new();
        for sequence in 1..=5 {
            hub.publish(envelope(sequence));
        }
        let events = hub
            .replay(GlobalSequence(2), Some(GlobalSequence(4)))
            .expect("replay");
        let sequences: Vec<u64> = events.iter().map(|event| event.global_sequence.0).collect();
        assert_eq!(sequences, vec![2, 3, 4]);
        assert!(hub.replay(GlobalSequence(1), None).is_ok());
        // 超过当前上限则只返回可用部分。
        let events = hub
            .replay(GlobalSequence(4), Some(GlobalSequence(99)))
            .expect("replay");
        assert_eq!(events.len(), 2);
    }

    #[test]
    fn ring_evicts_oldest_beyond_capacity() {
        let hub = EventHub::with_capacity(4);
        for sequence in 1..=6 {
            hub.publish(envelope(sequence));
        }
        assert_eq!(hub.earliest_available(), Some(GlobalSequence(3)));
        assert_eq!(
            hub.replay(GlobalSequence(1), None),
            Err(HubError::ReplayUnavailable {
                requested_from: GlobalSequence(1),
                earliest_available: GlobalSequence(3),
            })
        );
        let events = hub.replay(GlobalSequence(3), None).expect("replay");
        let sequences: Vec<u64> = events.iter().map(|event| event.global_sequence.0).collect();
        assert_eq!(sequences, vec![3, 4, 5, 6]);
    }

    #[tokio::test]
    async fn subscription_receives_published_events_in_order() {
        let hub = EventHub::new();
        let mut subscription = hub.subscribe();
        hub.publish(envelope(1));
        hub.publish(envelope(2));
        let first = subscription.recv().await.expect("first event");
        let second = subscription.recv().await.expect("second event");
        assert_eq!(first.global_sequence, GlobalSequence(1));
        assert_eq!(second.global_sequence, GlobalSequence(2));
        assert_eq!(second.validate_after(&first), Ok(()));
    }

    #[tokio::test]
    async fn slow_subscriber_lags_instead_of_blocking_publisher() {
        let hub = EventHub::with_capacity(2);
        let mut subscription = hub.subscribe();
        // 订阅后不消费，容量 2 发布 4 条 → 订阅者落后。
        for sequence in 1..=4 {
            hub.publish(envelope(sequence));
        }
        match subscription.recv().await {
            Err(HubError::Lagged { missed }) => assert_eq!(missed, 2),
            other => panic!("expected lagged, got {other:?}"),
        }
        // Lagged 后从 ring 中最旧可用事件继续（容量 2：事件 3、4 被保留）。
        let first = subscription
            .recv()
            .await
            .expect("oldest retained after lag");
        assert_eq!(first.global_sequence, GlobalSequence(3));
        let latest = subscription.recv().await.expect("latest after lag");
        assert_eq!(latest.global_sequence, GlobalSequence(4));
    }

    #[tokio::test]
    async fn try_recv_reports_empty_without_events() {
        let hub = EventHub::new();
        let mut subscription = hub.subscribe();
        assert_eq!(subscription.try_recv(), Err(HubError::Empty));
        hub.publish(envelope(1));
        assert_eq!(
            subscription.try_recv().expect("event").global_sequence,
            GlobalSequence(1)
        );
    }

    #[test]
    fn lagged_degrade_frame_is_published_onto_the_hub() {
        let hub = EventHub::new();
        let mut subscription = hub.subscribe();
        hub.publish_lagged_degrade(
            pawork_domain::CoreInstanceId::from("instance-1"),
            Some(2),
            Some("gui-1"),
        );
        let event = subscription.try_recv().expect("lagged frame");
        match event.payload {
            AppEvent::Diagnostic { level, code, message } => {
                assert_eq!(level, DiagnosticLevel::Warning);
                assert_eq!(code, "degrade.event_stream_lagged");
                assert_eq!(message, "event stream subscriber lagged");
            }
            other => panic!("expected lagged diagnostic, got {other:?}"),
        }
    }
}
