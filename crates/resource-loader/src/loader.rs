//! Resource Loader 聚合入口。

use std::{
    path::{Component, Path, PathBuf},
    time::Duration,
};

use agent_domain::WorkspaceId;
use config_service::ConfigTier;
use serde::{Deserialize, Serialize};
use workspace_service::WorkspaceService;

use crate::{
    agents::load_agents_hierarchy,
    io::read_utf8_bounded_within,
    profiles::{load_profiles, ProfileResolution},
    skills::load_skills,
    templates::load_templates,
    AgentProfile, AgentsHierarchy, ResolvedInstructions, ResourceDiagnosticEntry,
    ResourceDiagnosticStatus, ResourceDiagnosticView, ResourceDiagnostics, ResourceHotReload,
    ResourceIssue, ResourceKind, ResourceLimits, ResourceLoadError, ResourceLoaderOptions,
    ResourceOrigin, ResourceProvenance, ResourceRequest, ResourceWatcher, SkillResolution,
    TemplateResolution,
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
    const fn priority(self) -> u8 {
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResourceBundle {
    pub workspace_id: WorkspaceId,
    pub root_index: usize,
    pub agents: AgentsHierarchy,
    pub skills: SkillResolution,
    pub templates: TemplateResolution,
    pub profiles: Vec<AgentProfile>,
    pub resolved_instructions: ResolvedInstructions,
    pub instructions: Vec<ResourceInstruction>,
    pub diagnostics: ResourceDiagnostics,
}

impl ResourceBundle {
    pub fn diagnostic_view(&self) -> ResourceDiagnosticView {
        ResourceDiagnosticView::build(&self.diagnostics, &diagnostics::Redactor::default())
    }
}

pub trait LoadResources: Send + Sync {
    fn load(&self, request: &ResourceRequest) -> Result<ResourceBundle, ResourceLoadError>;
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

    pub fn options(&self) -> &ResourceLoaderOptions {
        &self.options
    }

    /// 加载当前快照，并监听全局资源目录与全部 workspace roots。`AGENTS.md` 可位于
    /// 任意路径层级，因此 workspace root 使用递归监听。
    pub fn watch(
        &self,
        request: ResourceRequest,
        debounce: Duration,
    ) -> Result<(ResourceHotReload<ResourceBundle>, ResourceWatcher), ResourceLoadError> {
        let workspace = self.workspaces.get(&request.workspace_id)?.ok_or_else(|| {
            ResourceLoadError::WorkspaceNotFound(request.workspace_id.to_string())
        })?;
        let mut paths = workspace
            .roots
            .iter()
            .map(|root| root.path.clone())
            .collect::<Vec<_>>();
        if let Some(global) = &self.options.global_resource_dir {
            if let Some(watch_root) = nearest_existing_watch_root(global) {
                paths.push(watch_root);
            }
        }
        let loader = self.clone();
        ResourceHotReload::start(paths, debounce, move || {
            loader.load(&request).map_err(|error| error.to_string())
        })
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
}

fn nearest_existing_watch_root(path: &Path) -> Option<PathBuf> {
    path.ancestors()
        .find(|ancestor| ancestor.is_dir() && ancestor.parent().is_some())
        .map(Path::to_path_buf)
}

impl LoadResources for ResourceLoader {
    fn load(&self, request: &ResourceRequest) -> Result<ResourceBundle, ResourceLoadError> {
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
        let roots = workspace
            .roots
            .iter()
            .map(|root| root.path.clone())
            .collect::<Vec<_>>();
        let limits = self.options.limits;
        let global = self.options.global_resource_dir.as_deref();

        let (agents, agent_issues) = load_agents_hierarchy(
            &selected_root.path,
            request.root_index,
            &request.current_path,
            request.current_path_kind,
            limits,
        );
        let skills = load_skills(
            global,
            &roots,
            &self.options.workspace_resource_dir,
            &request.selection,
            &limits,
        );
        let templates = load_templates(
            global,
            &roots,
            &self.options.workspace_resource_dir,
            request.root_index,
            &request.selection,
            limits,
        );
        let profiles = load_profiles(
            global,
            &roots,
            &self.options.workspace_resource_dir,
            &request.selection,
            limits,
        );

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
        append_template(&templates, &mut instructions);
        append_profile_instructions(&profiles, &mut instructions);

        merge_diagnostics(&mut diagnostics, &skills.diagnostics);
        merge_diagnostics(&mut diagnostics, &templates.diagnostics);
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
            templates,
            profiles: profiles.profiles,
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
        load_instruction_file(
            &root.join(&relative),
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
        Err(crate::error::ResourceFileError::NotFound) => {}
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

fn append_template(templates: &TemplateResolution, instructions: &mut Vec<ResourceInstruction>) {
    if let Some(prompt) = &templates.selected {
        instructions.push(ResourceInstruction {
            kind: ResourceInstructionKind::PromptTemplate,
            resource_id: prompt.template_id.clone(),
            content: prompt.content.clone(),
            provenance: prompt.provenance.clone(),
        });
    }
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

    use agent_domain::{Timestamp, WorkspaceId};

    use super::*;
    use crate::{ResourceSelection, WorkspaceRelativePath};

    fn loader_for(root: &Path) -> ResourceLoader {
        let workspaces = WorkspaceService::new();
        workspaces
            .add(
                WorkspaceId::from("w"),
                "test",
                [root],
                Timestamp::from_unix_millis(1),
            )
            .expect("workspace");
        ResourceLoader::new(workspaces, ResourceLoaderOptions::default())
    }

    #[test]
    fn broken_resource_is_isolated_and_other_resources_still_load() {
        let temp = tempfile::tempdir().expect("tempdir");
        fs::write(temp.path().join("AGENTS.md"), "agents").expect("agents");
        let prompt_dir = temp.path().join(".pawork/prompts");
        fs::create_dir_all(&prompt_dir).expect("mkdir");
        fs::write(prompt_dir.join("bad.md"), "not frontmatter").expect("bad prompt");
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
            .any(|issue| issue.code == "prompt_frontmatter_invalid"));
    }

    #[test]
    fn parse_diagnostics_do_not_echo_resource_body_content() {
        let temp = tempfile::tempdir().expect("tempdir");
        let profile_dir = temp.path().join(".pawork/profiles");
        let prompt_dir = temp.path().join(".pawork/prompts");
        let skill_dir = temp.path().join(".pawork/skills/bad");
        fs::create_dir_all(&profile_dir).expect("profile dir");
        fs::create_dir_all(&prompt_dir).expect("prompt dir");
        fs::create_dir_all(&skill_dir).expect("skill dir");
        fs::write(
            profile_dir.join("bad.toml"),
            "instructions='private profile prose'\n[",
        )
        .expect("profile");
        fs::write(
            prompt_dir.join("bad.md"),
            "+++\nid='bad'\nprivate='private prompt prose'\n[\n+++\nbody",
        )
        .expect("prompt");
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
        for forbidden in [
            "private profile prose",
            "private prompt prose",
            "private skill prose",
        ] {
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
        fs::create_dir_all(pawork.join("prompts")).expect("prompts");
        fs::create_dir_all(pawork.join("profiles")).expect("profiles");
        fs::create_dir_all(pawork.join("skills")).expect("skills");

        fs::write(
            outside.path().join("instructions.md"),
            "outside instructions secret",
        )
        .expect("outside instructions");
        fs::write(
            outside.path().join("prompt.md"),
            "+++\nid='outside'\n+++\noutside prompt secret",
        )
        .expect("outside prompt");
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
            outside.path().join("prompt.md"),
            pawork.join("prompts/outside.md"),
        )
        .expect("prompt symlink");
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
        assert!(bundle.templates.templates.is_empty());
        assert!(bundle.profiles.is_empty());
        assert!(
            bundle
                .diagnostics
                .issues
                .iter()
                .filter(|issue| issue.code == "resource_outside_root")
                .count()
                >= 4
        );
        for forbidden in [
            "outside instructions secret",
            "outside prompt secret",
            "outside profile secret",
            "outside skill secret",
        ] {
            assert!(!serialized.contains(forbidden));
        }
    }

    #[test]
    fn watcher_reloads_bundle_after_resource_change() {
        let temp = tempfile::tempdir().expect("tempdir");
        fs::write(temp.path().join("AGENTS.md"), "one").expect("agents");
        let request = ResourceRequest::new(
            WorkspaceId::from("w"),
            0,
            WorkspaceRelativePath::new("src/lib.rs").expect("path"),
        );
        let loader = loader_for(temp.path());
        let (store, _watcher) = loader
            .watch(request, Duration::from_millis(50))
            .expect("watch");
        assert_eq!(
            store
                .snapshot()
                .value
                .agents
                .nearest()
                .expect("agents")
                .body,
            "one"
        );
        fs::write(temp.path().join("AGENTS.md"), "two").expect("agents changed");
        let started = std::time::Instant::now();
        while store
            .snapshot()
            .value
            .agents
            .nearest()
            .map(|document| document.body.as_str())
            != Some("two")
            && started.elapsed() < Duration::from_secs(5)
        {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(
            store
                .snapshot()
                .value
                .agents
                .nearest()
                .expect("agents")
                .body,
            "two"
        );
    }

    #[test]
    fn watcher_observes_creation_of_missing_global_resource_directory() {
        let workspace = tempfile::tempdir().expect("workspace");
        let config_parent = tempfile::tempdir().expect("config parent");
        let global = config_parent.path().join("missing/resources");
        let mut loader = loader_for(workspace.path());
        loader.options.global_resource_dir = Some(global.clone());
        let request = ResourceRequest::new(
            WorkspaceId::from("w"),
            0,
            WorkspaceRelativePath::new("src/lib.rs").expect("path"),
        );
        let (store, _watcher) = loader
            .watch(request, Duration::from_millis(50))
            .expect("watch");
        assert!(store.snapshot().value.instructions.is_empty());

        fs::create_dir_all(&global).expect("global dir");
        fs::write(global.join("instructions.md"), "global created").expect("global instructions");
        let started = std::time::Instant::now();
        while !store
            .snapshot()
            .value
            .instructions
            .iter()
            .any(|instruction| instruction.content == "global created")
            && started.elapsed() < Duration::from_secs(5)
        {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(store
            .snapshot()
            .value
            .instructions
            .iter()
            .any(|instruction| instruction.content == "global created"));
    }
}
