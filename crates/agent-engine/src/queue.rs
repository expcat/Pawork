//! 用户消息队列（P3-5）。
//!
//! Agent 运行中用户可继续发送消息；队列保证不丢失，并支持 `replace queued`
//! 语义：后到的消息可覆盖尚未被消费的待处理消息。队列状态可快照/恢复，供
//! 崩溃后重建（与事件重放配合，见 P3-10）。

use std::collections::VecDeque;

use agent_domain::Message;
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, Notify};

/// 队列内一条待处理用户消息及其入队序号（稳定排序、去重判据）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct QueuedMessage {
    /// 单调递增入队序号，用于稳定排序与 replace 后重置。
    pub sequence: u64,
    pub message: Message,
}

/// 可序列化的队列快照（持久化与恢复用）。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct MessageQueueSnapshot {
    pub next_sequence: u64,
    pub pending: Vec<QueuedMessage>,
}

#[derive(Debug, Default)]
struct MessageQueueInner {
    next_sequence: u64,
    pending: VecDeque<QueuedMessage>,
}

/// 用户消息队列（线程安全、异步友好）。
///
/// - [`MessageQueue::enqueue`]：追加到队尾，不丢失。
/// - [`MessageQueue::replace_queued`]：清空所有尚未消费的待处理消息，仅保留新消息。
/// - [`MessageQueue::drain_one`]：循环每轮取出最早的一条。
#[derive(Debug, Default)]
pub struct MessageQueue {
    inner: Mutex<MessageQueueInner>,
    notify: Notify,
}

impl MessageQueue {
    pub fn new() -> Self {
        Self::default()
    }

    /// 从快照恢复（崩溃后重建）。
    pub async fn from_snapshot(snapshot: MessageQueueSnapshot) -> Self {
        let queue = Self::default();
        let non_empty = {
            let mut inner = queue.inner.lock().await;
            inner.next_sequence = snapshot.next_sequence;
            inner.pending = snapshot.pending.into_iter().collect();
            !inner.pending.is_empty()
        };
        if non_empty {
            queue.notify.notify_one();
        }
        queue
    }

    /// 追加一条用户消息到队尾（不丢失）。
    pub async fn enqueue(&self, message: Message) -> u64 {
        let mut inner = self.inner.lock().await;
        inner.next_sequence += 1;
        let sequence = inner.next_sequence;
        inner.pending.push_back(QueuedMessage { sequence, message });
        drop(inner);
        self.notify.notify_one();
        sequence
    }

    /// 清空所有尚未消费的待处理消息，仅保留新消息；返回被替换掉的条数。
    pub async fn replace_queued(&self, message: Message) -> usize {
        let mut inner = self.inner.lock().await;
        let replaced = inner.pending.len();
        inner.pending.clear();
        inner.next_sequence += 1;
        let sequence = inner.next_sequence;
        inner.pending.push_back(QueuedMessage { sequence, message });
        drop(inner);
        self.notify.notify_one();
        replaced
    }

    /// 取出最早的一条待处理消息（循环每轮消费一条）。
    pub async fn drain_one(&self) -> Option<QueuedMessage> {
        let mut inner = self.inner.lock().await;
        inner.pending.pop_front()
    }

    /// 取出全部待处理消息（按入队序稳定排序）。
    pub async fn drain_all(&self) -> Vec<QueuedMessage> {
        let mut inner = self.inner.lock().await;
        let mut drained: Vec<_> = inner.pending.drain(..).collect();
        drained.sort_by_key(|q| q.sequence);
        drained
    }

    /// 阻塞直到队列非空。
    pub async fn wait_for_message(&self) {
        loop {
            {
                let inner = self.inner.lock().await;
                if !inner.pending.is_empty() {
                    return;
                }
            }
            self.notify.notified().await;
        }
    }

    /// 当前待处理条数。
    pub async fn pending_count(&self) -> usize {
        self.inner.lock().await.pending.len()
    }

    /// 当前待处理条数（同步，仅供测试与快照路径使用）。
    pub fn pending_count_blocking(&self) -> usize {
        self.inner.blocking_lock().pending.len()
    }

    /// 生成可持久化快照（不修改队列）。
    pub async fn snapshot(&self) -> MessageQueueSnapshot {
        let inner = self.inner.lock().await;
        let mut pending: Vec<_> = inner.pending.iter().cloned().collect();
        pending.sort_by_key(|q| q.sequence);
        MessageQueueSnapshot {
            next_sequence: inner.next_sequence,
            pending,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_domain::{ContentPart, MessageId, MessageMetadata, MessageRole, TextContent};

    fn user_message(id: &str, text: &str) -> Message {
        Message {
            id: MessageId::from(id),
            role: MessageRole::User,
            content: vec![ContentPart::Text(TextContent { text: text.into() })],
            metadata: MessageMetadata::default(),
        }
    }

    #[tokio::test]
    async fn enqueue_then_drain_preserves_order() {
        let queue = MessageQueue::new();
        queue.enqueue(user_message("m1", "a")).await;
        queue.enqueue(user_message("m2", "b")).await;
        queue.enqueue(user_message("m3", "c")).await;

        let first = queue.drain_one().await.unwrap();
        let second = queue.drain_one().await.unwrap();
        let third = queue.drain_one().await.unwrap();
        assert_eq!(first.message.id.as_str(), "m1");
        assert_eq!(second.message.id.as_str(), "m2");
        assert_eq!(third.message.id.as_str(), "m3");
        assert!(queue.drain_one().await.is_none());
    }

    #[tokio::test]
    async fn replace_queued_drops_pending_keeps_newest() {
        let queue = MessageQueue::new();
        queue.enqueue(user_message("m1", "a")).await;
        queue.enqueue(user_message("m2", "b")).await;

        let replaced = queue.replace_queued(user_message("m3", "latest")).await;
        assert_eq!(replaced, 2);

        let pending = queue.drain_all().await;
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].message.id.as_str(), "m3");
    }

    #[tokio::test]
    async fn wait_for_message_unblocks_on_enqueue() {
        let queue = MessageQueue::new();
        let queue_clone = std::sync::Arc::new(queue);
        let q = queue_clone.clone();
        let handle = tokio::spawn(async move { q.wait_for_message().await });

        tokio::task::yield_now().await;
        queue_clone.enqueue(user_message("m1", "a")).await;

        tokio::time::timeout(std::time::Duration::from_secs(1), handle)
            .await
            .expect("wait_for_message 应被 enqueue 唤醒")
            .unwrap();
    }

    #[tokio::test]
    async fn snapshot_and_restore_round_trip() {
        let queue = MessageQueue::new();
        queue.enqueue(user_message("m1", "a")).await;
        queue.enqueue(user_message("m2", "b")).await;
        let snapshot = queue.snapshot().await;
        queue.drain_one().await;

        let restored = MessageQueue::from_snapshot(snapshot).await;
        let pending = restored.snapshot().await;
        assert_eq!(pending.pending.len(), 2);
        assert_eq!(pending.pending[0].message.id.as_str(), "m1");
    }

    #[tokio::test]
    async fn concurrent_enqueue_does_not_lose_messages() {
        let queue = std::sync::Arc::new(MessageQueue::new());
        let mut handles = Vec::new();
        for i in 0..20u32 {
            let q = queue.clone();
            handles.push(tokio::spawn(async move {
                q.enqueue(user_message(&format!("m{i}"), "x")).await;
            }));
        }
        for handle in handles {
            handle.await.unwrap();
        }
        let drained = queue.drain_all().await;
        assert_eq!(drained.len(), 20, "并发入队不应丢消息");
        let mut seqs: Vec<_> = drained.iter().map(|q| q.sequence).collect();
        seqs.sort();
        let unique: std::collections::HashSet<_> = seqs.iter().collect();
        assert_eq!(unique.len(), 20);
    }
}
