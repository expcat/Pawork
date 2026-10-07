//! 有界环形输出缓冲：支持重连快照与输出区间。

use std::collections::VecDeque;

/// 输出游标：单调递增的绝对字节偏移。
pub type OutputCursor = u64;

/// 有界环形缓冲。超过容量时丢弃最旧数据，并推进 `start` 游标。
#[derive(Debug, Clone)]
pub(super) struct RingBuffer {
    capacity: usize,
    data: VecDeque<u8>,
    /// 仍在缓冲中的最旧字节的绝对游标。
    start: OutputCursor,
    /// 下一个将写入字节的绝对游标。
    end: OutputCursor,
}

impl RingBuffer {
    pub(super) fn new(capacity: usize) -> Self {
        let capacity = capacity.max(1);
        Self {
            capacity,
            data: VecDeque::with_capacity(capacity.min(64 * 1024)),
            start: 0,
            end: 0,
        }
    }

    pub(super) fn end(&self) -> OutputCursor {
        self.end
    }

    /// 追加输出；若超容量则丢弃前缀。
    pub(super) fn push(&mut self, bytes: &[u8]) {
        if self.capacity == 0 {
            return;
        }
        for &byte in bytes {
            if self.data.len() >= self.capacity {
                let _ = self.data.pop_front();
                self.start = self.start.saturating_add(1);
            }
            self.data.push_back(byte);
            self.end = self.end.saturating_add(1);
        }
    }

    /// 返回缓冲中全部数据及起止游标。
    pub(super) fn snapshot(&self) -> (OutputCursor, OutputCursor, Vec<u8>) {
        (self.start, self.end, self.data.iter().copied().collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_drops_oldest_when_full() {
        let mut ring = RingBuffer::new(4);
        ring.push(b"abcdef");
        assert_eq!(ring.end(), 6);
        let (start, end, data) = ring.snapshot();
        assert_eq!(start, 2);
        assert_eq!(end, 6);
        assert_eq!(data, b"cdef");
    }
}
