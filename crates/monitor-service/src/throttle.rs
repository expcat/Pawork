//! 高吞吐输出节流（P16-6）。
//!
//! 常驻进程输出超量走裁剪：[`Throttle`] 是一个有界缓冲，容量满时按
//! [`ThrottlePolicy`] 丢弃（旧或新），`dropped()` 统计被丢弃数量，保证不无界
//! 堆积。本类型只做内存裁剪模拟；真实 PTY 捕获在 pty-service，本任务不重复。

use std::collections::VecDeque;

/// 容量满时的丢弃策略。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ThrottlePolicy {
    /// 丢弃最旧项（保留最近输出，适合 tail 视图）。
    #[default]
    DropOldest,
    /// 丢弃新项（采样：保留先到的，限流上游）。
    DropNewest,
}

/// 有界缓冲：模拟常驻进程高吞吐输出的裁剪。
///
/// 容量满时按 policy 丢弃一项；`dropped()` 统计累计丢弃数，`pushed()` 统计
/// 累计写入数，便于断言「高吞吐经裁剪不堆积」。
#[derive(Debug)]
pub struct Throttle<T> {
    buf: VecDeque<T>,
    capacity: usize,
    policy: ThrottlePolicy,
    dropped: u64,
    pushed: u64,
}

impl<T> Throttle<T> {
    /// 构造容量为 `capacity`（至少 1）的缓冲。
    pub fn new(capacity: usize, policy: ThrottlePolicy) -> Self {
        let capacity = capacity.max(1);
        Self {
            buf: VecDeque::with_capacity(capacity),
            capacity,
            policy,
            dropped: 0,
            pushed: 0,
        }
    }

    /// 仅 DropOldest 的便捷构造。
    pub fn with_capacity(capacity: usize) -> Self {
        Self::new(capacity, ThrottlePolicy::DropOldest)
    }

    /// 当前保留项数（恒 <= capacity）。
    pub fn len(&self) -> usize {
        self.buf.len()
    }

    /// 是否空。
    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    /// 配置容量。
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// 累计被丢弃项数。
    pub fn dropped(&self) -> u64 {
        self.dropped
    }

    /// 累计写入项数（含被丢弃的）。
    pub fn pushed(&self) -> u64 {
        self.pushed
    }

    /// 追加一项，返回该项是否被保留。
    pub fn push(&mut self, item: T) -> bool {
        self.pushed = self.pushed.saturating_add(1);
        if self.buf.len() >= self.capacity {
            match self.policy {
                ThrottlePolicy::DropOldest => {
                    self.buf.pop_front();
                    self.buf.push_back(item);
                    self.dropped = self.dropped.saturating_add(1);
                    true
                }
                ThrottlePolicy::DropNewest => {
                    self.dropped = self.dropped.saturating_add(1);
                    false
                }
            }
        } else {
            self.buf.push_back(item);
            true
        }
    }

    /// 取出全部保留项（按写入顺序），缓冲清空。
    pub fn drain(&mut self) -> Vec<T> {
        self.buf.drain(..).collect()
    }

    /// 借用访问保留项（按写入顺序）。
    pub fn iter(&self) -> impl Iterator<Item = &T> {
        self.buf.iter()
    }
}

impl<T> Default for Throttle<T> {
    fn default() -> Self {
        Self::new(64, ThrottlePolicy::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drop_oldest_keeps_recent_and_counts_drops() {
        let mut t: Throttle<u32> = Throttle::new(3, ThrottlePolicy::DropOldest);
        for v in 0..10u32 {
            assert!(t.push(v));
        }
        assert_eq!(t.len(), 3);
        assert_eq!(t.pushed(), 10);
        assert_eq!(t.dropped(), 7);
        assert_eq!(t.drain(), vec![7, 8, 9]);
    }

    #[test]
    fn drop_newest_samples_first_and_reports_false() {
        let mut t: Throttle<u32> = Throttle::new(3, ThrottlePolicy::DropNewest);
        for v in 0..10u32 {
            let kept = t.push(v);
            assert_eq!(kept, v < 3);
        }
        assert_eq!(t.len(), 3);
        assert_eq!(t.dropped(), 7);
        assert_eq!(t.drain(), vec![0, 1, 2]);
    }

    #[test]
    fn capacity_clamped_to_one() {
        let mut t: Throttle<u32> = Throttle::new(0, ThrottlePolicy::DropOldest);
        assert_eq!(t.capacity(), 1);
        assert!(t.push(1));
        assert!(t.push(2));
        assert_eq!(t.len(), 1);
        assert_eq!(t.dropped(), 1);
        assert_eq!(t.drain(), vec![2]);
    }

    #[test]
    fn never_grows_unbounded_under_high_throughput() {
        let mut t: Throttle<u32> = Throttle::with_capacity(8);
        for v in 0..1_000_000u32 {
            t.push(v);
        }
        assert_eq!(t.len(), 8);
        assert_eq!(t.pushed(), 1_000_000);
        assert_eq!(t.dropped(), 1_000_000 - 8);
        assert_eq!(
            t.iter().copied().collect::<Vec<_>>(),
            (1_000_000 - 8..1_000_000).collect::<Vec<_>>()
        );
    }
}
