//! Agent Profile v1 与运行期 instructions。

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use config_service::ConfigTier;
use serde::{Deserialize, Serialize};

use crate::{
    io::{path_key, read_utf8_bounded_within, sorted_children_within, workspace_relative_key},
    ResourceDiagnosticEntry, ResourceDiagnosticStatus, ResourceDiagnostics, ResourceIssue,
    ResourceKind, ResourceLimits, ResourceOrigin, ResourceProvenance, ResourceSelection,
};

/// Phase 8 的 Agent Profile v1。完整工具、Skills、MCP、权限等维度属于 P17-5 v2。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentProfile {
    pub name: String,
    pub instructions: String,
    pub default_provider: Option<String>,
    pub default_model: Option<String>,
    pub provenance: ResourceProvenance,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstructionLayer {
    pub tier: ConfigTier,
    pub source_key: String,
    pub instructions: String,
    pub provenance: ResourceProvenance,
}

/// 已按 profile < session < run 固定顺序解析的指令层。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedInstructions {
    pub profile: Option<AgentProfile>,
    pub session: Option<InstructionLayer>,
    pub run: Option<InstructionLayer>,
    pub issues: Vec<ResourceIssue>,
}

impl ResolvedInstructions {
    pub fn ordered_layers(&self) -> Vec<InstructionLayer> {
        let mut layers = Vec::with_capacity(3);
        if let Some(profile) = &self.profile {
            layers.push(InstructionLayer {
                tier: ConfigTier::Profile,
                source_key: profile.provenance.source_key.clone(),
                instructions: profile.instructions.clone(),
                provenance: profile.provenance.clone(),
            });
        }
        if let Some(session) = &self.session {
            layers.push(session.clone());
        }
        if let Some(run) = &self.run {
            layers.push(run.clone());
        }
        layers
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct ProfileResolution {
    pub profiles: Vec<AgentProfile>,
    pub instructions: ResolvedInstructions,
    pub diagnostics: ResourceDiagnostics,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProfileFile {
    name: Option<String>,
    instructions: String,
    #[serde(default)]
    default_provider: Option<String>,
    #[serde(default)]
    default_model: Option<String>,
}

pub(crate) fn load_profiles(
    global_resource_dir: Option<&Path>,
    workspace_roots: &[PathBuf],
    workspace_resource_dir: &str,
    selection: &ResourceSelection,
    limits: ResourceLimits,
) -> ProfileResolution {
    let mut candidates = Vec::new();
    let mut diagnostics = ResourceDiagnostics::default();
    if let Some(global_root) = global_resource_dir {
        load_directory(
            &global_root.join("profiles"),
            global_root,
            ConfigTier::Global,
            None,
            limits,
            &mut candidates,
            &mut diagnostics,
        );
    }
    for (root_index, root) in workspace_roots.iter().enumerate() {
        load_directory(
            &root.join(workspace_resource_dir).join("profiles"),
            root,
            ConfigTier::Workspace,
            Some(root_index),
            limits,
            &mut candidates,
            &mut diagnostics,
        );
    }

    candidates.sort_by(|left: &AgentProfile, right| {
        left.provenance
            .tier
            .cmp(&right.provenance.tier)
            .then_with(|| left.provenance.source_key.cmp(&right.provenance.source_key))
    });
    let mut effective = BTreeMap::new();
    for candidate in candidates {
        if let Some(overridden) = effective.insert(candidate.name.clone(), candidate) {
            diagnostics.entries.push(ResourceDiagnosticEntry {
                kind: ResourceKind::AgentProfile,
                resource_id: overridden.name,
                status: ResourceDiagnosticStatus::Overridden,
                provenance: overridden.provenance,
            });
        }
    }

    let selected_profile = selection
        .profile
        .as_ref()
        .and_then(|name| effective.get(name).cloned());
    let mut instruction_issues = Vec::new();
    if let Some(name) = &selection.profile {
        if selected_profile.is_none() {
            instruction_issues.push(
                ResourceIssue::error(
                    "agent_profile_not_found",
                    format!("selected agent profile '{name}' was not found"),
                )
                .for_resource(
                    ResourceKind::AgentProfile,
                    name,
                    "selection:profile",
                ),
            );
        }
    }
    let instructions = ResolvedInstructions {
        profile: selected_profile.clone(),
        session: selection
            .session_instructions
            .as_ref()
            .filter(|value| !value.is_empty())
            .map(|value| InstructionLayer {
                tier: ConfigTier::Session,
                source_key: "session:instructions".into(),
                instructions: value.clone(),
                provenance: ResourceProvenance::new(
                    ConfigTier::Session,
                    "session:instructions",
                    ResourceOrigin::Session {
                        name: "instructions".into(),
                    },
                ),
            }),
        run: selection
            .run_instructions
            .as_ref()
            .filter(|value| !value.is_empty())
            .map(|value| InstructionLayer {
                tier: ConfigTier::Run,
                source_key: "run:instructions".into(),
                instructions: value.clone(),
                provenance: ResourceProvenance::new(
                    ConfigTier::Run,
                    "run:instructions",
                    ResourceOrigin::Run {
                        name: "instructions".into(),
                    },
                ),
            }),
        issues: instruction_issues.clone(),
    };
    diagnostics.issues.extend(instruction_issues);

    let selected_name = selected_profile
        .as_ref()
        .map(|profile| profile.name.as_str());
    let profiles = effective
        .into_values()
        .inspect(|profile| {
            diagnostics.entries.push(ResourceDiagnosticEntry {
                kind: ResourceKind::AgentProfile,
                resource_id: profile.name.clone(),
                status: if selected_name == Some(profile.name.as_str()) {
                    ResourceDiagnosticStatus::Active
                } else {
                    ResourceDiagnosticStatus::Loaded
                },
                provenance: profile.provenance.clone(),
            });
        })
        .collect();
    if let Some(session) = &instructions.session {
        diagnostics.entries.push(ResourceDiagnosticEntry {
            kind: ResourceKind::Instructions,
            resource_id: "session".into(),
            status: ResourceDiagnosticStatus::Active,
            provenance: session.provenance.clone(),
        });
    }
    if let Some(run) = &instructions.run {
        diagnostics.entries.push(ResourceDiagnosticEntry {
            kind: ResourceKind::Instructions,
            resource_id: "run".into(),
            status: ResourceDiagnosticStatus::Active,
            provenance: run.provenance.clone(),
        });
    }
    diagnostics.sort_deterministically();
    ProfileResolution {
        profiles,
        instructions,
        diagnostics,
    }
}

fn load_directory(
    directory: &Path,
    source_root: &Path,
    tier: ConfigTier,
    root_index: Option<usize>,
    limits: ResourceLimits,
    candidates: &mut Vec<AgentProfile>,
    diagnostics: &mut ResourceDiagnostics,
) {
    let paths = match sorted_children_within(directory, source_root, limits.max_resources_per_kind)
    {
        Ok(paths) => paths,
        Err(error) => {
            diagnostics.issues.push(ResourceIssue::error(
                error.code(),
                format!("profile directory could not be read: {error}"),
            ));
            return;
        }
    };
    for path in paths {
        if path.extension().and_then(|value| value.to_str()) != Some("toml") {
            continue;
        }
        let relative = workspace_relative_key(&path, source_root);
        let source_key = root_index.map_or_else(
            || format!("global:profile:{relative}"),
            |index| format!("workspace:{index:08}:profile:{relative}"),
        );
        let provenance = ResourceProvenance::new(
            tier,
            source_key.clone(),
            root_index.map_or_else(
                || ResourceOrigin::Global {
                    relative_path: relative.clone(),
                },
                |index| ResourceOrigin::Workspace {
                    root_index: index,
                    relative_path: relative.clone(),
                },
            ),
        );
        match parse_profile(&path, source_root, provenance, limits.max_file_bytes) {
            Ok(profile) => candidates.push(profile),
            Err(issue) => diagnostics.issues.push(issue),
        }
    }
}

fn parse_profile(
    path: &Path,
    source_root: &Path,
    provenance: ResourceProvenance,
    max_file_bytes: u64,
) -> Result<AgentProfile, ResourceIssue> {
    let fallback_name = file_stem(path);
    let source_key = provenance.source_key.clone();
    let content = read_utf8_bounded_within(path, source_root, max_file_bytes).map_err(|error| {
        ResourceIssue::error(
            error.code(),
            format!("agent profile could not be loaded: {error}"),
        )
        .for_resource(
            ResourceKind::AgentProfile,
            &fallback_name,
            source_key.clone(),
        )
    })?;
    let file: ProfileFile = crate::io::parse_toml_resource(
        &content,
        "agent_profile_invalid",
        "agent profile has invalid TOML syntax or unsupported v1 fields",
    )
    .map_err(|issue| {
        issue.for_resource(
            ResourceKind::AgentProfile,
            &fallback_name,
            source_key.clone(),
        )
    })?;
    let name = file.name.unwrap_or(fallback_name);
    validate_name(&name).map_err(|message| {
        ResourceIssue::error("agent_profile_name_invalid", message).for_resource(
            ResourceKind::AgentProfile,
            &name,
            source_key,
        )
    })?;
    if file.instructions.trim().is_empty() {
        return Err(ResourceIssue::error(
            "agent_profile_instructions_empty",
            "agent profile instructions may not be empty",
        )
        .for_resource(
            ResourceKind::AgentProfile,
            &name,
            provenance.source_key.clone(),
        ));
    }
    Ok(AgentProfile {
        name,
        instructions: file.instructions,
        default_provider: file.default_provider,
        default_model: file.default_model,
        provenance,
    })
}

fn validate_name(name: &str) -> Result<(), String> {
    if !crate::io::is_valid_identifier(name, true) {
        return Err(format!(
            "agent profile name '{name}' must contain only ASCII letters, digits, '.', '-' or '_'"
        ));
    }
    Ok(())
}

fn file_stem(path: &Path) -> String {
    path.file_stem()
        .and_then(|value| value.to_str())
        .map(str::to_owned)
        .unwrap_or_else(|| path_key(path))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    fn write_profile(root: &Path, relative: &str, content: &str) {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        fs::write(path, content).expect("write");
    }

    #[test]
    fn workspace_profile_overrides_global_and_run_is_last() {
        let global = tempfile::tempdir().expect("global");
        let workspace = tempfile::tempdir().expect("workspace");
        write_profile(
            global.path(),
            "profiles/review.toml",
            "name='review'\ninstructions='global'",
        );
        write_profile(
            workspace.path(),
            ".pawork/profiles/review.toml",
            "name='review'\ninstructions='workspace'\ndefault_model='m'",
        );
        let selection = ResourceSelection {
            profile: Some("review".into()),
            session_instructions: Some("session".into()),
            run_instructions: Some("run".into()),
            ..ResourceSelection::default()
        };
        let resolution = load_profiles(
            Some(global.path()),
            &[workspace.path().to_path_buf()],
            ".pawork",
            &selection,
            ResourceLimits::default(),
        );
        let layers = resolution.instructions.ordered_layers();
        assert_eq!(
            layers
                .iter()
                .map(|layer| layer.instructions.as_str())
                .collect::<Vec<_>>(),
            vec!["workspace", "session", "run"]
        );
        assert_eq!(
            resolution
                .instructions
                .profile
                .as_ref()
                .and_then(|profile| profile.default_model.as_deref()),
            Some("m")
        );
    }

    #[test]
    fn missing_profile_does_not_drop_run_instructions() {
        let workspace = tempfile::tempdir().expect("workspace");
        let selection = ResourceSelection {
            profile: Some("missing".into()),
            run_instructions: Some("still active".into()),
            ..ResourceSelection::default()
        };
        let resolution = load_profiles(
            None,
            &[workspace.path().to_path_buf()],
            ".pawork",
            &selection,
            ResourceLimits::default(),
        );
        assert!(resolution.instructions.profile.is_none());
        assert_eq!(
            resolution
                .instructions
                .run
                .as_ref()
                .map(|run| run.instructions.as_str()),
            Some("still active")
        );
        assert!(resolution
            .instructions
            .issues
            .iter()
            .any(|issue| issue.code == "agent_profile_not_found"));
    }
}
