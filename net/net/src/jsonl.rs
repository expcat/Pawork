//! 增量 JSON Lines（JSONL）解析器。
//!
//! 按 `\n` 切行（兼容 `\r\n`/`\r`），跨 chunk 与 UTF-8 边界安全；空行跳过。
//! 每行独立解析为 [`serde_json::Value`]，畸形行产出
//! [`JsonLinesItem::ParseError`] 而不 panic。

use serde_json::Value;

/// 单条 JSONL 行允许占用的最大缓冲（1 MiB）。
pub const MAX_BUFFER_BYTES: usize = 1024 * 1024;

/// JSON Lines 解析的单条产出。
#[derive(Clone, Debug)]
pub enum JsonLinesItem {
    /// 成功解析的 JSON 值。
    Parsed(Value),
    /// 无法解析的行，保留原始行文本与错误描述。
    ParseError { line: String, error: String },
}

/// 跨 chunk、UTF-8 边界安全的增量 JSON Lines 解析器。
pub struct JsonLinesParser {
    buf: Vec<u8>,
    discarding_line: bool,
}

impl Default for JsonLinesParser {
    fn default() -> Self {
        Self::new()
    }
}

impl JsonLinesParser {
    /// 创建空解析器。
    pub fn new() -> Self {
        Self {
            buf: Vec::new(),
            discarding_line: false,
        }
    }

    /// 喂入任意字节，返回本批已完整的行解析结果。
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<JsonLinesItem> {
        self.buf.extend_from_slice(bytes);
        remove_invalid_utf8(&mut self.buf);
        let mut items = Vec::new();

        while let Some((line_end, term_end)) = find_terminator(&self.buf) {
            if self.discarding_line {
                self.buf.drain(..term_end);
                self.discarding_line = false;
                continue;
            }
            if line_end > MAX_BUFFER_BYTES {
                self.buf.drain(..term_end);
                items.push(buffer_limit_error());
                continue;
            }
            let line = std::str::from_utf8(&self.buf[..line_end])
                .expect("invalid UTF-8 was removed before line extraction")
                .to_string();
            self.buf.drain(..term_end);
            if line.is_empty() {
                continue;
            }
            items.push(parse_line(&line));
        }

        if self.buf.len() > MAX_BUFFER_BYTES {
            self.buf.clear();
            if !self.discarding_line {
                items.push(buffer_limit_error());
            }
            self.discarding_line = true;
        }
        items
    }

    /// 提前断开：把残留缓冲当作最后一行解析（非空时）。
    pub fn finish(self) -> Vec<JsonLinesItem> {
        let mut items = Vec::new();
        if self.discarding_line || self.buf.is_empty() {
            return items;
        }
        let Ok(s) = std::str::from_utf8(&self.buf) else {
            return items; // 残留非法 UTF-8，丢弃
        };
        let trimmed = s.strip_suffix('\r').unwrap_or(s);
        if trimmed.is_empty() {
            return items;
        }
        items.push(parse_line(trimmed));
        items
    }
}

fn buffer_limit_error() -> JsonLinesItem {
    JsonLinesItem::ParseError {
        line: String::new(),
        error: format!("JSONL buffer exceeded {MAX_BUFFER_BYTES} bytes"),
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

fn parse_line(line: &str) -> JsonLinesItem {
    match serde_json::from_str::<Value>(line) {
        Ok(value) => JsonLinesItem::Parsed(value),
        Err(error) => JsonLinesItem::ParseError {
            line: line.to_string(),
            error: error.to_string(),
        },
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
    fn parses_multiple_lines() {
        let mut parser = JsonLinesParser::new();
        let items = parser.feed(b"{\"a\":1}\n{\"b\":2}\n");
        assert_eq!(items.len(), 2);
        assert!(matches!(
            &items[0],
            JsonLinesItem::Parsed(v) if v == &serde_json::json!({"a": 1})
        ));
        assert!(matches!(
            &items[1],
            JsonLinesItem::Parsed(v) if v == &serde_json::json!({"b": 2})
        ));
    }

    #[test]
    fn line_split_across_feeds() {
        let mut parser = JsonLinesParser::new();
        assert!(parser.feed(b"{\"x\":").is_empty());
        let items = parser.feed(b"true}\n");
        assert_eq!(items.len(), 1);
        assert!(matches!(
            &items[0],
            JsonLinesItem::Parsed(v) if v == &serde_json::json!({"x": true})
        ));
    }

    #[test]
    fn finish_parses_residual_line() {
        let mut parser = JsonLinesParser::new();
        parser.feed(b"{\"a\":1}\n");
        let items = parser.feed(b"{\"b\":2}");
        assert!(items.is_empty());
        let items = parser.finish();
        assert_eq!(items.len(), 1);
        assert!(matches!(
            &items[0],
            JsonLinesItem::Parsed(v) if v == &serde_json::json!({"b": 2})
        ));
    }

    #[test]
    fn malformed_line_produces_error_not_panic() {
        let mut parser = JsonLinesParser::new();
        let items = parser.feed(b"{bad\n");
        assert_eq!(items.len(), 1);
        assert!(matches!(items[0], JsonLinesItem::ParseError { .. }));
    }

    #[test]
    fn empty_lines_are_skipped() {
        let mut parser = JsonLinesParser::new();
        let items = parser.feed(b"\n\n{\"a\":1}\n\n");
        assert_eq!(items.len(), 1);
    }

    #[test]
    fn multibyte_split_safe() {
        // "中" = E4 B8 AD
        let mut parser = JsonLinesParser::new();
        parser.feed(b"\"\xe4\xb8");
        let items = parser.feed(b"\xad\"\n");
        assert_eq!(items.len(), 1);
        assert!(matches!(
            &items[0],
            JsonLinesItem::Parsed(v) if v == &serde_json::json!("中")
        ));
    }

    #[test]
    fn oversized_buffer_emits_error_resets_and_recovers() {
        let mut parser = JsonLinesParser::new();
        let items = parser.feed(&vec![b'x'; MAX_BUFFER_BYTES + 1]);
        assert!(matches!(
            items.as_slice(),
            [JsonLinesItem::ParseError { error, .. }] if error.contains("exceeded")
        ));
        assert!(parser.buf.is_empty(), "overflow must reset the byte buffer");

        let items = parser.feed(b"\n{\"recovered\":true}\n");
        assert!(matches!(
            items.as_slice(),
            [JsonLinesItem::Parsed(value)] if value == &serde_json::json!({"recovered": true})
        ));
    }

    #[test]
    fn invalid_utf8_is_removed_in_bulk() {
        let mut parser = JsonLinesParser::new();
        let items = parser.feed(b"{\"v\":\"a\xff\xfe\xfdb\"}\n");
        assert!(matches!(
            items.as_slice(),
            [JsonLinesItem::Parsed(value)] if value == &serde_json::json!({"v": "ab"})
        ));
    }

    proptest! {
        #[test]
        fn random_bytes_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..1024)) {
            let mut parser = JsonLinesParser::new();
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
