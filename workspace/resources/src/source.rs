use pawork_config::ConfigTier;
use serde::{Deserialize, Serialize};

/// 可加载的 canonical 资源类别。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceKind {
    Instructions,
    AgentsFile,
    Skill,
    PromptTemplate,
    AgentProfile,
    LanguageServer,
    UserHook,
}

impl ResourceKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Instructions => "instructions",
            Self::AgentsFile => "agents_file",
            Self::Skill => "skill",
            Self::PromptTemplate => "prompt_template",
            Self::AgentProfile => "agent_profile",
            Self::LanguageServer => "language_server",
            Self::UserHook => "user_hook",
        }
    }
}

/// 不暴露宿主绝对路径的资源位置。
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "scope", rename_all = "snake_case")]
pub enum ResourceOrigin {
    Global {
        relative_path: String,
    },
    Workspace {
        root_index: usize,
        relative_path: String,
    },
    Session {
        name: String,
    },
    Run {
        name: String,
    },
}

/// 每个资源都携带确定性层级、排序键和安全来源。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceProvenance {
    pub tier: ConfigTier,
    pub source_key: String,
    pub origin: ResourceOrigin,
}

impl ResourceProvenance {
    pub fn new(tier: ConfigTier, source_key: impl Into<String>, origin: ResourceOrigin) -> Self {
        Self {
            tier,
            source_key: source_key.into(),
            origin,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceDiagnosticStatus {
    Loaded,
    Active,
    Overridden,
    Disabled,
    Rejected,
}

/// 诊断视图只包含元数据，不包含 instruction、prompt 或脚本正文。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceDiagnosticEntry {
    pub kind: ResourceKind,
    pub resource_id: String,
    pub status: ResourceDiagnosticStatus,
    pub provenance: ResourceProvenance,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceIssueSeverity {
    Warning,
    Error,
}

/// 单个资源失败或冲突；消息不得包含资源正文或 Secret。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceIssue {
    pub severity: ResourceIssueSeverity,
    pub code: String,
    pub kind: Option<ResourceKind>,
    pub resource_id: Option<String>,
    pub source_key: Option<String>,
    pub message: String,
}

impl ResourceIssue {
    pub fn warning(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            severity: ResourceIssueSeverity::Warning,
            code: code.into(),
            kind: None,
            resource_id: None,
            source_key: None,
            message: message.into(),
        }
    }

    pub fn error(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            severity: ResourceIssueSeverity::Error,
            code: code.into(),
            kind: None,
            resource_id: None,
            source_key: None,
            message: message.into(),
        }
    }

    pub fn for_resource(
        mut self,
        kind: ResourceKind,
        resource_id: impl Into<String>,
        source_key: impl Into<String>,
    ) -> Self {
        self.kind = Some(kind);
        self.resource_id = Some(resource_id.into());
        self.source_key = Some(source_key.into());
        self
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceDiagnostics {
    pub entries: Vec<ResourceDiagnosticEntry>,
    pub issues: Vec<ResourceIssue>,
}

impl ResourceDiagnostics {
    pub fn sort_deterministically(&mut self) {
        self.entries.sort_by(|left, right| {
            left.kind
                .cmp(&right.kind)
                .then_with(|| left.resource_id.cmp(&right.resource_id))
                .then_with(|| left.provenance.tier.cmp(&right.provenance.tier))
                .then_with(|| left.provenance.source_key.cmp(&right.provenance.source_key))
                .then_with(|| left.status.cmp(&right.status))
        });
        self.issues.sort_by(|left, right| {
            left.severity
                .cmp(&right.severity)
                .then_with(|| left.code.cmp(&right.code))
                .then_with(|| left.kind.cmp(&right.kind))
                .then_with(|| left.resource_id.cmp(&right.resource_id))
                .then_with(|| left.source_key.cmp(&right.source_key))
                .then_with(|| left.message.cmp(&right.message))
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostics_sort_without_insertion_order() {
        let provenance = |key: &str| {
            ResourceProvenance::new(
                ConfigTier::Workspace,
                key,
                ResourceOrigin::Workspace {
                    root_index: 0,
                    relative_path: key.into(),
                },
            )
        };
        let mut left = ResourceDiagnostics {
            entries: vec![
                ResourceDiagnosticEntry {
                    kind: ResourceKind::Skill,
                    resource_id: "z".into(),
                    status: ResourceDiagnosticStatus::Active,
                    provenance: provenance("z"),
                },
                ResourceDiagnosticEntry {
                    kind: ResourceKind::Skill,
                    resource_id: "a".into(),
                    status: ResourceDiagnosticStatus::Loaded,
                    provenance: provenance("a"),
                },
            ],
            issues: vec![
                ResourceIssue::warning("z", "later"),
                ResourceIssue::error("a", "first"),
            ],
        };
        let mut right = left.clone();
        right.entries.reverse();
        right.issues.reverse();
        left.sort_deterministically();
        right.sort_deterministically();
        assert_eq!(left, right);
    }
}
