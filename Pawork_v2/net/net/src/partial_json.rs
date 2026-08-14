//! 跨 chunk 的 tool arguments 增量 JSON 拼接。
//!
//! 每个 [`PartialJson`] 实例对应一个 tool call 的 arguments 流；多个并行 tool
//! call 由调用方按 id 维护多个实例。提供「确定性的尽力修复」解析
//! （[`PartialJson::parse_repaired`]）：补全未闭合的字符串/数组/对象，丢弃尾部
//! 不完整的元素或键值；以及仅对完整 JSON 生效的 [`PartialJson::parse_complete`]。

use serde_json::Value;

/// 跨 chunk 的 tool arguments 增量 JSON 缓冲。
pub struct PartialJson {
    buf: String,
}

impl Default for PartialJson {
    fn default() -> Self {
        Self::new()
    }
}

impl PartialJson {
    /// 创建空缓冲。
    pub fn new() -> Self {
        Self { buf: String::new() }
    }

    /// 追加一个片段。
    pub fn push(&mut self, fragment: &str) {
        self.buf.push_str(fragment);
    }

    /// 当前缓冲的只读视图。
    pub fn as_buffer(&self) -> &str {
        &self.buf
    }

    /// 仅当缓冲为合法完整 JSON 时解析。
    pub fn parse_complete(&self) -> Result<Value, serde_json::Error> {
        serde_json::from_str(&self.buf)
    }

    /// 尽力修复并解析当前缓冲为 [`Value`]。
    ///
    /// 修复语义（确定性）：补全未闭合的 `"`、未闭合的 `[`/`{`，丢弃尾部的逗号、
    /// 冒号及不完整的键值/元素/转义。无法修复时返回 `None`。
    pub fn parse_repaired(&self) -> Option<Value> {
        if let Ok(value) = serde_json::from_str::<Value>(&self.buf) {
            return Some(value);
        }
        let repaired = repair_json(&self.buf);
        serde_json::from_str::<Value>(&repaired).ok()
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Object,
    Array,
}

/// 单遍结构扫描 + 确定性修复的执行器。
struct Repairer<'a> {
    chars: &'a [char],
    stack: Vec<(Kind, u8)>,
    in_string: bool,
    escape: bool,
    unicode_remaining: u8,
    last_backslash: Option<usize>,
    string_is_key: bool,
    last_good: Option<usize>,
    stack_at_good: Vec<Kind>,
    top_done: bool,
    i: usize,
}

impl<'a> Repairer<'a> {
    fn new(chars: &'a [char]) -> Self {
        Self {
            chars,
            stack: Vec::new(),
            in_string: false,
            escape: false,
            unicode_remaining: 0,
            last_backslash: None,
            string_is_key: false,
            last_good: None,
            stack_at_good: Vec::new(),
            top_done: false,
            i: 0,
        }
    }

    fn n(&self) -> usize {
        self.chars.len()
    }

    /// 当前栈容器类型的快照（不含 phase，仅用于补全闭合括号）。
    fn snapshot(&self) -> Vec<Kind> {
        self.stack.iter().map(|(k, _)| *k).collect()
    }

    /// 记录一个完整 value 的提交点，并更新父容器 phase（或标记顶层完成）。
    fn commit(&mut self, after: usize) {
        self.last_good = Some(after);
        self.stack_at_good = self.snapshot();
        if let Some(top) = self.stack.last_mut() {
            match top.0 {
                Kind::Object => top.1 = 3, // 期待 ',' 或 '}'
                Kind::Array => top.1 = 1,  // 期待 ',' 或 ']'
            }
        } else {
            self.top_done = true;
        }
    }

    /// 当前是否处于对象「期待 key」位置（用于区分字符串是 key 还是 value）。
    fn expects_key(&self) -> bool {
        matches!(self.stack.last(), Some((Kind::Object, 0)))
    }

    fn run(&mut self) {
        let n = self.n();
        while self.i < n {
            let c = self.chars[self.i];
            if self.in_string {
                self.string_step(c);
                continue;
            }
            if self.top_done {
                // 顶层已完整；仅允许尾随空白，其余视为垃圾停止
                if matches!(c, ' ' | '\t' | '\n' | '\r') {
                    self.i += 1;
                    continue;
                }
                break;
            }
            match c {
                ' ' | '\t' | '\n' | '\r' => {
                    self.i += 1;
                }
                '"' => {
                    self.string_is_key = self.expects_key();
                    self.in_string = true;
                    self.escape = false;
                    self.unicode_remaining = 0;
                    self.last_backslash = None;
                    self.i += 1;
                }
                '{' => {
                    self.stack.push((Kind::Object, 0));
                    self.last_good = Some(self.i + 1);
                    self.stack_at_good = self.snapshot();
                    self.i += 1;
                }
                '[' => {
                    self.stack.push((Kind::Array, 0));
                    self.last_good = Some(self.i + 1);
                    self.stack_at_good = self.snapshot();
                    self.i += 1;
                }
                '}' => {
                    let close_allowed = matches!(
                        self.stack.last(),
                        Some((Kind::Object, 0)) | Some((Kind::Object, 3))
                    );
                    if close_allowed {
                        self.stack.pop();
                        self.commit(self.i + 1);
                        self.i += 1;
                    } else {
                        break;
                    }
                }
                ']' => {
                    let close_allowed = matches!(
                        self.stack.last(),
                        Some((Kind::Array, 0)) | Some((Kind::Array, 1))
                    );
                    if close_allowed {
                        self.stack.pop();
                        self.commit(self.i + 1);
                        self.i += 1;
                    } else {
                        break;
                    }
                }
                ':' => {
                    if matches!(self.stack.last(), Some((Kind::Object, 1))) {
                        if let Some(top) = self.stack.last_mut() {
                            top.1 = 2;
                        }
                        self.i += 1;
                    } else {
                        break;
                    }
                }
                ',' => {
                    let ok = match self.stack.last_mut() {
                        Some((Kind::Object, phase)) if *phase == 3 => {
                            *phase = 0;
                            true
                        }
                        Some((Kind::Array, phase)) if *phase == 1 => {
                            *phase = 0;
                            true
                        }
                        _ => false,
                    };
                    if ok {
                        self.i += 1;
                    } else {
                        break;
                    }
                }
                '0'..='9' | '-' => {
                    if !self.handle_number() {
                        break;
                    }
                }
                't' => {
                    if !self.handle_keyword("true") {
                        break;
                    }
                }
                'f' => {
                    if !self.handle_keyword("false") {
                        break;
                    }
                }
                'n' => {
                    if !self.handle_keyword("null") {
                        break;
                    }
                }
                _ => break,
            }
        }
    }

    /// 处理字符串内的一个字符。
    fn string_step(&mut self, c: char) {
        if self.escape {
            if c == 'u' {
                self.unicode_remaining = 4;
            }
            self.escape = false;
            self.i += 1;
            return;
        }
        if self.unicode_remaining > 0 {
            if c.is_ascii_hexdigit() {
                self.unicode_remaining -= 1;
                if self.unicode_remaining == 0 {
                    self.last_backslash = None;
                }
            } else {
                // 非法 hex：放弃追踪（最终整体解析会失败）
                self.unicode_remaining = 0;
                self.last_backslash = None;
            }
            self.i += 1;
            return;
        }
        if c == '\\' {
            self.escape = true;
            self.last_backslash = Some(self.i);
            self.i += 1;
            return;
        }
        if c == '"' {
            self.in_string = false;
            let after = self.i + 1;
            if self.string_is_key {
                if let Some(top) = self.stack.last_mut() {
                    if top.0 == Kind::Object {
                        top.1 = 1; // 期待 ':'
                    }
                }
            } else {
                self.commit(after);
            }
            self.i = after;
            return;
        }
        self.i += 1;
    }

    /// 扫描一个数字。返回 (完整结束位置, 不完整时的最后有效数字位置)。
    fn handle_number(&mut self) -> bool {
        let (end, valid) = scan_number(self.chars, self.i);
        match end {
            Some(e) => {
                self.commit(e);
                self.i = e;
                true
            }
            None => {
                // EOF 截断：尽量取最后一个有效数字位置作为完整数字
                if let Some(v) = valid {
                    self.commit(v);
                }
                self.i = self.n();
                false
            }
        }
    }

    fn handle_keyword(&mut self, kw: &str) -> bool {
        match match_keyword(self.chars, self.i, kw) {
            Some(end) => {
                self.commit(end);
                self.i = end;
                true
            }
            None => {
                self.i = self.n();
                false
            }
        }
    }

    /// 基于扫描结果生成修复后的 JSON 文本。
    fn finish(self) -> String {
        if self.top_done {
            if let Some(g) = self.last_good {
                return self.chars[..g].iter().collect();
            }
            return String::new();
        }
        // 截断点 / 是否需要补 '"' / 待闭合的容器栈
        let (cut, need_quote, closers): (usize, bool, Vec<Kind>) = if self.in_string {
            if self.string_is_key {
                // 未闭合的 key 字符串：回退到上一个提交点（丢弃该 key）
                (
                    self.last_good.unwrap_or(0),
                    false,
                    self.stack_at_good.clone(),
                )
            } else {
                // 未闭合的 value 字符串：补 '"'，先丢弃悬挂的反斜杠/转义
                let effective = if self.escape || self.unicode_remaining > 0 {
                    self.last_backslash.unwrap_or(self.n())
                } else {
                    self.n()
                };
                (effective, true, self.snapshot())
            }
        } else {
            (
                self.last_good.unwrap_or(0),
                false,
                self.stack_at_good.clone(),
            )
        };

        let mut result: String = self.chars[..cut].iter().collect();
        if need_quote {
            result.push('"');
        }
        for kind in closers.iter().rev() {
            match kind {
                Kind::Object => result.push('}'),
                Kind::Array => result.push(']'),
            }
        }
        result
    }
}

/// 尽力修复 `input` 为合法 JSON 文本；若已是合法 JSON 则原样返回。
fn repair_json(input: &str) -> String {
    if serde_json::from_str::<Value>(input).is_ok() {
        return input.to_string();
    }
    let chars: Vec<char> = input.chars().collect();
    let mut repairer = Repairer::new(&chars);
    repairer.run();
    repairer.finish()
}

/// 扫描数字：返回 `(完整结束位置, EOF 截断时的最后有效数字结束位置)`。
fn scan_number(chars: &[char], start: usize) -> (Option<usize>, Option<usize>) {
    let n = chars.len();
    let mut i = start;
    let mut last_digit_end: Option<usize> = None;
    while i < n {
        match chars[i] {
            '0'..='9' => {
                last_digit_end = Some(i + 1);
                i += 1;
            }
            '-' | '+' | '.' | 'e' | 'E' => {
                i += 1;
            }
            _ => break,
        }
    }
    if i == n {
        (None, last_digit_end)
    } else if last_digit_end.is_some() {
        (Some(i), None)
    } else {
        (None, None)
    }
}

/// 尝试在 `start` 处匹配关键字，匹配完整则返回结束位置。
fn match_keyword(chars: &[char], start: usize, kw: &str) -> Option<usize> {
    let kw_chars: Vec<char> = kw.chars().collect();
    if start + kw_chars.len() > chars.len() {
        return None;
    }
    for (k, c) in kw_chars.iter().enumerate() {
        if chars[start + k] != *c {
            return None;
        }
    }
    Some(start + kw_chars.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn complete_json_parses() {
        let mut p = PartialJson::new();
        p.push(r#"{"a":1,"b":"two"}"#);
        assert!(p.parse_complete().is_ok());
        let repaired = p.parse_repaired().expect("repair ok");
        assert_eq!(repaired, serde_json::json!({"a": 1, "b": "two"}));
    }

    #[test]
    fn fragments_assemble_to_complete() {
        let mut p = PartialJson::new();
        p.push(r#"{"path":"#);
        p.push(r#""a"}"#);
        assert_eq!(p.as_buffer(), r#"{"path":"a"}"#);
        assert!(p.parse_complete().is_ok());
    }

    #[test]
    fn parallel_tool_calls_independent() {
        let mut a = PartialJson::new();
        let mut b = PartialJson::new();
        a.push(r#"{"id":"a","#);
        b.push(r#"{"id":"b","#);
        a.push(r#""v":1}"#);
        b.push(r#""v":2}"#);
        assert_eq!(
            a.parse_complete().unwrap(),
            serde_json::json!({"id": "a", "v": 1})
        );
        assert_eq!(
            b.parse_complete().unwrap(),
            serde_json::json!({"id": "b", "v": 2})
        );
    }

    #[test]
    fn repaired_unclosed_object() {
        let mut p = PartialJson::new();
        p.push(r#"{"a":1"#);
        let repaired = p.parse_repaired().expect("repair closes object");
        assert_eq!(repaired, serde_json::json!({"a": 1}));
    }

    #[test]
    fn repaired_unclosed_array() {
        let mut p = PartialJson::new();
        p.push(r#"[1, "two""#);
        let repaired = p.parse_repaired().expect("repair closes array");
        assert_eq!(repaired, serde_json::json!([1, "two"]));
    }

    #[test]
    fn repaired_unclosed_string_value() {
        let mut p = PartialJson::new();
        p.push(r#"{"a":"hel"#);
        let repaired = p.parse_repaired().expect("repair closes string");
        assert_eq!(repaired, serde_json::json!({"a": "hel"}));
    }

    #[test]
    fn repaired_dangling_backslash() {
        let mut p = PartialJson::new();
        p.push(r#"{"a":"x\"#);
        let repaired = p.parse_repaired().expect("repair drops dangling escape");
        assert_eq!(repaired, serde_json::json!({"a": "x"}));
    }

    #[test]
    fn repaired_trailing_comma() {
        let mut p = PartialJson::new();
        p.push(r#"{"a":1,"#);
        let repaired = p.parse_repaired().expect("repair drops trailing comma");
        assert_eq!(repaired, serde_json::json!({"a": 1}));
    }

    #[test]
    fn repaired_partial_key_dropped() {
        let mut p = PartialJson::new();
        p.push(r#"{"a":1,"b:"#);
        // 未完成的键值被丢弃，回退到上一对
        let repaired = p.parse_repaired().expect("repair ok");
        assert_eq!(repaired, serde_json::json!({"a": 1}));
    }

    #[test]
    fn malformed_unrepairable_returns_none() {
        let mut p = PartialJson::new();
        p.push("}");
        assert!(p.parse_repaired().is_none());
    }

    #[test]
    fn repaired_number_truncated_to_last_digit() {
        let mut p = PartialJson::new();
        p.push(r#"{"n":12."#);
        let repaired = p.parse_repaired().expect("repair truncates number");
        assert_eq!(repaired, serde_json::json!({"n": 12}));
    }

    proptest! {
        #[test]
        fn arbitrary_string_no_panic(s in any::<String>()) {
            let mut p = PartialJson::new();
            p.push(&s);
            let _ = p.parse_complete();
            let _ = p.parse_repaired();
        }
    }
}
