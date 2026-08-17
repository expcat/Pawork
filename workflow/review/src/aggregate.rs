//! 聚合与 diff 导入：按 file / severity / status 聚合 findings；
//! 导入 diff 范围生成待发布评论（仅生成，不自动发布）。

use pawork_domain::{ReviewAnchor, ReviewResolution, ReviewSeverity};
use pawork_git::{DiffFile, LineKind};

use crate::model::{GroupCount, PendingComment, ReviewFinding};

/// 聚合维度。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AggregateBy {
    File,
    Severity,
    Status,
}

impl AggregateBy {
    pub fn label(self) -> &'static str {
        match self {
            AggregateBy::File => "file",
            AggregateBy::Severity => "severity",
            AggregateBy::Status => "status",
        }
    }
}

pub fn severity_key(severity: ReviewSeverity) -> &'static str {
    match severity {
        ReviewSeverity::Info => "info",
        ReviewSeverity::Minor => "minor",
        ReviewSeverity::Major => "major",
        ReviewSeverity::Critical => "critical",
    }
}

pub fn resolution_key(resolution: ReviewResolution) -> &'static str {
    match resolution {
        ReviewResolution::Open => "open",
        ReviewResolution::Addressed => "addressed",
        ReviewResolution::Resolved => "resolved",
        ReviewResolution::Wontfix => "wontfix",
    }
}

fn group_key(by: AggregateBy, finding: &ReviewFinding) -> String {
    match by {
        AggregateBy::File => finding.anchor.file.clone(),
        AggregateBy::Severity => severity_key(finding.severity).to_string(),
        AggregateBy::Status => resolution_key(finding.resolution).to_string(),
    }
}

/// 按维度聚合 findings（key 字典序，确定性输出）。
pub fn aggregate<'a>(
    findings: impl IntoIterator<Item = &'a ReviewFinding>,
    by: AggregateBy,
) -> Vec<GroupCount> {
    let mut map = std::collections::BTreeMap::<String, GroupCount>::new();
    for finding in findings {
        let key = group_key(by, finding);
        let entry = map.entry(key.clone()).or_insert_with(|| GroupCount {
            key,
            total: 0,
            open: 0,
            addressed: 0,
            resolved: 0,
            wontfix: 0,
        });
        entry.total += 1;
        match finding.resolution {
            ReviewResolution::Open => entry.open += 1,
            ReviewResolution::Addressed => entry.addressed += 1,
            ReviewResolution::Resolved => entry.resolved += 1,
            ReviewResolution::Wontfix => entry.wontfix += 1,
        }
    }
    map.into_values().collect()
}

/// 由 findings 生成待发布评论（仅 open / addressed，不自动发布）。
pub fn pending_comments<'a>(
    findings: impl IntoIterator<Item = &'a ReviewFinding>,
) -> Vec<PendingComment> {
    findings
        .into_iter()
        .filter(|f| f.is_open())
        .map(|f| PendingComment {
            finding_id: f.finding_id.clone(),
            anchor: f.anchor.clone(),
            body: f.body.clone(),
        })
        .collect()
}

/// diff 变更行草稿（导入时生成 finding 的输入）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FindingDraft {
    pub anchor: ReviewAnchor,
    pub severity: ReviewSeverity,
    pub body: String,
    pub evidence: Vec<String>,
}

/// 把 DiffFile 的变更行映射为 finding 草稿：每条 Addition 行一个锚点
/// （新侧行号），body 带行文本，evidence 记录 `diff:<path>:<line>`。
pub fn drafts_from_diff(diff: &DiffFile) -> Vec<FindingDraft> {
    let mut drafts = Vec::new();
    for hunk in &diff.hunks {
        let mut new_line = hunk.new_start;
        for line in &hunk.lines {
            match line.kind {
                LineKind::Context | LineKind::Addition => {
                    if line.kind == LineKind::Addition {
                        drafts.push(FindingDraft {
                            anchor: ReviewAnchor {
                                file: diff.path.clone(),
                                line: new_line,
                                end_line: Some(new_line),
                            },
                            severity: ReviewSeverity::Info,
                            body: format!("diff 变更行：{}", line.text),
                            evidence: vec![format!("diff:{}:{}", diff.path, new_line)],
                        });
                    }
                    new_line += 1;
                }
                LineKind::Deletion => {
                    // 旧侧行：不在新侧生成锚点。
                }
            }
        }
    }
    drafts
}
