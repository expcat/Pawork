//! resource-loader 的中性 instruction DTO 到 ContextContribution 的映射。

use resource_loader::{ResourceInstruction, ResourceInstructionKind};

use crate::{ContextContribution, ContextSource};

pub fn contribution_from_resource(resource: &ResourceInstruction) -> ContextContribution {
    let source = match resource.kind {
        ResourceInstructionKind::AgentProfile => ContextSource::AgentProfile,
        ResourceInstructionKind::UserGlobalInstructions => ContextSource::UserGlobalInstructions,
        ResourceInstructionKind::WorkspaceInstructions => ContextSource::WorkspaceInstructions,
        ResourceInstructionKind::RootAgentsFile => ContextSource::RootAgentsFile,
        ResourceInstructionKind::PathAgentsFile => ContextSource::PathAgentsFile,
        ResourceInstructionKind::ActiveSkill => ContextSource::ActiveSkills,
        ResourceInstructionKind::PromptTemplate => ContextSource::PromptTemplate,
        ResourceInstructionKind::SessionInstructions | ResourceInstructionKind::RunInstructions => {
            ContextSource::AdHocInstructions
        }
    };
    // 同一 ContextSource 内显式编码 config tier，保证 session 永远先于 run，且不依赖
    // 调用方的扫描/插入顺序。
    let source_key = format!(
        "{:02}:{}:{}",
        resource.provenance.tier.priority(),
        resource.provenance.source_key,
        resource.resource_id
    );
    ContextContribution::new(source, source_key, resource.content.clone())
}

pub fn contributions_from_resources<'a>(
    resources: impl IntoIterator<Item = &'a ResourceInstruction>,
) -> Vec<ContextContribution> {
    let mut contributions = resources
        .into_iter()
        .map(contribution_from_resource)
        .collect::<Vec<_>>();
    crate::sort_contributions(&mut contributions);
    contributions
}

#[cfg(test)]
mod tests {
    use resource_loader::{ConfigTier, ResourceOrigin, ResourceProvenance};

    use super::*;

    fn instruction(
        kind: ResourceInstructionKind,
        tier: ConfigTier,
        key: &str,
    ) -> ResourceInstruction {
        ResourceInstruction {
            kind,
            resource_id: key.into(),
            content: key.into(),
            provenance: ResourceProvenance::new(
                tier,
                key,
                ResourceOrigin::Run { name: key.into() },
            ),
        }
    }

    #[test]
    fn resource_mapping_matches_context_source_contract() {
        let mappings = [
            (
                ResourceInstructionKind::AgentProfile,
                ContextSource::AgentProfile,
            ),
            (
                ResourceInstructionKind::UserGlobalInstructions,
                ContextSource::UserGlobalInstructions,
            ),
            (
                ResourceInstructionKind::WorkspaceInstructions,
                ContextSource::WorkspaceInstructions,
            ),
            (
                ResourceInstructionKind::RootAgentsFile,
                ContextSource::RootAgentsFile,
            ),
            (
                ResourceInstructionKind::PathAgentsFile,
                ContextSource::PathAgentsFile,
            ),
            (
                ResourceInstructionKind::ActiveSkill,
                ContextSource::ActiveSkills,
            ),
            (
                ResourceInstructionKind::PromptTemplate,
                ContextSource::PromptTemplate,
            ),
            (
                ResourceInstructionKind::RunInstructions,
                ContextSource::AdHocInstructions,
            ),
        ];
        for (kind, expected) in mappings {
            assert_eq!(
                contribution_from_resource(&instruction(kind, ConfigTier::Run, "x")).source,
                expected
            );
        }
    }

    /// 交叉校验两张优先级表（ResourceInstructionKind::priority 与
    /// ContextSource::priority）保持数值一致，防止静默的确定性漂移
    /// （P8 review §4.1）。
    #[test]
    fn priority_tables_stay_consistent() {
        let direct = [
            (
                ResourceInstructionKind::AgentProfile,
                ContextSource::AgentProfile,
                2,
            ),
            (
                ResourceInstructionKind::UserGlobalInstructions,
                ContextSource::UserGlobalInstructions,
                4,
            ),
            (
                ResourceInstructionKind::WorkspaceInstructions,
                ContextSource::WorkspaceInstructions,
                5,
            ),
            (
                ResourceInstructionKind::RootAgentsFile,
                ContextSource::RootAgentsFile,
                6,
            ),
            (
                ResourceInstructionKind::PathAgentsFile,
                ContextSource::PathAgentsFile,
                7,
            ),
            (
                ResourceInstructionKind::ActiveSkill,
                ContextSource::ActiveSkills,
                8,
            ),
            (
                ResourceInstructionKind::PromptTemplate,
                ContextSource::PromptTemplate,
                9,
            ),
        ];
        for (kind, source, expected) in direct {
            assert_eq!(kind.priority(), expected, "{kind:?} 优先级漂移");
            assert_eq!(source.priority(), expected, "{source:?} 优先级漂移");
            assert_eq!(
                kind.priority(),
                source.priority(),
                "{kind:?} 与 {source:?} 两张优先级表不一致"
            );
        }

        // SessionInstructions(13) / RunInstructions(14) 有意重映射到
        // ContextSource::AdHocInstructions(14)（P8 §4.2），不参与直接映射。
        assert_eq!(ResourceInstructionKind::SessionInstructions.priority(), 13);
        assert_eq!(ResourceInstructionKind::RunInstructions.priority(), 14);
        assert_eq!(ContextSource::AdHocInstructions.priority(), 14);
    }

    #[test]
    fn mapping_is_deterministic_for_reversed_input() {
        let resources = vec![
            instruction(
                ResourceInstructionKind::RunInstructions,
                ConfigTier::Run,
                "run",
            ),
            instruction(
                ResourceInstructionKind::SessionInstructions,
                ConfigTier::Session,
                "session",
            ),
            instruction(
                ResourceInstructionKind::WorkspaceInstructions,
                ConfigTier::Workspace,
                "workspace",
            ),
        ];
        let mut reversed = resources.clone();
        reversed.reverse();
        assert_eq!(
            contributions_from_resources(&resources),
            contributions_from_resources(&reversed)
        );
        let ordered = contributions_from_resources(&resources);
        assert_eq!(ordered[1].content, "session");
        assert_eq!(ordered[2].content, "run");
    }
}
