//! 有界环形输出缓冲：支持重连快照与游标续读。

use std::collections::VecDeque;

/// 输出游标：单调递增的绝对字节偏移。
pub type OutputCursor = u64;

/// 有界环形缓冲。超过容量时丢弃最旧数据，并推进 `start` 游标。
#[derive(Debug, Clone)]
pub struct RingBuffer {
    capacity: usize,
    data: VecDeque<u8>,
    /// 仍在缓冲中的最旧字节的绝对游标。
    start: OutputCursor,
    /// 下一个将写入字节的绝对游标。
    end: OutputCursor,
}

impl RingBuffer {
    pub fn new(capacity: usize) -> Self {
        let capacity = capacity.max(1);
        Self {
            capacity,
            data: VecDeque::with_capacity(capacity.min(64 * 1024)),
            start: 0,
            end: 0,
        }
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn start(&self) -> OutputCursor {
        self.start
    }

    pub fn end(&self) -> OutputCursor {
        self.end
    }

    pub fn len(&self) -> usize {
        self.data.len()
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    /// 追加输出；若超容量则丢弃前缀。
    pub fn push(&mut self, bytes: &[u8]) {
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
    pub fn snapshot(&self) -> (OutputCursor, OutputCursor, Vec<u8>) {
        (self.start, self.end, self.data.iter().copied().collect())
    }

    /// 从 `cursor` 起读取可用增量。
    ///
    /// - `cursor == end`：空增量
    /// - `cursor < start`：数据已被丢弃，返回 `Err(Stale)`
    /// - `cursor > end`：未来游标，返回 `Err(Future)`
    pub fn read_since(
        &self,
        cursor: OutputCursor,
    ) -> Result<(OutputCursor, OutputCursor, Vec<u8>), RingReadError> {
        if cursor > self.end {
            return Err(RingReadError::Future {
                requested: cursor,
                end: self.end,
            });
        }
        if cursor < self.start {
            return Err(RingReadError::Stale {
                requested: cursor,
                available_from: self.start,
            });
        }
        let offset = (cursor - self.start) as usize;
        let data: Vec<u8> = self.data.iter().skip(offset).copied().collect();
        Ok((cursor, self.end, data))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RingReadError {
    Stale {
        requested: OutputCursor,
        available_from: OutputCursor,
    },
    Future {
        requested: OutputCursor,
        end: OutputCursor,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_drops_oldest_when_full() {
        let mut ring = RingBuffer::new(4);
        ring.push(b"abcdef");
        assert_eq!(ring.start(), 2);
        assert_eq!(ring.end(), 6);
        let (start, end, data) = ring.snapshot();
        assert_eq!(start, 2);
        assert_eq!(end, 6);
        assert_eq!(data, b"cdef");
    }

    #[test]
    fn read_since_reports_stale_and_incremental() {
        let mut ring = RingBuffer::new(4);
        ring.push(b"123456");
        assert!(matches!(
            ring.read_since(0),
            Err(RingReadError::Stale {
                requested: 0,
                available_from: 2
            })
        ));
        let (from, to, data) = ring.read_since(3).expect("incremental");
        assert_eq!((from, to), (3, 6));
        assert_eq!(data, b"456");
        let (_, _, empty) = ring.read_since(6).expect("eof");
        assert!(empty.is_empty());
    }
}
