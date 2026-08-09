//! Resource Diagnostics 安全视图。

use diagnostics::Redactor;
use serde::{Deserialize, Serialize};

use crate::{ResourceDiagnosticEntry, ResourceDiagnostics, ResourceIssue, ResourceOrigin};

/// 可供 CLI/GUI/诊断包消费的 allowlist 视图。
///
/// 只包含来源元数据、状态和脱敏后的问题；不包含 instruction、prompt、Skill 正文、
/// 脚本正文或任意宿主绝对路径。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceDiagnosticView {
    pub sources: Vec<ResourceDiagnosticEntry>,
    pub issues: Vec<ResourceIssue>,
}

impl ResourceDiagnosticView {
    pub fn build(diagnostics: &ResourceDiagnostics, redactor: &Redactor) -> Self {
        let mut normalized = diagnostics.clone();
        normalized.sort_deterministically();
        for source in &mut normalized.entries {
            source.resource_id = redactor.redact(&source.resource_id);
            source.provenance.source_key = redactor.redact(&source.provenance.source_key);
            source.provenance.origin = redact_origin(&source.provenance.origin, redactor);
        }
        for issue in &mut normalized.issues {
            issue.code = redactor.redact(&issue.code);
            issue.message = redactor.redact(&issue.message);
            issue.resource_id = issue
                .resource_id
                .as_ref()
                .map(|value| redactor.redact(value));
            issue.source_key = issue
                .source_key
                .as_ref()
                .map(|value| redactor.redact(value));
        }
        Self {
            sources: normalized.entries,
            issues: normalized.issues,
        }
    }

    /// 稳定的人类可读视图，用于回答「为什么这条资源生效」。
    pub fn render_text(&self) -> String {
        let mut lines = Vec::with_capacity(self.sources.len() + self.issues.len());
        for source in &self.sources {
            lines.push(format!(
                "{} {} [{}:{}] {}",
                source.kind.as_str(),
                escape_controls(&source.resource_id),
                source.provenance.tier.as_str(),
                escape_controls(&source.provenance.source_key),
                origin_label(&source.provenance.origin),
            ));
        }
        for issue in &self.issues {
            lines.push(format!(
                "issue {} {}{}",
                issue.code,
                escape_controls(&issue.message),
                issue
                    .source_key
                    .as_ref()
                    .map(|key| format!(" ({})", escape_controls(key)))
                    .unwrap_or_default(),
            ));
        }
        lines.join("\n")
    }
}

fn redact_origin(origin: &ResourceOrigin, redactor: &Redactor) -> ResourceOrigin {
    match origin {
        ResourceOrigin::Global { relative_path } => ResourceOrigin::Global {
            relative_path: redactor.redact(relative_path),
        },
        ResourceOrigin::Workspace {
            root_index,
            relative_path,
        } => ResourceOrigin::Workspace {
            root_index: *root_index,
            relative_path: redactor.redact(relative_path),
        },
        ResourceOrigin::Session { name } => ResourceOrigin::Session {
            name: redactor.redact(name),
        },
        ResourceOrigin::Run { name } => ResourceOrigin::Run {
            name: redactor.redact(name),
        },
    }
}

fn origin_label(origin: &ResourceOrigin) -> String {
    match origin {
        ResourceOrigin::Global { relative_path } => {
            format!("global:{}", escape_controls(relative_path))
        }
        ResourceOrigin::Workspace {
            root_index,
            relative_path,
        } => format!("workspace[{root_index}]:{}", escape_controls(relative_path)),
        ResourceOrigin::Session { name } => format!("session:{}", escape_controls(name)),
        ResourceOrigin::Run { name } => format!("run:{}", escape_controls(name)),
    }
}

fn escape_controls(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    for character in value.chars() {
        if character.is_control() {
            output.extend(character.escape_default());
        } else {
            output.push(character);
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use config_service::ConfigTier;

    use super::*;
    use crate::{
        ResourceDiagnosticStatus, ResourceIssueSeverity, ResourceKind, ResourceProvenance,
    };

    #[test]
    fn view_lists_sources_without_content_and_redacts_issues() {
        let secret = "sk-abcdefghijklmnop";
        let diagnostics = ResourceDiagnostics {
            entries: vec![ResourceDiagnosticEntry {
                kind: ResourceKind::Skill,
                resource_id: format!("review-{secret}"),
                status: ResourceDiagnosticStatus::Active,
                provenance: ResourceProvenance::new(
                    ConfigTier::Workspace,
                    format!("workspace:skill:review-{secret}"),
                    ResourceOrigin::Workspace {
                        root_index: 0,
                        relative_path: format!(".pawork/skills/{secret}/manifest.toml"),
                    },
                ),
            }],
            issues: vec![ResourceIssue {
                severity: ResourceIssueSeverity::Error,
                code: format!("load_failed_{secret}"),
                kind: Some(ResourceKind::Skill),
                resource_id: Some(format!("review-{secret}")),
                source_key: Some(format!("workspace:skill:review-{secret}")),
                message: format!("Authorization: Bearer {secret}"),
            }],
        };
        let view = ResourceDiagnosticView::build(&diagnostics, &Redactor::default());
        let json = serde_json::to_string(&view).expect("serialize");
        assert!(!json.contains(secret), "{json}");
        assert!(!json.contains("instruction body"));
        let text = view.render_text();
        assert!(text.contains("workspace[0]:.pawork/skills/[REDACTED]/manifest.toml"));
        assert!(text.contains("[REDACTED]"));
    }
}
