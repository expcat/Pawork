//! Unified diff 解析器：把 `git diff` 的 patch 文本解析为 [`DiffHunk`] 列表。
//!
//! 纯字符串状态机，无正则，100k 行单线程解析远低于 500ms。处理：
//! - hunk 头 `@@ -o,ol +n,nl @@ optional`；
//! - 行前缀 ` `(context)/`-`(del)/`+`(add)；
//! - `\ No newline at end of file`：标记到其**上一行**；按该行类型区分旧侧
//!   (`old_no_newline`，作用于 `Deletion` / `Context`) 与新侧
//!   (`new_no_newline`，作用于 `Addition` / `Context`)，`Context` 行两侧一致；
//! - 文件级头（`diff --git`、`--- `、`+++ `、`index`、`new file mode`、
//!   `deleted file mode`、`Binary files`、`similarity`、`rename from/to`）在
//!   [`crate::service`] 内部跳过/用于标记 binary，不进 hunks。

use crate::model::{DiffHunk, DiffLine, HunkId, LineKind};

/// 解析 unified patch 为 hunks，HunkId 从 0 起自增。
pub fn parse_unified(patch: &str) -> Vec<DiffHunk> {
    parse_unified_with_start(patch, 0).0
}

/// 解析 unified patch，HunkId 从 `start` 起自增，返回 (hunks, next_id)。
pub fn parse_unified_with_start(patch: &str, start: u64) -> (Vec<DiffHunk>, u64) {
    let mut hunks = Vec::new();
    let mut next_id = start;
    // 当前 hunk。
    let mut cur: Option<DiffHunk> = None;

    for raw in patch.lines() {
        // hunk 头：开新 hunk。
        if raw.strip_prefix("@@").is_some() {
            // 上一个 hunk 收尾：无末尾换行标记已在遇到该标记行时即时落到对应行。
            if let Some(h) = cur.take() {
                hunks.push(h);
            }

            let (old_start, old_lines, new_start, new_lines, full_header) = parse_hunk_header(raw);
            cur = Some(DiffHunk {
                id: HunkId(next_id),
                old_start,
                old_lines,
                new_start,
                new_lines,
                header: full_header,
                lines: Vec::new(),
            });
            next_id += 1;
            continue;
        }

        // 还未进入任何 hunk（仍是文件头行），跳过。
        let h = match cur.as_mut() {
            Some(h) => h,
            None => continue,
        };

        // 无末尾换行标记：作用到其**上一行**（hunk 当前最后一行），按该行类型
        // 区分旧侧 / 新侧；Context 行两侧一致。
        if raw.starts_with("\\ No newline at end of file") {
            apply_no_newline_marker(h);
            continue;
        }

        // diff 内容行。
        let (kind, text) = match raw.chars().next() {
            Some('+') => (LineKind::Addition, &raw[1..]),
            Some('-') => (LineKind::Deletion, &raw[1..]),
            Some(' ') => (LineKind::Context, &raw[1..]),
            _ => {
                // 其它行（如残留的文件头 / 无前缀行）忽略，避免误判。
                continue;
            }
        };
        h.lines.push(DiffLine {
            kind,
            text: text.to_string(),
            old_no_newline: false,
            new_no_newline: false,
        });
    }

    // 收尾最后一个 hunk。
    if let Some(h) = cur.take() {
        hunks.push(h);
    }
    (hunks, next_id)
}

/// 把 `\ No newline at end of file` 标记作用到 hunk 当前最后一行（即标记的上一行）。
///
/// 按该行类型选择标记侧：`Deletion` → 仅旧侧；`Addition` → 仅新侧；
/// `Context` → 两侧一致（同时标记）。无最后一行时为无操作。
fn apply_no_newline_marker(hunk: &mut DiffHunk) {
    if let Some(last) = hunk.lines.last_mut() {
        match last.kind {
            LineKind::Deletion => last.old_no_newline = true,
            LineKind::Addition => last.new_no_newline = true,
            LineKind::Context => {
                last.old_no_newline = true;
                last.new_no_newline = true;
            }
        }
    }
}

/// 解析 hunk 头 `@@ -old_start,old_lines +new_start,new_lines @@ ...`。
fn parse_hunk_header(raw: &str) -> (u32, u32, u32, u32, String) {
    let full = raw.to_string();
    // 形如 "@@ -1,3 +1,4 @@ func"：分别定位 "-<n>,<m>" 与 "+<n>,<m>" 两段。
    let o = parse_signed_range(raw, b'-').unwrap_or((0, 0));
    let n = parse_signed_range(raw, b'+').unwrap_or((0, 0));
    (o.0, o.1, n.0, n.1, full)
}

/// 从 hunk 头中找由 `sign`（`b'-'`/`b'+'`）引导的范围（如 "-1,3"）。
/// 仅匹配「符号 + 数字」且前一字符非数字的位置，避免误命中数字内部。
fn parse_signed_range(raw: &str, sign: u8) -> Option<(u32, u32)> {
    let bytes = raw.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == sign && i + 1 < bytes.len() && bytes[i + 1].is_ascii_digit() {
            let prev_ok = i == 0 || !bytes[i - 1].is_ascii_digit();
            if prev_ok {
                return parse_range(&raw[i + 1..]);
            }
        }
        i += 1;
    }
    None
}

/// 从 "1,3 @@ ..." 形如的片段解析 (start, lines)；缺省 lines 视为 1。
fn parse_range(s: &str) -> Option<(u32, u32)> {
    let s = s.trim_start_matches(['-', '+']).trim_start();
    let end = s
        .find(|c: char| c.is_whitespace() || c == '@')
        .unwrap_or(s.len());
    let token = &s[..end];
    let (start_str, lines_str) = token.split_once(',').unwrap_or((token, "1"));
    let start: u32 = start_str.parse().ok()?;
    let lines: u32 = lines_str.parse().unwrap_or(1);
    Some((start, lines))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_basic_hunk() {
        let patch = "\
diff --git a/f.txt b/f.txt
--- a/f.txt
+++ b/f.txt
@@ -1,3 +1,4 @@
 a
-b
+B
 c
+d
";
        let hunks = parse_unified(patch);
        assert_eq!(hunks.len(), 1);
        let h = &hunks[0];
        assert_eq!(h.old_start, 1);
        assert_eq!(h.old_lines, 3);
        assert_eq!(h.new_start, 1);
        assert_eq!(h.new_lines, 4);
        assert_eq!(h.header, "@@ -1,3 +1,4 @@");
        assert_eq!(h.lines.len(), 5);
        assert_eq!(h.lines[0].kind, LineKind::Context);
        assert_eq!(h.lines[1].kind, LineKind::Deletion);
        assert_eq!(h.lines[1].text, "b");
        assert_eq!(h.lines[2].kind, LineKind::Addition);
        assert_eq!(h.lines[2].text, "B");
    }

    #[test]
    fn parses_no_newline_at_end() {
        let patch = "\
--- a/f.txt
+++ b/f.txt
@@ -1 +1 @@
-xyz
\\ No newline at end of file
+abc
\\ No newline at end of file
";
        let hunks = parse_unified(patch);
        assert_eq!(hunks.len(), 1);
        let lines = &hunks[0].lines;
        // 删除行：标记旧侧无末尾换行；新侧仍有换行。
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].kind, LineKind::Deletion);
        assert!(
            lines[0].old_no_newline,
            "deletion line should be old-side no-newline"
        );
        assert!(
            !lines[0].new_no_newline,
            "deletion line new side should have newline"
        );
        // 新增行：标记新侧无末尾换行；旧侧仍有换行。
        assert_eq!(lines[1].kind, LineKind::Addition);
        assert!(
            lines[1].new_no_newline,
            "addition line should be new-side no-newline"
        );
        assert!(
            !lines[1].old_no_newline,
            "addition line old side should have newline"
        );
    }

    #[test]
    fn parses_context_no_newline_marks_both_sides() {
        // 旧、新文件均以无末尾换行的相同末行结尾 → context 行两侧一致标记。
        let patch = "\
--- a/f.txt
+++ b/f.txt
@@ -1,2 +1,2 @@
 a
 z
\\ No newline at end of file
";
        let hunks = parse_unified(patch);
        assert_eq!(hunks.len(), 1);
        let lines = &hunks[0].lines;
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[1].kind, LineKind::Context);
        assert!(
            lines[1].old_no_newline,
            "context no-newline should mark old side"
        );
        assert!(
            lines[1].new_no_newline,
            "context no-newline should mark new side"
        );
        assert!(
            !lines[0].old_no_newline && !lines[0].new_no_newline,
            "non-final context line should have no no-newline flags"
        );
    }

    #[test]
    fn hunk_ids_increment_with_start() {
        let patch = "@@ -1,1 +1,1 @@\n x\n";
        let (hunks, next) = parse_unified_with_start(patch, 10);
        assert_eq!(hunks.len(), 1);
        assert_eq!(hunks[0].id, HunkId(10));
        assert_eq!(next, 11);
    }

    #[test]
    fn parses_large_diff_under_500ms() {
        // 构造 100,000 行的 patch（单个 hunk，混合 add/del/context）。
        let mut patch = String::from("--- a/big.txt\n+++ b/big.txt\n@@ -1,100000 +1,100000 @@\n");
        for i in 0..50_000 {
            patch.push_str(&format!("-old line {i}\n"));
            patch.push_str(&format!("+new line {i}\n"));
        }
        let start = std::time::Instant::now();
        let hunks = parse_unified(&patch);
        let elapsed = start.elapsed();
        assert_eq!(hunks.len(), 1);
        assert_eq!(hunks[0].lines.len(), 100_000);
        assert!(
            elapsed.as_millis() < 500,
            "parsing 100k lines took {:?}, expected < 500ms",
            elapsed
        );
    }
}
