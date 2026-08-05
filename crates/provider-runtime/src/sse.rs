//! 增量 SSE（Server-Sent Events）解析器。
//!
//! 跨任意 chunk、Unicode 边界安全的纯字节解析，不执行任何 IO。语义遵循
//! WHATWG Server-Sent Events 规范的子集：
//! - 行分隔符接受 `\n`、`\r\n` 与 `\r`；
//! - `:` 开头为注释行，忽略；
//! - 支持 `event`/`data`/`id`/`retry` 字段；`data` 多行以 `\n` 连接并丢弃末尾换行；
//! - 空行派发当前事件；`event` 默认为 `message`；
//! - `retry` 解析失败时忽略；忽略流首的 UTF-8 BOM。

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
        }
    }

    /// 喂入任意字节，返回因空行而在本批派发的事件。
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<SseEvent> {
        self.buf.extend_from_slice(bytes);
        let mut events = Vec::new();
        while let Some(line) = self.take_next_line() {
            self.process_line(&line, &mut events);
        }
        events
    }

    /// 流结束：处理最后未终止的一行（若有），并派发残留事件。
    pub fn finish(mut self) -> Option<SseEvent> {
        let buf = std::mem::take(&mut self.buf);
        if !buf.is_empty() {
            if let Ok(s) = std::str::from_utf8(&buf) {
                let s = s.strip_prefix('\u{FEFF}').unwrap_or(s);
                if !s.is_empty() {
                    self.process_line(s, &mut Vec::new());
                }
            }
            // 残留为非法 UTF-8 时直接丢弃（尽力而为，不 panic）
        }
        self.acc.dispatch()
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
    fn take_next_line(&mut self) -> Option<String> {
        self.strip_bom_once();
        loop {
            let (valid_len, err_len) = match std::str::from_utf8(&self.buf) {
                Ok(_) => (self.buf.len(), None),
                Err(e) => (e.valid_up_to(), e.error_len()),
            };
            match find_terminator(&self.buf[..valid_len]) {
                Some((line_end, term_end)) => {
                    let line_bytes = &self.buf[..line_end];
                    let line = std::str::from_utf8(line_bytes)
                        .expect("line lies within valid UTF-8 prefix")
                        .to_string();
                    self.buf.drain(..term_end);
                    return Some(line);
                }
                None => {
                    // valid 区内无完整行；若 valid 区尾部紧跟确定非法字节，丢弃以推进
                    if err_len.is_some() && valid_len < self.buf.len() {
                        self.buf.remove(valid_len);
                        continue;
                    }
                    return None;
                }
            }
        }
    }

    fn process_line(&mut self, line: &str, events: &mut Vec<SseEvent>) {
        // 兼容行首 BOM
        let line = line.strip_prefix('\u{FEFF}').unwrap_or(line);
        if line.is_empty() {
            if let Some(event) = self.acc.dispatch() {
                events.push(event);
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

    #[test]
    fn single_event_split_across_feeds() {
        let mut parser = SseParser::new();
        assert!(parser.feed(b"even").is_empty());
        assert!(parser.feed(b"t: add\n").is_empty());
        assert!(parser.feed(b"data: ").is_empty());
        let events = parser.feed(b"hello\n\n");
        assert_eq!(events.len(), 1);
        let ev = &events[0];
        assert_eq!(ev.event.as_deref(), Some("add"));
        assert_eq!(ev.data, "hello");
    }

    #[test]
    fn multiple_events_in_one_feed() {
        let mut parser = SseParser::new();
        let events = parser.feed(b"data: a\n\ndata: b\n\n");
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].data, "a");
        assert_eq!(events[1].data, "b");
        assert_eq!(events[0].event.as_deref(), Some("message"));
    }

    #[test]
    fn multiline_data_joined_with_newline() {
        let mut parser = SseParser::new();
        let events = parser.feed(b"data: line1\ndata: line2\n\n");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].data, "line1\nline2");
    }

    #[test]
    fn multibyte_char_split_across_feeds() {
        // "中" = E4 B8 AD
        let mut parser = SseParser::new();
        parser.feed(b"data: \xe4\xb8");
        let events = parser.feed(b"\xad\n\n");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].data, "中");
    }

    #[test]
    fn crlf_line_endings() {
        let mut parser = SseParser::new();
        let events = parser.feed(b"data: hi\r\n\r\n");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].data, "hi");
    }

    #[test]
    fn comment_id_and_retry() {
        let mut parser = SseParser::new();
        let events = parser.feed(b": a comment\ndata: x\nid: 7\nretry: 250\n\n");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].data, "x");
        assert_eq!(events[0].id.as_deref(), Some("7"));
        assert_eq!(events[0].retry, Some(250));
    }

    #[test]
    fn invalid_retry_is_ignored() {
        let mut parser = SseParser::new();
        let events = parser.feed(b"data: x\nretry: nope\n\n");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].retry, None);
    }

    #[test]
    fn finish_flushes_pending_event() {
        let mut parser = SseParser::new();
        parser.feed(b"data: tail");
        let event = parser.finish();
        assert_eq!(event.map(|e| e.data), Some("tail".to_string()));
    }

    #[test]
    fn finish_without_data_returns_none() {
        let mut parser = SseParser::new();
        parser.feed(b": only a comment");
        assert!(parser.finish().is_none());
    }

    #[test]
    fn leading_bom_is_stripped() {
        let mut parser = SseParser::new();
        let events = parser.feed(b"\xef\xbb\xbfdata: bom\n\n");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].data, "bom");
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
