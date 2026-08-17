//! 中性取消令牌：与 `pawork-domain::CancellationToken` 同形，但不依赖 domain（W1 自含）。

use std::{
    collections::BTreeMap,
    future::Future,
    pin::Pin,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
    task::{Context, Poll, Waker},
};

/// 进程 / 沙箱共用的协作式取消令牌。
#[derive(Clone, Debug, Default)]
pub struct CancellationToken {
    inner: Arc<CancellationState>,
}

#[derive(Debug, Default)]
struct CancellationState {
    cancelled: AtomicBool,
    next_waiter_id: AtomicU64,
    waiters: Mutex<BTreeMap<u64, Waker>>,
}

impl CancellationToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        if self.inner.cancelled.swap(true, Ordering::AcqRel) {
            return;
        }

        let waiters = match self.inner.waiters.lock() {
            Ok(mut waiters) => std::mem::take(&mut *waiters),
            Err(poisoned) => std::mem::take(&mut *poisoned.into_inner()),
        };
        for waiter in waiters.into_values() {
            waiter.wake();
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.inner.cancelled.load(Ordering::Acquire)
    }

    pub fn cancelled(&self) -> CancellationFuture {
        CancellationFuture {
            token: self.clone(),
            waiter_id: None,
        }
    }
}

#[must_use = "取消等待 Future 只有在被 await 或 poll 时才生效"]
pub struct CancellationFuture {
    token: CancellationToken,
    waiter_id: Option<u64>,
}

impl Future for CancellationFuture {
    type Output = ();

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        if this.token.is_cancelled() {
            return Poll::Ready(());
        }

        let mut waiters = match this.token.inner.waiters.lock() {
            Ok(waiters) => waiters,
            Err(poisoned) => poisoned.into_inner(),
        };
        if this.token.is_cancelled() {
            return Poll::Ready(());
        }
        if let Some(waiter_id) = this.waiter_id {
            let replace = match waiters.get(&waiter_id) {
                Some(waiter) => !waiter.will_wake(context.waker()),
                None => true,
            };
            if replace {
                waiters.insert(waiter_id, context.waker().clone());
            }
        } else {
            let waiter_id = this
                .token
                .inner
                .next_waiter_id
                .fetch_add(1, Ordering::Relaxed);
            waiters.insert(waiter_id, context.waker().clone());
            this.waiter_id = Some(waiter_id);
        }
        Poll::Pending
    }
}

impl Drop for CancellationFuture {
    fn drop(&mut self) {
        let Some(waiter_id) = self.waiter_id else {
            return;
        };
        let mut waiters = match self.token.inner.waiters.lock() {
            Ok(waiters) => waiters,
            Err(poisoned) => poisoned.into_inner(),
        };
        waiters.remove(&waiter_id);
    }
}

#[cfg(test)]
mod tests {
    use std::{
        pin::pin,
        sync::Arc,
        task::{Wake, Waker},
    };

    use super::*;

    struct FlagWaker(AtomicBool);

    impl Wake for FlagWaker {
        fn wake(self: Arc<Self>) {
            self.0.store(true, Ordering::Release);
        }
    }

    #[test]
    fn cancellation_is_shared_and_wakes_waiters() {
        let token = CancellationToken::new();
        let observer = token.clone();
        let flag = Arc::new(FlagWaker(AtomicBool::new(false)));
        let waker = Waker::from(flag.clone());
        let mut context = Context::from_waker(&waker);
        let mut cancelled = pin!(observer.cancelled());

        assert_eq!(cancelled.as_mut().poll(&mut context), Poll::Pending);
        token.cancel();

        assert!(observer.is_cancelled());
        assert!(flag.0.load(Ordering::Acquire));
        assert_eq!(cancelled.as_mut().poll(&mut context), Poll::Ready(()));
    }

    #[test]
    fn dropped_waiters_are_removed_before_cancellation() {
        let token = CancellationToken::new();
        let flag = Arc::new(FlagWaker(AtomicBool::new(false)));
        let waker = Waker::from(flag.clone());
        let mut context = Context::from_waker(&waker);

        {
            let mut cancelled = pin!(token.cancelled());
            assert_eq!(cancelled.as_mut().poll(&mut context), Poll::Pending);
            assert_eq!(token.inner.waiters.lock().expect("waiters").len(), 1);
        }

        assert!(token.inner.waiters.lock().expect("waiters").is_empty());
        token.cancel();
        assert!(!flag.0.load(Ordering::Acquire));
    }
}
