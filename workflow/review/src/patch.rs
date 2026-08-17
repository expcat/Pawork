//! SuggestedPatch 的 dry-run：只校验 / 解析 / 内存试应用，不落盘。
//!
//! 实际应用交既有工具 + policy（checkpoint / sandbox），本模块不产生任何写。

use pawork_git::diff::{parse_unified, LineKind};
use serde::{Deserialize, Serialize};

use crate::{error::ReviewError, model::SuggestedPatch};

/// dry-run 报告。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PatchReport {
    pub hunks: usize,
    pub additions: u32,
    pub deletions: u32,
    /// 内存试应用后的新内容（校验通过时才有）。
    pub applied_text: Option<String>,
}

/// 补丁 dry-run 校验器（纯内存，无 IO）。
pub struct PatchValidator;

impl PatchValidator {
    /// 只校验 / 解析（不落盘）：hunk 头行数与内容行数必须一致。
    pub fn dry_run(patch: &SuggestedPatch) -> Result<PatchReport, ReviewError> {
        let hunks = parse_unified(&patch.payload);
        if hunks.is_empty() {
            return Err(ReviewError::InvalidPatch(
                "补丁未解析出任何 hunk".to_string(),
            ));
        }
        let mut additions = 0u32;
        let mut deletions = 0u32;
        for hunk in &hunks {
            let old_side = hunk
                .lines
                .iter()
                .filter(|l| matches!(l.kind, LineKind::Context | LineKind::Deletion))
                .count();
            let new_side = hunk
                .lines
                .iter()
                .filter(|l| matches!(l.kind, LineKind::Context | LineKind::Addition))
                .count();
            if old_side != hunk.old_lines as usize || new_side != hunk.new_lines as usize {
                return Err(ReviewError::InvalidPatch(format!(
                    "hunk {} 行数不一致：old {}!={} new {}!={}",
                    hunk.id.0, old_side, hunk.old_lines, new_side, hunk.new_lines
                )));
            }
            additions += hunk
                .lines
                .iter()
                .filter(|l| l.kind == LineKind::Addition)
                .count() as u32;
            deletions += hunk
                .lines
                .iter()
                .filter(|l| l.kind == LineKind::Deletion)
                .count() as u32;
        }
        Ok(PatchReport {
            hunks: hunks.len(),
            additions,
            deletions,
            applied_text: None,
        })
    }

    /// 对 `current` 做内存内试应用（不写盘），并校验 context / deletion 行与
    /// 当前内容匹配。返回应用后的新内容。
    pub fn apply_in_memory(patch: &SuggestedPatch, current: &str) -> Result<String, ReviewError> {
        let hunks = parse_unified(&patch.payload);
        if hunks.is_empty() {
            return Err(ReviewError::InvalidPatch(
                "补丁未解析出任何 hunk".to_string(),
            ));
        }
        let mut lines: Vec<String> = current.split('\n').map(str::to_string).collect();
        if lines.last().map(String::as_str) == Some("") {
            lines.pop();
        }
        for hunk in &hunks {
            let start = if hunk.old_start == 0 {
                0
            } else {
                (hunk.old_start - 1) as usize
            };
            let end = start + hunk.old_lines as usize;
            if end > lines.len() {
                return Err(ReviewError::InvalidPatch(format!(
                    "hunk {} 超出文件范围（old_start={} old_lines={}，当前 {} 行）",
                    hunk.id.0,
                    hunk.old_start,
                    hunk.old_lines,
                    lines.len()
                )));
            }
            let mut idx = start;
            let mut block: Vec<String> = Vec::new();
            for line in &hunk.lines {
                match line.kind {
                    LineKind::Context | LineKind::Deletion => {
                        let actual =
                            lines
                                .get(idx)
                                .ok_or_else(|| ReviewError::PatchContextMismatch {
                                    position: format!("{}:{}", patch.file, idx + 1),
                                    expected: line.text.clone(),
                                    found: String::new(),
                                })?;
                        if actual != &line.text {
                            return Err(ReviewError::PatchContextMismatch {
                                position: format!("{}:{}", patch.file, idx + 1),
                                expected: line.text.clone(),
                                found: actual.to_string(),
                            });
                        }
                        // Context 行两侧保留；Deletion 行仅校验后丢弃。
                        if line.kind == LineKind::Context {
                            block.push(line.text.clone());
                        }
                        idx += 1;
                    }
                    LineKind::Addition => block.push(line.text.clone()),
                }
            }
            lines.splice(start..idx, block);
        }
        Ok(lines.join("\n"))
    }
}
