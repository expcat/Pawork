//! 事件订阅：有界通道 + 背压策略。

use pawork_protocol::AppEventEnvelope;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;

use crate::error::{SdkError, SdkErrorKind};

/// 背压策略：消费者跟不上时的行为。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BackpressurePolicy {
    /// 丢弃新事件并计数（不阻塞 Host 读取循环，事件流永远前进）。
    Drop,
    /// 记录溢出，下一次读取返回 [`SdkErrorKind::Backpressure`]。
    Error,
}

/// 事件订阅句柄：从有界通道读取 [`AppEventEnvelope`]。
///
/// 订阅被客户端关闭或自身被 drop 后，读取返回 [`SdkErrorKind::Cancelled`]。
#[derive(Debug)]
pub struct EventSubscription {
    stream_label: String,
    policy: BackpressurePolicy,
    receiver: mpsc::Receiver<AppEventEnvelope>,
    dropped: Arc<AtomicU64>,
    overflow_error: Arc<AtomicBool>,
    open: bool,
}

impl EventSubscription {
    pub(crate) fn new_with_counters(
        stream_label: String,
        policy: BackpressurePolicy,
        receiver: mpsc::Receiver<AppEventEnvelope>,
        dropped: Arc<AtomicU64>,
        overflow_error: Arc<AtomicBool>,
    ) -> Self {
        Self {
            stream_label,
            policy,
            receiver,
            dropped,
            overflow_error,
            open: true,
        }
    }

    /// 订阅的事件流标签（`session/<id>`、`run/<id>`、`global` 等）。
    pub fn stream_label(&self) -> &str {
        &self.stream_label
    }

    /// 等待下一个事件；通道关闭（订阅被取消/客户端关闭）返回 Cancelled。
    pub async fn next_event(&mut self) -> Result<AppEventEnvelope, SdkError> {
        if !self.open {
            return Err(SdkError::Cancelled(self.stream_label.clone()));
        }
        // 先消化已缓冲事件；溢出错误在缓冲区排空后才暴露。
        if let Ok(event) = self.receiver.try_recv() {
            return Ok(event);
        }
        self.check_overflow()?;
        self.receiver
            .recv()
            .await
            .ok_or_else(|| SdkError::Cancelled(self.stream_label.clone()))
    }

    /// 非阻塞读取；无事件返回 `None`。
    pub fn try_next(&mut self) -> Result<Option<AppEventEnvelope>, SdkError> {
        if !self.open {
            return Err(SdkError::Cancelled(self.stream_label.clone()));
        }
        if let Ok(event) = self.receiver.try_recv() {
            return Ok(Some(event));
        }
        self.check_overflow()?;
        Ok(None)
    }

    fn check_overflow(&mut self) -> Result<(), SdkError> {
        if self.policy == BackpressurePolicy::Error
            && self.overflow_error.swap(false, Ordering::SeqCst)
        {
            return Err(SdkError::Backpressure(format!(
                "stream `{}` overflowed ({} events dropped)",
                self.stream_label,
                self.dropped_events()
            )));
        }
        Ok(())
    }

    /// 因背压丢弃的事件数（累计）。
    pub fn dropped_events(&self) -> u64 {
        self.dropped.load(Ordering::SeqCst)
    }
}

impl Drop for EventSubscription {
    fn drop(&mut self) {
        self.receiver.close();
    }
}

/// 断言用的辅助：把事件流标签与错误类别暴露给测试。
pub fn backpressure_error_kind() -> SdkErrorKind {
    SdkErrorKind::Backpressure
}
