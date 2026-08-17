//! unified parser golden + proptest：任意字符串不 panic。

use pawork_git::diff::{parse_unified, LineKind};
use proptest::prelude::*;

const BASIC_HUNK: &str = include_str!("golden/basic_hunk.diff");
const NO_NEWLINE: &str = include_str!("golden/no_newline.diff");
const CONTEXT_NO_NEWLINE: &str = include_str!("golden/context_no_newline.diff");

#[test]
fn golden_basic_hunk_is_stable() {
    let hunks = parse_unified(BASIC_HUNK);
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
fn golden_no_newline_marks_previous_line() {
    let hunks = parse_unified(NO_NEWLINE);
    assert_eq!(hunks.len(), 1);
    let lines = &hunks[0].lines;
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0].kind, LineKind::Deletion);
    assert!(lines[0].old_no_newline);
    assert!(!lines[0].new_no_newline);
    assert_eq!(lines[1].kind, LineKind::Addition);
    assert!(lines[1].new_no_newline);
    assert!(!lines[1].old_no_newline);
}

#[test]
fn golden_context_no_newline_marks_both_sides() {
    let hunks = parse_unified(CONTEXT_NO_NEWLINE);
    assert_eq!(hunks.len(), 1);
    let lines = &hunks[0].lines;
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[1].kind, LineKind::Context);
    assert!(lines[1].old_no_newline);
    assert!(lines[1].new_no_newline);
    assert!(!lines[0].old_no_newline && !lines[0].new_no_newline);
}

proptest! {
    #[test]
    fn parse_unified_never_panics(s in "\\PC{0,512}") {
        let _ = parse_unified(&s);
    }

    #[test]
    fn parse_unified_never_panics_on_bytes(bytes in prop::collection::vec(any::<u8>(), 0..512)) {
        let s = String::from_utf8_lossy(&bytes);
        let _ = parse_unified(&s);
    }
}
