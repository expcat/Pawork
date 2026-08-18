//! 优先级与冲突裁决（P8-6 确定性）。
//!
//! 同一 (category, id) 的多个来源条目：ConfigTier 优先级高者胜，同 tier 按
//! 外部来源 rank，再按来源相对路径字典序，保证同输入同输出。落败条目转为
//! Conflict 并保留来源诊断，不静默覆盖、不复制落败者内容。

use std::collections::BTreeMap;

use super::model::{CompatIssue, CompatItem, ImportCategory, ImportStatus};

pub(crate) fn resolve_conflicts(
    items: Vec<CompatItem>,
    issues: &mut [CompatIssue],
) -> Vec<CompatItem> {
    let mut groups: BTreeMap<(ImportCategory, String), Vec<CompatItem>> = BTreeMap::new();
    for item in items {
        groups
            .entry((item.category, item.id.clone()))
            .or_default()
            .push(item);
    }
    let mut resolved = Vec::new();
    for ((category, id), mut group) in groups {
        let contested = group.len() > 1;
        group.sort_by(|left, right| {
            priority_key(left)
                .cmp(&priority_key(right))
                .then_with(|| left.source.relative_path.cmp(&right.source.relative_path))
        });
        let mut winner = group.pop().expect("conflict group is non-empty");
        // 同一 (category, id) 多来源竞争时保留确定性胜者，但必须人工审查后再启用。
        if contested {
            winner.requires_review = true;
        }
        for mut loser in group {
            let winner_description = format!(
                "{}:{}",
                winner.source.external.as_str(),
                winner.source.relative_path
            );
            loser.status = ImportStatus::Conflict;
            loser.payload = None;
            loser.issues.push(
                CompatIssue::warning(
                    "conflict_loser",
                    format!(
                        "{} {} already imported from {} (tier {}); deterministic winner kept",
                        category.as_str(),
                        id,
                        winner_description,
                        winner.source.tier.priority(),
                    ),
                )
                .for_item(category, id.clone(), loser.source.relative_path.clone()),
            );
            resolved.push(loser);
        }
        resolved.push(winner);
    }
    issues.sort_by(|left, right| {
        left.severity
            .cmp(&right.severity)
            .then_with(|| left.code.cmp(&right.code))
            .then_with(|| left.source_path.cmp(&right.source_path))
            .then_with(|| left.message.cmp(&right.message))
    });
    resolved
}

fn priority_key(item: &CompatItem) -> (u8, u8) {
    (item.source.tier.priority(), item.source.external.rank())
}
