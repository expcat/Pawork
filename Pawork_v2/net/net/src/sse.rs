//! 增量 SSE（Server-Sent Events）解析器。
//!
//! 跨任意 chunk、Unicode 边界安全的纯字节解析，不执行任何 IO。语义遵循
//! WHATWG Server-Sent Events 规范的子集：
//! - 行分隔符接受 `\n`、`\r\n` 与 `\r`；
//! - `:` 开头为注释行，忽略；
//! - 支持 `event`/`data`/`id`/`retry` 字段；`data` 多行以 `\n` 连接并丢弃末尾换行；
//! - 空行派发当前事件；`event` 默认为 `message`；
//! - `retry` 解析失败时忽略；忽略流首的 UTF-8 BOM。

#[cfg(feature = "http")]
use pawork_api::{ProviderError, ProviderErrorKind};
use thiserror::Error;

/// 单条 SSE 行或事件允许占用的最大缓冲（1 MiB）。
pub const MAX_BUFFER_BYTES: usize = 1024 * 1024;

/// SSE 增量解析错误。
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum SseParseError {
    #[error("SSE buffer exceeded {limit} bytes")]
    BufferLimitExceeded { limit: usize },
}

#[cfg(feature = "http")]
impl From<SseParseError> for ProviderError {
    fn from(error: SseParseError) -> Self {
        ProviderError::new(ProviderErrorKind::MalformedResponse, error.to_string())
    }
}

/// 单条已完成的 SSE 事件。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SseEvent {
    /// 事件类型；未显式声明（或声明为空）时解析为 `Some("message")`。
    pub event: Option<String>,
    /// `data` 字段，多行以 `\n` 连接（末尾换行已丢弃）。
    pub data: String,
    /// 该事件块内最近一次 `id:` 字段（未出现则为 `None`）。
    pub id: Option<String>,
    /// `retry:` 字段解析出的重连间隔（毫秒）；解析失败则为 `None`。
    pub retry: Option<u64>,
}

/// 正在拼装中的事件累加器。
#[derive(Default)]
struct EventAccumulator {
    event: Option<String>,
    data: String,
    id: Option<String>,
    retry: Option<u64>,
}

impl EventAccumulator {
    /// 空行触发：派发并重置累加器，仅当 `data` 非空时返回事件。
    fn dispatch(&mut self) -> Option<SseEvent> {
        if self.data.is_empty() {
            self.event = None;
            self.id = None;
            self.retry = None;
            return None;
        }
        let mut data = std::mem::take(&mut self.data);
        if data.ends_with('\n') {
            data.pop();
        }
        let event_name = match self.event.take() {
            Some(name) if !name.is_empty() => name,
            _ => "message".to_string(),
        };
        Some(SseEvent {
            event: Some(event_name),
            data,
            id: self.id.take(),
            retry: self.retry.take(),
        })
    }
}

/// 跨 chunk、UTF-8 边界安全的增量 SSE 解析器。
pub struct SseParser {
    buf: Vec<u8>,
    acc: EventAccumulator,
    bom_seen: bool,
    discard_until_boundary: bool,
}

impl Default for SseParser {
    fn default() -> Self {
        Self::new()
    }
}

impl SseParser {
    /// 创建空解析器。
    pub fn new() -> Self {
        Self {
            buf: Vec::new(),
            acc: EventAccumulator::default(),
            bom_seen: false,
            discard_until_boundary: false,
        }
    }

    /// 喂入任意字节，返回因空行而在本批派发的事件或有界缓冲错误。
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<Result<SseEvent, SseParseError>> {
        self.buf.extend_from_slice(bytes);
        self.strip_bom_once();
        remove_invalid_utf8(&mut self.buf);

        let mut items = Vec::new();
        loop {
            match self.take_next_line() {
                Ok(Some(line)) => self.process_line(&line, &mut items),
                Ok(None) => break,
                Err(error) => {
                    self.acc = EventAccumulator::default();
                    if !self.discard_until_boundary {
                        items.push(Err(error));
                    }
                    self.discard_until_boundary = true;
                }
            }
        }
        items
    }

    /// 流结束：处理最后未终止的一行（若有），并派发残留事件。
    pub fn finish(mut self) -> Result<Option<SseEvent>, SseParseError> {
        if self.discard_until_boundary {
            return Ok(None);
        }
        let buf = std::mem::take(&mut self.buf);
        if buf.len() > MAX_BUFFER_BYTES {
            return Err(SseParseError::BufferLimitExceeded {
                limit: MAX_BUFFER_BYTES,
            });
        }
        if !buf.is_empty() {
            if let Ok(s) = std::str::from_utf8(&buf) {
                let s = s.strip_prefix('\u{FEFF}').unwrap_or(s);
                if !s.is_empty() {
                    let mut items = Vec::new();
                    self.process_line(s, &mut items);
                    if let Some(Err(error)) = items.into_iter().find(Result::is_err) {
                        return Err(error);
                    }
                }
            }
            // 残留为非法 UTF-8 时直接丢弃（尽力而为，不 panic）
        }
        Ok(self.acc.dispatch())
    }

    /// 流首（仅一次）剥离 UTF-8 BOM。
    fn strip_bom_once(&mut self) {
        if !self.bom_seen {
            self.bom_seen = true;
            if self.buf.starts_with(&[0xEF, 0xBB, 0xBF]) {
                self.buf.drain(..3);
            }
        }
    }

    /// 取出下一条完整行（不含行终止符）；无完整行或需等待更多字节时返回 `None`。
    fn take_next_line(&mut self) -> Result<Option<String>, SseParseError> {
        if let Some((line_end, term_end)) = find_terminator(&self.buf) {
            if line_end > MAX_BUFFER_BYTES {
                self.buf.drain(..term_end);
                return Err(SseParseError::BufferLimitExceeded {
                    limit: MAX_BUFFER_BYTES,
                });
            }
            let line = std::str::from_utf8(&self.buf[..line_end])
                .expect("invalid UTF-8 was removed before line extraction")
                .to_string();
            self.buf.drain(..term_end);
            return Ok(Some(line));
        }

        if self.buf.len() > MAX_BUFFER_BYTES {
            self.buf.clear();
            return Err(SseParseError::BufferLimitExceeded {
                limit: MAX_BUFFER_BYTES,
            });
        }
        Ok(None)
    }

    fn process_line(&mut self, line: &str, items: &mut Vec<Result<SseEvent, SseParseError>>) {
        // 兼容行首 BOM
        let line = line.strip_prefix('\u{FEFF}').unwrap_or(line);
        if self.discard_until_boundary {
            if line.is_empty() {
                self.discard_until_boundary = false;
            }
            return;
        }
        if line.is_empty() {
            if let Some(event) = self.acc.dispatch() {
                items.push(Ok(event));
            }
            return;
        }
        if line.starts_with(':') {
            return; // 注释行
        }
        let (field, value) = match line.find(':') {
            Some(idx) => {
                let field = &line[..idx];
                let value = line[idx + 1..]
                    .strip_prefix(' ')
                    .unwrap_or(&line[idx + 1..]);
                (field, value)
            }
            None => (line, ""),
        };
        match field {
            "event" => self.acc.event = Some(value.to_string()),
            "data" => {
                if self
                    .acc
                    .data
                    .len()
                    .saturating_add(value.len())
                    .saturating_add(1)
                    > MAX_BUFFER_BYTES
                {
                    self.acc = EventAccumulator::default();
                    self.discard_until_boundary = true;
                    items.push(Err(SseParseError::BufferLimitExceeded {
                        limit: MAX_BUFFER_BYTES,
                    }));
                    return;
                }
                self.acc.data.push_str(value);
                self.acc.data.push('\n');
            }
            "id" => {
                // 含 U+0000 的 id 按规范忽略
                if !value.contains('\u{0000}') {
                    self.acc.id = Some(value.to_string());
                }
            }
            "retry" => {
                if let Ok(parsed) = value.parse::<u64>() {
                    self.acc.retry = Some(parsed);
                }
            }
            _ => {}
        }
    }
}

/// 线性压缩所有确定非法的 UTF-8 字节；保留尾部可能尚未收齐的多字节序列。
fn remove_invalid_utf8(buf: &mut Vec<u8>) {
    let len = buf.len();
    let mut read = 0;
    let mut write = 0;

    while read < len {
        let (valid_len, error_len) = match std::str::from_utf8(&buf[read..]) {
            Ok(_) => (len - read, None),
            Err(error) => (error.valid_up_to(), error.error_len()),
        };

        if valid_len > 0 {
            if read != write {
                buf.copy_within(read..read + valid_len, write);
            }
            read += valid_len;
            write += valid_len;
        }

        match error_len {
            Some(invalid_len) => read += invalid_len,
            None => {
                if read < len {
                    if read != write {
                        buf.copy_within(read..len, write);
                    }
                    write += len - read;
                }
                break;
            }
        }
    }

    buf.truncate(write);
}

/// 在 `region` 中查找首个行终止符，返回 `(内容结束位置, 终止符结束位置)`。
/// 同时识别 `\n`、`\r\n` 与 `\r`。
fn find_terminator(region: &[u8]) -> Option<(usize, usize)> {
    let mut i = 0;
    while i < region.len() {
        match region[i] {
            b'\n' => return Some((i, i + 1)),
            b'\r' => {
                if i + 1 < region.len() && region[i + 1] == b'\n' {
                    return Some((i, i + 2));
                }
                return Some((i, i + 1));
            }
            _ => i += 1,
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn feed_events(parser: &mut SseParser, bytes: &[u8]) -> Vec<SseEvent> {
        parser
            .feed(bytes)
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .expect("valid SSE input")
    }

    #[test]
    fn single_event_split_across_feeds() {
        let mut parser = SseParser::new();
        assert!(parser.feed(b"even").is_empty());
        assert!(parser.feed(b"t: add\n").is_empty());
        assert!(parser.feed(b"data: ").is_empty());
        let events = feed_events(&mut parser, b"hello\n\n");
        assert_eq!(events.len(), 1);
        let ev = &events[0];
        assert_eq!(ev.event.as_deref(), Some("add"));
        assert_eq!(ev.data, "hello");
    }

    #[test]
    fn multiple_events_in_one_feed() {
        let mut parser = SseParser::new();
        let events = feed_events(&mut parser, b"data: a\n\ndata: b\n\n");
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].data, "a");
        assert_eq!(events[1].data, "b");
        assert_eq!(events[0].event.as_deref(), Some("message"));
    }

    #[test]
    fn multiline_data_joined_with_newline() {
        let mut parser = SseParser::new();
        let events = feed_events(&mut parser, b"data: line1\ndata: line2\n\n");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].data, "line1\nline2");
    }

    #[test]
    fn multibyte_char_split_across_feeds() {
        // "中" = E4 B8 AD
        let mut parser = SseParser::new();
        parser.feed(b"data: \xe4\xb8");
        let events = feed_events(&mut parser, b"\xad\n\n");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].data, "中");
    }

    #[test]
    fn crlf_line_endings() {
        let mut parser = SseParser::new();
        let events = feed_events(&mut parser, b"data: hi\r\n\r\n");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].data, "hi");
    }

    #[test]
    fn comment_id_and_retry() {
        let mut parser = SseParser::new();
        let events = feed_events(&mut parser, b": a comment\ndata: x\nid: 7\nretry: 250\n\n");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].data, "x");
        assert_eq!(events[0].id.as_deref(), Some("7"));
        assert_eq!(events[0].retry, Some(250));
    }

    #[test]
    fn invalid_retry_is_ignored() {
        let mut parser = SseParser::new();
        let events = feed_events(&mut parser, b"data: x\nretry: nope\n\n");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].retry, None);
    }

    #[test]
    fn finish_flushes_pending_event() {
        let mut parser = SseParser::new();
        parser.feed(b"data: tail");
        let event = parser.finish().expect("valid residual SSE");
        assert_eq!(event.map(|e| e.data), Some("tail".to_string()));
    }

    #[test]
    fn finish_without_data_returns_none() {
        let mut parser = SseParser::new();
        parser.feed(b": only a comment");
        assert!(parser.finish().expect("valid residual SSE").is_none());
    }

    #[test]
    fn leading_bom_is_stripped() {
        let mut parser = SseParser::new();
        let events = feed_events(&mut parser, b"\xef\xbb\xbfdata: bom\n\n");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].data, "bom");
    }

    #[test]
    fn oversized_buffer_emits_error_resets_and_recovers() {
        let mut parser = SseParser::new();
        let items = parser.feed(&vec![b'x'; MAX_BUFFER_BYTES + 1]);
        assert!(matches!(
            items.as_slice(),
            [Err(SseParseError::BufferLimitExceeded { limit })] if *limit == MAX_BUFFER_BYTES
        ));
        assert!(parser.buf.is_empty(), "overflow must reset the byte buffer");

        let events = feed_events(&mut parser, b"\n\ndata: recovered\n\n");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].data, "recovered");
    }

    #[test]
    fn invalid_utf8_is_removed_in_bulk() {
        let mut parser = SseParser::new();
        let events = feed_events(&mut parser, b"data: a\xff\xfe\xfdb\n\n");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].data, "ab");
    }

    proptest! {
        #[test]
        fn random_bytes_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..1024)) {
            let mut parser = SseParser::new();
            let mut start = 0;
            while start < bytes.len() {
                let step = (bytes.len() - start) % 7 + 1;
                let end = (start + step).min(bytes.len());
                let _ = parser.feed(&bytes[start..end]);
                start = end;
            }
            let _ = parser.finish();
        }
    }
}
