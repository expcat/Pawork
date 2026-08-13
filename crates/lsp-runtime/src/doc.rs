//! 文档同步追踪：didOpen / didChange / didClose，维护文档版本与增量同步。

use std::collections::HashMap;

use serde_json::{json, Value};

use crate::protocol::{Position, Range};

/// 一个被追踪的文档。
#[derive(Debug, Clone)]
pub struct TrackedDocument {
    pub uri: String,
    pub language_id: String,
    pub version: i64,
    pub text: String,
}

/// 文档同步状态机。
///
/// 维护每个已 `didOpen` 文档的当前文本与版本号；didChange 应用增量编辑并产出
/// LSP `contentChanges`（增量 range 或 full），didClose 移除追踪。崩溃 restart 后
/// 调用 [`DocumentSync::resync`] 重新 didOpen 所有追踪文档。
#[derive(Debug, Default)]
pub struct DocumentSync {
    docs: HashMap<String, TrackedDocument>,
}

impl DocumentSync {
    pub fn new() -> Self {
        Self::default()
    }

    /// 记录一次 didOpen，返回发送给服务端的 params。
    pub fn open(
        &mut self,
        uri: impl Into<String>,
        language_id: impl Into<String>,
        text: impl Into<String>,
    ) -> Value {
        let uri = uri.into();
        let language_id = language_id.into();
        let text = text.into();
        let doc = TrackedDocument {
            uri: uri.clone(),
            language_id,
            version: 0,
            text,
        };
        let params = json!({
            "textDocument": {
                "uri": doc.uri,
                "languageId": doc.language_id,
                "version": doc.version,
                "text": doc.text,
            }
        });
        self.docs.insert(uri, doc);
        params
    }

    /// 应用一次全量替换 didChange（`TextDocumentSyncKind.Full`）。
    pub fn change_full(&mut self, uri: &str, new_text: impl Into<String>) -> Option<Value> {
        let doc = self.docs.get_mut(uri)?;
        doc.version = doc.version.wrapping_add(1);
        let new_text = new_text.into();
        let params = json!({
            "textDocument": { "uri": doc.uri, "version": doc.version },
            "contentChanges": [{ "text": new_text }],
        });
        doc.text = new_text;
        Some(params)
    }

    /// 应用一次增量 didChange：替换 `range` 内文本为 `new_text`。
    ///
    /// 增量编辑的行/列按 LSP 语义（0-based，UTF-16 code unit 偏移）精确解释；
    /// 落在多 code-unit 字符内部（如代理对中间）的位置钳制到字符起始，越界位置
    /// 钳制到行尾 / 文档末尾，越界行号回退为全量替换。发送给服务端的 range 也是
    /// 钳制后的值，保证客户端本地文本与服务端应用完全一致的编辑。全程只做
    /// char 边界切片，不会 panic。
    pub fn change_incremental(
        &mut self,
        uri: &str,
        range: Range,
        new_text: impl Into<String>,
    ) -> Option<Value> {
        let doc = self.docs.get(uri)?;
        let (byte_start, byte_end, clamped) = match offsets_for_range(&doc.text, range) {
            Some(v) => v,
            None => return self.change_full(uri, new_text),
        };
        let new_text = new_text.into();
        let mut updated = String::with_capacity(doc.text.len() + new_text.len());
        updated.push_str(&doc.text[..byte_start]);
        updated.push_str(&new_text);
        updated.push_str(&doc.text[byte_end..]);
        let doc = self.docs.get_mut(uri)?;
        doc.version = doc.version.wrapping_add(1);
        let params = json!({
            "textDocument": { "uri": doc.uri, "version": doc.version },
            "contentChanges": [{
                "range": clamped,
                "text": new_text,
            }],
        });
        doc.text = updated;
        Some(params)
    }

    /// 记录一次 didClose，返回 params 并移除追踪。
    pub fn close(&mut self, uri: &str) -> Option<Value> {
        if self.docs.remove(uri).is_some() {
            Some(json!({ "textDocument": { "uri": uri } }))
        } else {
            None
        }
    }

    pub fn get(&self, uri: &str) -> Option<&TrackedDocument> {
        self.docs.get(uri)
    }

    pub fn contains(&self, uri: &str) -> bool {
        self.docs.contains_key(uri)
    }

    /// 当前所有追踪文档的 uri（用于崩溃后 resync）。
    pub fn tracked_uris(&self) -> Vec<String> {
        self.docs.keys().cloned().collect()
    }

    /// 把当前所有追踪文档重新 didOpen（崩溃 restart 后调用）。返回每个文档的 params。
    pub fn resync(&self) -> Vec<Value> {
        self.docs
            .values()
            .map(|doc| {
                json!({
                    "textDocument": {
                        "uri": doc.uri,
                        "languageId": doc.language_id,
                        "version": doc.version,
                        "text": doc.text,
                    }
                })
            })
            .collect()
    }
}

/// 把 (line, character) 范围映射为 (byte_start, byte_end, 钳制后的 LSP range)。
/// 越界返回 None。
fn offsets_for_range(text: &str, range: Range) -> Option<(usize, usize, Range)> {
    let start = position_to_offset(text, range.start)?;
    let end = position_to_offset(text, range.end)?;
    if end.0 < start.0 {
        return None;
    }
    Some((start.0, end.0, Range::new(start.1, end.1)))
}

/// 把 LSP 位置（0-based line / UTF-16 code unit column）映射为 UTF-8 字节偏移。
/// 返回 `(byte_offset, clamped_position)`：`clamped_position` 是与字节偏移一致的
/// 有效 LSP 位置（钳制到字符边界 / 行尾），供发送给服务端的 range 使用。
///
/// 单次线性扫描整份文档，按行维护 UTF-16 列计数（O(n)，不反复重算）；
/// 目标行内 `character` 落在字符边界时精确返回，落在多 code-unit 字符内部时
/// 钳制到该字符起始，超出行尾时钳制到行尾（不含 `\n`）。行号越界返回 None。
fn position_to_offset(text: &str, pos: Position) -> Option<(usize, Position)> {
    let mut line = 0u32;
    let mut col = 0u32;
    for (byte_off, ch) in text.char_indices() {
        if line == pos.line {
            if ch == '\n' {
                // 目标行行尾（`\n` 之前）。
                return Some((byte_off, Position::new(pos.line, col)));
            }
            let units = ch.len_utf16() as u32;
            if col >= pos.character || col + units > pos.character {
                // 列在字符边界上，或落在多 code-unit 字符内部（钳制到字符起始）。
                return Some((byte_off, Position::new(pos.line, col)));
            }
            col += units;
        } else if ch == '\n' {
            line += 1;
            col = 0;
        }
    }
    if line == pos.line {
        // 目标行是最后一行：列在行尾或更远 → 钳制到文档末尾（行尾）。
        if col <= pos.character {
            return Some((text.len(), Position::new(pos.line, col)));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{Position, Range};

    #[test]
    fn open_tracks_version_zero() {
        let mut s = DocumentSync::new();
        let p = s.open("file:///a.rs", "rust", "fn main() {}\n");
        assert_eq!(p["textDocument"]["version"], 0);
        assert!(s.contains("file:///a.rs"));
    }

    #[test]
    fn incremental_change_updates_text_and_version() {
        let mut s = DocumentSync::new();
        s.open("file:///a.rs", "rust", "hello world");
        let p = s
            .change_incremental(
                "file:///a.rs",
                Range::new(Position::new(0, 6), Position::new(0, 11)),
                "rust",
            )
            .unwrap();
        assert_eq!(p["textDocument"]["version"], 1);
        assert_eq!(s.get("file:///a.rs").unwrap().text, "hello rust");
    }

    #[test]
    fn utf16_positions_with_mixed_cjk_emoji_ascii() {
        // line 0 UTF-16 列：f n ' ' 你 好 😀 ( ) = 9 units；
        // line 1: n e x t = 4 units。
        let text = "fn 你好😀()\nnext";
        // 字节布局：f=0 n=1 ' '=2 你=3..6 好=6..9 😀=9..13 (=13 )=14 \n=15 n=16 e=17 x=18 t=19
        assert_eq!(
            position_to_offset(text, Position::new(0, 0)),
            Some((0, Position::new(0, 0)))
        );
        assert_eq!(
            position_to_offset(text, Position::new(0, 1)),
            Some((1, Position::new(0, 1)))
        );
        // 你 = 第 3 个 UTF-16 unit。
        assert_eq!(
            position_to_offset(text, Position::new(0, 3)),
            Some((3, Position::new(0, 3)))
        );
        // 😀 = 第 5 个 UTF-16 unit（占 2 个 unit）。
        assert_eq!(
            position_to_offset(text, Position::new(0, 5)),
            Some((9, Position::new(0, 5)))
        );
        // 落在 😀 的代理对中间 → 钳制到字符起始。
        assert_eq!(
            position_to_offset(text, Position::new(0, 6)),
            Some((9, Position::new(0, 5)))
        );
        // ( = 第 7 个 unit。
        assert_eq!(
            position_to_offset(text, Position::new(0, 7)),
            Some((13, Position::new(0, 7)))
        );
        // 行尾（9 units，`\n` 之前）。
        assert_eq!(
            position_to_offset(text, Position::new(0, 9)),
            Some((15, Position::new(0, 9)))
        );
        // 超出行尾 → 钳制到行尾。
        assert_eq!(
            position_to_offset(text, Position::new(0, 100)),
            Some((15, Position::new(0, 9)))
        );
        // 第二行。
        assert_eq!(
            position_to_offset(text, Position::new(1, 0)),
            Some((16, Position::new(1, 0)))
        );
        assert_eq!(
            position_to_offset(text, Position::new(1, 4)),
            Some((20, Position::new(1, 4)))
        );
        // 行号越界 → None。
        assert_eq!(position_to_offset(text, Position::new(2, 0)), None);
    }

    #[test]
    fn utf16_positions_no_panic_on_multibyte_start() {
        // 旧实现按字节枚举并在非 char 边界切片会 panic；这里覆盖各种小列号。
        let text = "你好 world";
        assert_eq!(
            position_to_offset(text, Position::new(0, 0)),
            Some((0, Position::new(0, 0)))
        );
        assert_eq!(
            position_to_offset(text, Position::new(0, 1)),
            Some((3, Position::new(0, 1)))
        );
        assert_eq!(
            position_to_offset(text, Position::new(0, 2)),
            Some((6, Position::new(0, 2)))
        );
        assert_eq!(
            position_to_offset(text, Position::new(0, 3)),
            Some((7, Position::new(0, 3)))
        );
        assert_eq!(
            position_to_offset(text, Position::new(0, 8)),
            Some((12, Position::new(0, 8)))
        );
    }

    #[test]
    fn utf16_positions_multiline_empty_lines() {
        let text = "a\n\nb";
        assert_eq!(
            position_to_offset(text, Position::new(0, 1)),
            Some((1, Position::new(0, 1)))
        );
        // 空行。
        assert_eq!(
            position_to_offset(text, Position::new(1, 0)),
            Some((2, Position::new(1, 0)))
        );
        assert_eq!(
            position_to_offset(text, Position::new(1, 5)),
            Some((2, Position::new(1, 0)))
        );
        assert_eq!(
            position_to_offset(text, Position::new(2, 0)),
            Some((3, Position::new(2, 0)))
        );
        assert_eq!(
            position_to_offset(text, Position::new(2, 1)),
            Some((4, Position::new(2, 1)))
        );
    }

    #[test]
    fn incremental_change_with_emoji_utf16_range() {
        let mut s = DocumentSync::new();
        s.open("file:///a.rs", "rust", "let s = \"😀中a\";");
        // 😀 占 UTF-16 units 9..11，中 = unit 11，a = unit 12；
        // 替换 units 9..12（😀中）为 "OK"。
        let p = s
            .change_incremental(
                "file:///a.rs",
                Range::new(Position::new(0, 9), Position::new(0, 12)),
                "OK",
            )
            .unwrap();
        assert_eq!(s.get("file:///a.rs").unwrap().text, "let s = \"OKa\";");
        assert_eq!(p["contentChanges"][0]["text"], "OK");
        assert_eq!(p["contentChanges"][0]["range"]["start"]["character"], 9);
    }

    #[test]
    fn incremental_change_range_inside_surrogate_clamps_consistently() {
        let mut s = DocumentSync::new();
        s.open("file:///a.rs", "rust", "a😀b");
        // end.character=2 落在 😀（units 1..3）内部 → 钳制到 😀 起始（unit 1），
        // 成为空 range 插入；发送给服务端的 range 同样钳制，两端一致。
        let p = s
            .change_incremental(
                "file:///a.rs",
                Range::new(Position::new(0, 1), Position::new(0, 2)),
                "x",
            )
            .unwrap();
        assert_eq!(s.get("file:///a.rs").unwrap().text, "ax😀b");
        assert_eq!(p["contentChanges"][0]["range"]["start"]["character"], 1);
        assert_eq!(p["contentChanges"][0]["range"]["end"]["character"], 1);
        assert_eq!(p["contentChanges"][0]["text"], "x");
    }

    #[test]
    fn full_change_replaces_text() {
        let mut s = DocumentSync::new();
        s.open("file:///a.rs", "rust", "old");
        let p = s.change_full("file:///a.rs", "brand new").unwrap();
        assert_eq!(p["textDocument"]["version"], 1);
        assert_eq!(s.get("file:///a.rs").unwrap().text, "brand new");
    }

    #[test]
    fn out_of_range_incremental_falls_back_to_full() {
        let mut s = DocumentSync::new();
        s.open("file:///a.rs", "rust", "x");
        let p = s
            .change_incremental(
                "file:///a.rs",
                Range::new(Position::new(99, 0), Position::new(99, 1)),
                "y",
            )
            .unwrap();
        assert!(p["contentChanges"][0].get("range").is_none());
        assert_eq!(s.get("file:///a.rs").unwrap().text, "y");
    }

    #[test]
    fn close_removes_tracking() {
        let mut s = DocumentSync::new();
        s.open("file:///a.rs", "rust", "x");
        assert!(s.close("file:///a.rs").is_some());
        assert!(!s.contains("file:///a.rs"));
    }

    #[test]
    fn resync_emits_didopen_for_all_tracked() {
        let mut s = DocumentSync::new();
        s.open("file:///a.rs", "rust", "a");
        s.open("file:///b.py", "python", "b");
        let r = s.resync();
        assert_eq!(r.len(), 2);
    }
}
