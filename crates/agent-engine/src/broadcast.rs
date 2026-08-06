//! 事件流式分发（P3-9）。
//!
//! 基于 `tokio::sync::broadcast` 的有界多订阅者广播：核心（Agent Loop）发布
//! [`AgentEventEnvelope`](agent_events::AgentEventEnvelope)，CLI/GUI 等订阅者按
//! 相同顺序接收。有界容量提供背压——慢消费者会被标记为 `Lagged`（丢弃最旧事件）
//! 而不会阻塞核心，满足「慢消费者不拖垮核心、可控丢弃」。
//!
//! 内存广播，分发延迟通常在微秒级，满足 < 2ms 目标。

use std::fmt;

use agent_events::AgentEventEnvelope;
use tokio::sync::broadcast;

/// 广播容量（每个订阅者的有界缓冲）。超过则最旧事件被丢弃并标记 Lagged。
pub const DEFAULT_BROADCAST_CAPACITY: usize = 1024;

/// 广播发布或订阅错误。
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum BroadcastError {
    /// 发布时没有任何活跃订阅者（核心可安全忽略）。
    #[error("no active subscribers")]
    NoSubscribers,
    /// 订阅者消费过慢，错过了 {missed} 条事件。
    #[error("subscriber lagged behind by {missed} events")]
    Lagged { missed: u64 },
    /// 广播通道已关闭（所有 Sender 被释放）。
    #[error("broadcast channel closed")]
    Closed,
}

/// 多订阅者事件广播器。
///
/// 克隆廉价（内部 `Arc`）。调用 [`EventBroadcaster::subscribe`] 获取订阅句柄。
#[derive(Clone)]
pub struct EventBroadcaster {
    sender: broadcast::Sender<AgentEventEnvelope>,
    capacity: usize,
}

impl fmt::Debug for EventBroadcaster {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("EventBroadcaster")
            .field("capacity", &self.capacity)
            .field("subscriber_count", &self.sender.receiver_count())
            .finish_non_exhaustive()
    }
}

impl EventBroadcaster {
    /// 以默认容量创建广播器。
    pub fn new() -> Self {
        Self::with_capacity(DEFAULT_BROADCAST_CAPACITY)
    }

    /// 以指定容量创建广播器。
    pub fn with_capacity(capacity: usize) -> Self {
        let capacity = capacity.max(1);
        let (sender, _) = broadcast::channel(capacity);
        Self { sender, capacity }
    }

    /// 当前订阅者数量。
    pub fn subscriber_count(&self) -> usize {
        self.sender.receiver_count()
    }

    /// 缓冲容量。
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// 订阅事件流。新订阅者只能收到订阅之后发布的事件。
    pub fn subscribe(&self) -> Subscriber {
        Subscriber {
            receiver: self.sender.subscribe(),
        }
    }

    /// 向所有订阅者广播一条事件。
    ///
    /// 返回成功投递的订阅者数量；无订阅者时返回 [`BroadcastError::NoSubscribers`]
    /// （核心通常可忽略该错误而非中断循环）。
    pub fn publish(&self, event: AgentEventEnvelope) -> Result<usize, BroadcastError> {
        match self.sender.send(event) {
            Ok(reached) => Ok(reached),
            Err(broadcast::error::SendError(_)) => Err(BroadcastError::NoSubscribers),
        }
    }
}

impl Default for EventBroadcaster {
    fn default() -> Self {
        Self::new()
    }
}

/// 事件订阅句柄。
///
/// 慢消费者调用 [`Subscriber::recv`] 在错过事件时返回 [`BroadcastError::Lagged`]，
/// 随后可继续接收最新事件（被丢弃的是最旧的若干条）。
pub struct Subscriber {
    receiver: broadcast::Receiver<AgentEventEnvelope>,
}

impl Subscriber {
    /// 异步接收下一条事件。
    ///
    /// - `Ok(event)`：收到事件。
    /// - `Err(Lagged)`：消费过慢，已丢弃 `missed` 条最旧事件；继续轮询可拿最新事件。
    /// - `Err(Closed)`：通道已关闭，不会再有新事件。
    pub async fn recv(&mut self) -> Result<AgentEventEnvelope, BroadcastError> {
        match self.receiver.recv().await {
            Ok(event) => Ok(event),
            Err(broadcast::error::RecvError::Lagged(missed)) => {
                Err(BroadcastError::Lagged { missed })
            }
            Err(broadcast::error::RecvError::Closed) => Err(BroadcastError::Closed),
        }
    }

    /// 非阻塞尝试接收；无就绪事件返回 `None`。
    pub fn try_recv(&mut self) -> Result<Option<AgentEventEnvelope>, BroadcastError> {
        match self.receiver.try_recv() {
            Ok(event) => Ok(Some(event)),
            Err(broadcast::error::TryRecvError::Empty) => Ok(None),
            Err(broadcast::error::TryRecvError::Lagged(missed)) => {
                Err(BroadcastError::Lagged { missed })
            }
            Err(broadcast::error::TryRecvError::Closed) => Err(BroadcastError::Closed),
        }
    }
}

impl fmt::Debug for Subscriber {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("Subscriber").finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use agent_domain::{EventId, RunId, SessionId, Timestamp};
    use agent_events::{AgentEvent, EventSequence};

    use super::*;

    fn envelope(seq: u64) -> AgentEventEnvelope {
        AgentEventEnvelope::new(
            EventId::from(format!("event-{seq}")),
            SessionId::from("session-1"),
            RunId::from("run-1"),
            EventSequence::new(seq),
            Timestamp::from_unix_millis(seq),
            AgentEvent::RunStarted {
                trigger_message_id: agent_domain::MessageId::from("m-1"),
            },
        )
    }

    #[tokio::test]
    async fn multiple_subscribers_receive_same_order() {
        let broadcaster = EventBroadcaster::new();
        let mut a = broadcaster.subscribe();
        let mut b = broadcaster.subscribe();

        for seq in 1..=5 {
            broadcaster.publish(envelope(seq)).unwrap();
        }

        for expected in 1..=5 {
            let ea = a.recv().await.unwrap();
            let eb = b.recv().await.unwrap();
            assert_eq!(ea.sequence.value(), expected);
            assert_eq!(eb.sequence.value(), expected);
        }
    }

    #[tokio::test]
    async fn publish_without_subscribers_is_no_subscribers() {
        let broadcaster = EventBroadcaster::new();
        assert_eq!(
            broadcaster.publish(envelope(1)).unwrap_err(),
            BroadcastError::NoSubscribers
        );
    }

    #[tokio::test]
    async fn slow_subscriber_is_marked_lagged_not_blocking_core() {
        // 极小容量：超过容量后旧事件被丢弃，订阅者拿到 Lagged。
        let broadcaster = EventBroadcaster::with_capacity(2);
        let mut slow = broadcaster.subscribe();

        // 发布 5 条，容量 2 → slow 错过若干条。
        for seq in 1..=5 {
            broadcaster.publish(envelope(seq)).unwrap();
        }

        // 第一次 recv 应当报告 Lagged（核心未被阻塞）。
        let mut saw_lagged = false;
        loop {
            match slow.recv().await {
                Ok(event) => {
                    // 收到的应是较新的事件。
                    assert!(event.sequence.value() >= 4);
                    break;
                }
                Err(BroadcastError::Lagged { missed }) => {
                    assert!(missed > 0);
                    saw_lagged = true;
                }
                Err(other) => panic!("unexpected {other:?}"),
            }
        }
        assert!(saw_lagged, "慢订阅者应至少被标记一次 Lagged");
    }

    #[tokio::test]
    async fn dispatch_latency_is_low() {
        // 验证内存广播分发延迟远低于 2ms 目标。
        let broadcaster = EventBroadcaster::new();
        let mut sub = broadcaster.subscribe();
        let iterations = 1000u32;

        tokio::spawn(async move {
            for seq in 1..=iterations as u64 {
                broadcaster.publish(envelope(seq)).unwrap();
            }
        });

        let mut max_latency = Duration::ZERO;
        for _ in 0..iterations {
            let start = Instant::now();
            let _ = sub.recv().await.unwrap();
            max_latency = max_latency.max(start.elapsed());
        }
        // 留充足余量（含调度抖动），仍远低于 2ms。
        assert!(
            max_latency < Duration::from_millis(2),
            "max dispatch latency {max_latency:?} 超过 2ms 目标"
        );
    }

    #[tokio::test]
    async fn try_recv_returns_none_when_empty() {
        let broadcaster = EventBroadcaster::new();
        let mut sub = broadcaster.subscribe();
        assert!(sub.try_recv().unwrap().is_none());
        broadcaster.publish(envelope(1)).unwrap();
        assert_eq!(sub.try_recv().unwrap().unwrap().sequence.value(), 1);
    }
}
