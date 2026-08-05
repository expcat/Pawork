//! 增量 JSON Lines（JSONL）解析器。
//!
//! 按 `\n` 切行（兼容 `\r\n`/`\r`），跨 chunk 与 UTF-8 边界安全；空行跳过。
//! 每行独立解析为 [`serde_json::Value`]，畸形行产出
//! [`JsonLinesItem::ParseError`] 而不 panic。

use serde_json::Value;

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
}

impl Default for JsonLinesParser {
    fn default() -> Self {
        Self::new()
    }
}

impl JsonLinesParser {
    /// 创建空解析器。
    pub fn new() -> Self {
        Self { buf: Vec::new() }
    }

    /// 喂入任意字节，返回本批已完整的行解析结果。
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<JsonLinesItem> {
        self.buf.extend_from_slice(bytes);
        let mut items = Vec::new();
        while let Some(line) = self.take_next_line() {
            if line.is_empty() {
                continue;
            }
            items.push(parse_line(&line));
        }
        items
    }

    /// 提前断开：把残留缓冲当作最后一行解析（非空时）。
    pub fn finish(self) -> Vec<JsonLinesItem> {
        let mut items = Vec::new();
        if self.buf.is_empty() {
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

    /// 取出下一条完整行（不含行终止符）；无完整行或需等待更多字节时返回 `None`。
    fn take_next_line(&mut self) -> Option<String> {
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
                    // valid 区内无完整行；若尾部紧跟确定非法字节，丢弃以推进
                    if err_len.is_some() && valid_len < self.buf.len() {
                        self.buf.remove(valid_len);
                        continue;
                    }
                    return None;
                }
            }
        }
    }
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
