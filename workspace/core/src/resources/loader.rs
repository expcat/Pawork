//! Resource Loader 聚合入口。

use std::path::{Component, Path, PathBuf};

use crate::config::ConfigTier;
use pawork_domain::WorkspaceId;
use crate::WorkspaceService;
use serde::{Deserialize, Serialize};

use super::{
    agents::load_agents_hierarchy,
    io::{join_under_root, read_utf8_bounded_within},
    profiles::{load_profiles, resolve_profile_references, ProfileResolution},
    AgentProfile, AgentsHierarchy, LoadedAgentProfileV2, ResolvedInstructions,
    ResourceDiagnosticEntry, ResourceDiagnosticStatus, ResourceDiagnostics, ResourceIssue,
    ResourceKind, ResourceLimits, ResourceLoadError, ResourceLoaderOptions, ResourceOrigin,
    ResourceProvenance, ResourceRequest, SkillResolution,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceInstructionKind {
    AgentProfile,
    UserGlobalInstructions,
    WorkspaceInstructions,
    RootAgentsFile,
    PathAgentsFile,
    ActiveSkill,
    PromptTemplate,
    SessionInstructions,
    RunInstructions,
}

impl ResourceInstructionKind {
    pub const fn priority(self) -> u8 {
        match self {
            Self::AgentProfile => 2,
            Self::UserGlobalInstructions => 4,
            Self::WorkspaceInstructions => 5,
            Self::RootAgentsFile => 6,
            Self::PathAgentsFile => 7,
            Self::ActiveSkill => 8,
            Self::PromptTemplate => 9,
            Self::SessionInstructions => 13,
            Self::RunInstructions => 14,
        }
    }
}

/// resource-loader 到 context-engine 的中性 DTO，避免反向依赖。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceInstruction {
    pub kind: ResourceInstructionKind,
    pub resource_id: String,
    pub content: String,
    pub provenance: ResourceProvenance,
}

impl ResourceInstruction {
    /// `content` 的字节长度，供主循环计入 context 预算。
    pub fn byte_len(&self) -> usize {
        self.content.len()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResourceBundle {
    pub workspace_id: WorkspaceId,
    pub root_index: usize,
    pub agents: AgentsHierarchy,
    pub skills: SkillResolution,
    /// v1 兼容视图（name/instructions/default provider-model）。
    pub profiles: Vec<AgentProfile>,
    /// Profile v2：全维度档案（加载校验后）。
    pub profiles_v2: Vec<LoadedAgentProfileV2>,
    pub resolved_instructions: ResolvedInstructions,
    pub instructions: Vec<ResourceInstruction>,
    pub diagnostics: ResourceDiagnostics,
}

#[derive(Clone)]
pub struct ResourceLoader {
    workspaces: WorkspaceService,
    options: ResourceLoaderOptions,
}

impl ResourceLoader {
    pub fn new(workspaces: WorkspaceService, options: ResourceLoaderOptions) -> Self {
        Self {
            workspaces,
            options,
        }
    }

    fn validate_workspace_resource_dir(&self) -> Result<(), ResourceLoadError> {
        let path = Path::new(&self.options.workspace_resource_dir);
        if path.as_os_str().is_empty()
            || path.is_absolute()
            || path.components().any(|component| {
                matches!(
                    component,
                    Component::ParentDir | Component::RootDir | Component::Prefix(_)
                )
            })
        {
            return Err(ResourceLoadError::InvalidRelativePath(path.to_path_buf()));
        }
        Ok(())
    }

    /// 加载当前快照。
    pub fn load(&self, request: &ResourceRequest) -> Result<ResourceBundle, ResourceLoadError> {
        self.validate_workspace_resource_dir()?;
        request.current_path.validate()?;
        let workspace = self.workspaces.get(&request.workspace_id)?.ok_or_else(|| {
            ResourceLoadError::WorkspaceNotFound(request.workspace_id.to_string())
        })?;
        let selected_root = workspace.roots.get(request.root_index).ok_or(
            ResourceLoadError::RootIndexOutOfRange {
                root_index: request.root_index,
                root_count: workspace.roots.len(),
            },
        )?;
        let roots = workspace.roots.clone();
        let limits = self.options.limits;
        let global = self.options.global_resource_dir.as_deref();

        let (agents, agent_issues) = load_agents_hierarchy(
            selected_root,
            request.root_index,
            &request.current_path,
            request.current_path_kind,
            limits,
        );
        let skills = super::skills::load_skills(
            global,
            &roots,
            &self.options.workspace_resource_dir,
            &request.selection,
            &limits,
        );
        let mut profiles = load_profiles(
            global,
            &roots,
            &self.options.workspace_resource_dir,
            &request.selection,
            limits,
            self.options.memory_available,
        );
        resolve_profile_references(&mut profiles, &skills);

        let mut diagnostics = ResourceDiagnostics::default();
        let mut instructions = load_plain_instructions(
            global,
            &roots,
            &self.options.workspace_resource_dir,
            limits,
            &mut diagnostics,
        );
        diagnostics.issues.extend(agent_issues);
        append_agent_documents(&agents, &mut instructions, &mut diagnostics);
        append_skills(&skills, &mut instructions);
        append_profile_instructions(&profiles, &mut instructions);

        merge_diagnostics(&mut diagnostics, &skills.diagnostics);
        merge_diagnostics(&mut diagnostics, &profiles.diagnostics);
        instructions.sort_by(|left, right| {
            left.kind
                .priority()
                .cmp(&right.kind.priority())
                .then_with(|| left.provenance.tier.cmp(&right.provenance.tier))
                .then_with(|| left.provenance.source_key.cmp(&right.provenance.source_key))
                .then_with(|| left.resource_id.cmp(&right.resource_id))
        });
        diagnostics.sort_deterministically();

        Ok(ResourceBundle {
            workspace_id: request.workspace_id.clone(),
            root_index: request.root_index,
            agents,
            skills,
            profiles: profiles.profiles,
            profiles_v2: profiles.profiles_v2,
            resolved_instructions: profiles.instructions,
            instructions,
            diagnostics,
        })
    }
}

fn load_plain_instructions(
    global_resource_dir: Option<&Path>,
    workspace_roots: &[PathBuf],
    workspace_resource_dir: &str,
    limits: ResourceLimits,
    diagnostics: &mut ResourceDiagnostics,
) -> Vec<ResourceInstruction> {
    let mut instructions = Vec::new();
    if let Some(global) = global_resource_dir {
        let provenance = ResourceProvenance::new(
            ConfigTier::Global,
            "global:instructions",
            ResourceOrigin::Global {
                relative_path: "instructions.md".into(),
            },
        );
        load_instruction_file(
            &global.join("instructions.md"),
            global,
            "global",
            ResourceInstructionKind::UserGlobalInstructions,
            provenance,
            limits,
            &mut instructions,
            diagnostics,
        );
    }
    for (root_index, root) in workspace_roots.iter().enumerate() {
        let relative = format!("{workspace_resource_dir}/instructions.md");
        let provenance = ResourceProvenance::new(
            ConfigTier::Workspace,
            format!("workspace:{root_index:08}:instructions"),
            ResourceOrigin::Workspace {
                root_index,
                relative_path: relative.clone(),
            },
        );
        let path = join_under_root(root, &relative).unwrap_or_else(|_| root.join(&relative));
        load_instruction_file(
            &path,
            root,
            &format!("workspace:{root_index}"),
            ResourceInstructionKind::WorkspaceInstructions,
            provenance,
            limits,
            &mut instructions,
            diagnostics,
        );
    }
    instructions
}

#[allow(clippy::too_many_arguments)]
fn load_instruction_file(
    path: &Path,
    boundary_root: &Path,
    resource_id: &str,
    kind: ResourceInstructionKind,
    provenance: ResourceProvenance,
    limits: ResourceLimits,
    instructions: &mut Vec<ResourceInstruction>,
    diagnostics: &mut ResourceDiagnostics,
) {
    match read_utf8_bounded_within(path, boundary_root, limits.max_file_bytes) {
        Ok(content) if !content.trim().is_empty() => {
            diagnostics.entries.push(ResourceDiagnosticEntry {
                kind: ResourceKind::Instructions,
                resource_id: resource_id.into(),
                status: ResourceDiagnosticStatus::Active,
                provenance: provenance.clone(),
            });
            instructions.push(ResourceInstruction {
                kind,
                resource_id: resource_id.into(),
                content,
                provenance,
            });
        }
        Ok(_) => diagnostics.issues.push(
            ResourceIssue::warning(
                "instructions_empty",
                format!("instructions resource '{resource_id}' is empty"),
            )
            .for_resource(
                ResourceKind::Instructions,
                resource_id,
                provenance.source_key,
            ),
        ),
        Err(super::error::ResourceFileError::NotFound) => {}
        Err(error) => diagnostics.issues.push(
            ResourceIssue::error(
                error.code(),
                format!("instructions resource '{resource_id}' could not be loaded: {error}"),
            )
            .for_resource(
                ResourceKind::Instructions,
                resource_id,
                provenance.source_key,
            ),
        ),
    }
}

fn append_agent_documents(
    agents: &AgentsHierarchy,
    instructions: &mut Vec<ResourceInstruction>,
    diagnostics: &mut ResourceDiagnostics,
) {
    for document in agents.documents() {
        let relative = document.relative_path();
        let root = relative == "AGENTS.md";
        diagnostics.entries.push(ResourceDiagnosticEntry {
            kind: ResourceKind::AgentsFile,
            resource_id: relative.into(),
            status: ResourceDiagnosticStatus::Active,
            provenance: document.provenance.clone(),
        });
        instructions.push(ResourceInstruction {
            kind: if root {
                ResourceInstructionKind::RootAgentsFile
            } else {
                ResourceInstructionKind::PathAgentsFile
            },
            resource_id: relative.into(),
            content: document.body.clone(),
            provenance: document.provenance.clone(),
        });
    }
}

fn append_skills(skills: &SkillResolution, instructions: &mut Vec<ResourceInstruction>) {
    instructions.extend(skills.skills.iter().map(|skill| ResourceInstruction {
        kind: ResourceInstructionKind::ActiveSkill,
        resource_id: skill.manifest.id.clone(),
        content: skill.skill_markdown.clone(),
        provenance: skill.provenance.clone(),
    }));
}

fn append_profile_instructions(
    profiles: &ProfileResolution,
    instructions: &mut Vec<ResourceInstruction>,
) {
    for layer in profiles.instructions.ordered_layers() {
        let (kind, resource_id) = match layer.tier {
            ConfigTier::Profile => (ResourceInstructionKind::AgentProfile, "profile"),
            ConfigTier::Session => (ResourceInstructionKind::SessionInstructions, "session"),
            ConfigTier::Run => (ResourceInstructionKind::RunInstructions, "run"),
            ConfigTier::Builtin | ConfigTier::Global | ConfigTier::Workspace => continue,
        };
        instructions.push(ResourceInstruction {
            kind,
            resource_id: resource_id.into(),
            content: layer.instructions,
            provenance: layer.provenance,
        });
    }
}

fn merge_diagnostics(target: &mut ResourceDiagnostics, source: &ResourceDiagnostics) {
    target.entries.extend(source.entries.iter().cloned());
    target.issues.extend(source.issues.iter().cloned());
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeSet, fs};

    use pawork_domain::WorkspaceId;

    use super::*;
    use super::super::{ResourceSelection, WorkspaceRelativePath};

    fn loader_for(root: &Path) -> ResourceLoader {
        let workspaces = WorkspaceService::new();
        workspaces
            .add(WorkspaceId::from("w"), "test", [root])
            .expect("workspace");
        ResourceLoader::new(workspaces, ResourceLoaderOptions::default())
    }

    #[test]
    fn broken_resource_is_isolated_and_other_resources_still_load() {
        let temp = tempfile::tempdir().expect("tempdir");
        fs::write(temp.path().join("AGENTS.md"), "agents").expect("agents");
        let skill_dir = temp.path().join(".pawork/skills/bad");
        fs::create_dir_all(&skill_dir).expect("mkdir");
        fs::write(skill_dir.join("manifest.toml"), "id='bad'\n[").expect("bad skill");
        fs::write(skill_dir.join("SKILL.md"), "body").expect("skill body");
        let request = ResourceRequest::new(
            WorkspaceId::from("w"),
            0,
            WorkspaceRelativePath::new("src/lib.rs").expect("path"),
        );
        let bundle = loader_for(temp.path()).load(&request).expect("bundle");
        assert_eq!(bundle.agents.len(), 1);
        assert!(bundle
            .diagnostics
            .issues
            .iter()
            .any(|issue| issue.code == "skill_manifest_parse"));
    }

    #[test]
    fn parse_diagnostics_do_not_echo_resource_body_content() {
        let temp = tempfile::tempdir().expect("tempdir");
        let profile_dir = temp.path().join(".pawork/profiles");
        let skill_dir = temp.path().join(".pawork/skills/bad");
        fs::create_dir_all(&profile_dir).expect("profile dir");
        fs::create_dir_all(&skill_dir).expect("skill dir");
        fs::write(
            profile_dir.join("bad.toml"),
            "instructions='private profile prose'\n[",
        )
        .expect("profile");
        fs::write(
            skill_dir.join("manifest.toml"),
            "id='bad'\nversion='1.0.0'\ndescription='private skill prose'\n[",
        )
        .expect("manifest");
        fs::write(skill_dir.join("SKILL.md"), "body").expect("skill body");
        let request = ResourceRequest::new(
            WorkspaceId::from("w"),
            0,
            WorkspaceRelativePath::new("src/lib.rs").expect("path"),
        );
        let bundle = loader_for(temp.path()).load(&request).expect("bundle");
        let serialized = serde_json::to_string(&bundle.diagnostics).expect("diagnostics");
        for forbidden in ["private profile prose", "private skill prose"] {
            assert!(!serialized.contains(forbidden));
        }
    }

    #[test]
    fn effective_instruction_order_matches_documented_layers() {
        let temp = tempfile::tempdir().expect("tempdir");
        fs::create_dir_all(temp.path().join("0/deep")).expect("target dirs");
        fs::write(temp.path().join("AGENTS.md"), "root agents").expect("root agents");
        fs::write(temp.path().join("0/AGENTS.md"), "path agents 1").expect("path agents 1");
        fs::write(temp.path().join("0/deep/AGENTS.md"), "path agents 2").expect("path agents 2");
        fs::create_dir_all(temp.path().join(".pawork")).expect("pawork");
        fs::write(
            temp.path().join(".pawork/instructions.md"),
            "workspace instructions",
        )
        .expect("instructions");
        let skill = temp.path().join(".pawork/skills/review");
        fs::create_dir_all(&skill).expect("skill dir");
        fs::write(
            skill.join("manifest.toml"),
            "id='review'\nversion='1.0.0'\ndescription='review'",
        )
        .expect("manifest");
        fs::write(skill.join("SKILL.md"), "skill instructions").expect("skill body");
        let mut request = ResourceRequest::new(
            WorkspaceId::from("w"),
            0,
            WorkspaceRelativePath::new("0/deep/lib.rs").expect("path"),
        );
        request.selection = ResourceSelection {
            active_skills: BTreeSet::from(["review".into()]),
            run_instructions: Some("run".into()),
            ..ResourceSelection::default()
        };
        let bundle = loader_for(temp.path()).load(&request).expect("bundle");
        assert_eq!(
            bundle
                .instructions
                .iter()
                .map(|instruction| instruction.kind)
                .collect::<Vec<_>>(),
            vec![
                ResourceInstructionKind::WorkspaceInstructions,
                ResourceInstructionKind::RootAgentsFile,
                ResourceInstructionKind::PathAgentsFile,
                ResourceInstructionKind::PathAgentsFile,
                ResourceInstructionKind::ActiveSkill,
                ResourceInstructionKind::RunInstructions,
            ]
        );
        assert_eq!(
            bundle
                .instructions
                .iter()
                .filter(|instruction| {
                    instruction.kind == ResourceInstructionKind::PathAgentsFile
                })
                .map(|instruction| instruction.content.as_str())
                .collect::<Vec<_>>(),
            vec!["path agents 1", "path agents 2"]
        );
    }

    #[test]
    fn instruction_byte_len_sums_content_lengths() {
        let temp = tempfile::tempdir().expect("tempdir");
        fs::create_dir_all(temp.path().join("0/deep")).expect("target dirs");
        fs::write(temp.path().join("AGENTS.md"), "root agents").expect("root agents");
        fs::write(temp.path().join("0/AGENTS.md"), "path agents 1").expect("path agents 1");
        fs::write(temp.path().join("0/deep/AGENTS.md"), "path agents 2").expect("path agents 2");
        fs::create_dir_all(temp.path().join(".pawork")).expect("pawork");
        fs::write(
            temp.path().join(".pawork/instructions.md"),
            "workspace instructions",
        )
        .expect("instructions");
        let skill = temp.path().join(".pawork/skills/review");
        fs::create_dir_all(&skill).expect("skill dir");
        fs::write(
            skill.join("manifest.toml"),
            "id='review'\nversion='1.0.0'\ndescription='review'",
        )
        .expect("manifest");
        fs::write(skill.join("SKILL.md"), "skill instructions").expect("skill body");
        let mut request = ResourceRequest::new(
            WorkspaceId::from("w"),
            0,
            WorkspaceRelativePath::new("0/deep/lib.rs").expect("path"),
        );
        request.selection = ResourceSelection {
            active_skills: BTreeSet::from(["review".into()]),
            run_instructions: Some("run".into()),
            ..ResourceSelection::default()
        };
        let bundle = loader_for(temp.path()).load(&request).expect("bundle");
        let summed: usize = bundle
            .instructions
            .iter()
            .map(ResourceInstruction::byte_len)
            .sum();
        let expected: usize = bundle
            .instructions
            .iter()
            .map(|instruction| instruction.content.len())
            .sum();
        assert_eq!(summed, expected);
        assert!(summed > 0);
    }

    #[test]
    fn invalid_root_index_is_fatal_but_does_not_read_arbitrary_paths() {
        let temp = tempfile::tempdir().expect("tempdir");
        let request = ResourceRequest::new(
            WorkspaceId::from("w"),
            5,
            WorkspaceRelativePath::new("src/lib.rs").expect("path"),
        );
        assert!(matches!(
            loader_for(temp.path()).load(&request),
            Err(ResourceLoadError::RootIndexOutOfRange { .. })
        ));
    }

    #[test]
    fn empty_workspace_resource_directory_is_rejected() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut loader = loader_for(temp.path());
        loader.options.workspace_resource_dir.clear();
        let request = ResourceRequest::new(
            WorkspaceId::from("w"),
            0,
            WorkspaceRelativePath::new("src/lib.rs").expect("path"),
        );
        assert!(matches!(
            loader.load(&request),
            Err(ResourceLoadError::InvalidRelativePath(_))
        ));
    }

    #[cfg(unix)]
    #[test]
    fn workspace_resource_symlinks_cannot_escape_root() {
        use std::os::unix::fs::symlink;

        let workspace = tempfile::tempdir().expect("workspace");
        let outside = tempfile::tempdir().expect("outside");
        let pawork = workspace.path().join(".pawork");
        fs::create_dir_all(pawork.join("profiles")).expect("profiles");
        fs::create_dir_all(pawork.join("skills")).expect("skills");

        fs::write(
            outside.path().join("instructions.md"),
            "outside instructions secret",
        )
        .expect("outside instructions");
        fs::write(
            outside.path().join("profile.toml"),
            "name='outside'\ninstructions='outside profile secret'",
        )
        .expect("outside profile");
        let outside_skill = outside.path().join("skill");
        fs::create_dir_all(&outside_skill).expect("outside skill");
        fs::write(
            outside_skill.join("manifest.toml"),
            "id='outside'\nversion='1.0.0'",
        )
        .expect("outside manifest");
        fs::write(outside_skill.join("SKILL.md"), "outside skill secret")
            .expect("outside skill body");

        symlink(
            outside.path().join("instructions.md"),
            pawork.join("instructions.md"),
        )
        .expect("instructions symlink");
        symlink(
            outside.path().join("profile.toml"),
            pawork.join("profiles/outside.toml"),
        )
        .expect("profile symlink");
        symlink(&outside_skill, pawork.join("skills/outside")).expect("skill symlink");

        let request = ResourceRequest::new(
            WorkspaceId::from("w"),
            0,
            WorkspaceRelativePath::new("src/lib.rs").expect("path"),
        );
        let bundle = loader_for(workspace.path()).load(&request).expect("bundle");
        let serialized = serde_json::to_string(&bundle.diagnostics).expect("diagnostics");
        assert!(bundle.instructions.is_empty());
        assert!(bundle.skills.skills.is_empty());
        assert!(bundle.profiles.is_empty());
        assert!(
            bundle
                .diagnostics
                .issues
                .iter()
                .filter(|issue| issue.code == "resource_outside_root")
                .count()
                >= 3
        );
        for forbidden in [
            "outside instructions secret",
            "outside profile secret",
            "outside skill secret",
        ] {
            assert!(!serialized.contains(forbidden));
        }
    }
}
