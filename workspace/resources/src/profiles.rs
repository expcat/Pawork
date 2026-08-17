//! Agent Profile v1/v2 加载、校验与运行期 instructions。
//!
//! v1（P8-5）：`name / instructions / default_provider / default_model`。
//! v2：`pawork_domain::AgentProfileV2` 全维度（prompt / model /
//! canonical effort / tools(denied) / skills / mcp / permissions / hooks /
//! memory / max-turns / background / isolation）。加载时自动迁移 v1→v2，
//! deny-first 校验工具规则、校验引用格式与跨类解析（本波只解析 skills，
//! fail-closed；hooks 引用跳过不解析；mcp / permissions 只做格式校验）、
//! 拒绝明文 secret。memory 默认 off；生产记忆不可用时显式标注 `Unavailable`，
//! 绝不虚假可用。

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use pawork_config::ConfigTier;
use pawork_domain::{
    AgentProfileV2, MemoryPrivacy, ProfileIsolation, ProfileMemory, ProfileModel, ProfilePrompt,
    ProfileRef, ProfileToolRules, ReasoningEffort,
};
use semver::{Version, VersionReq};
use serde::{Deserialize, Serialize};

use crate::{
    io::{
        join_under_root, path_key, read_utf8_bounded_within, sorted_children_within,
        workspace_relative_key,
    },
    skills::SkillResolution,
    ResourceDiagnosticEntry, ResourceDiagnosticStatus, ResourceDiagnostics, ResourceIssue,
    ResourceKind, ResourceLimits, ResourceOrigin, ResourceProvenance, ResourceSelection,
};

/// Phase 8 的 Agent Profile v1 兼容视图。v2 完整维度见 [`LoadedAgentProfileV2`]。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentProfile {
    pub name: String,
    pub instructions: String,
    pub default_provider: Option<String>,
    pub default_model: Option<String>,
    pub provenance: ResourceProvenance,
}

/// 已加载并校验的 Profile v2：领域类型 + 来源 provenance。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoadedAgentProfileV2 {
    pub profile: AgentProfileV2,
    pub provenance: ResourceProvenance,
}

impl LoadedAgentProfileV2 {
    fn compat_view(&self) -> AgentProfile {
        let mut instructions = self.profile.prompt.system.clone();
        if let Some(extra) = &self.profile.prompt.instructions {
            instructions.push_str("\n\n");
            instructions.push_str(extra);
        }
        AgentProfile {
            name: self.profile.name.clone(),
            instructions,
            default_provider: self.profile.model.provider.clone(),
            default_model: self.profile.model.name.clone(),
            provenance: self.provenance.clone(),
        }
    }
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
    pub profiles_v2: Vec<LoadedAgentProfileV2>,
    pub instructions: ResolvedInstructions,
    pub diagnostics: ResourceDiagnostics,
    effective: BTreeMap<String, LoadedAgentProfileV2>,
    overridden: Vec<ResourceDiagnosticEntry>,
    selection: ResourceSelection,
}

// ---------------------------------------------------------------------------
// 文件格式（raw，全部字段可选；未知字段一律拒绝，fail-closed）
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProfileFileRaw {
    /// `"v1"` / `"v2"`；缺省时只要出现任一 v2-only 字段就按 v2 解析。
    /// v1/v2 字段混用一律显式报错，绝不静默丢字段。
    schema: Option<String>,
    name: Option<String>,
    // v1 字段
    instructions: Option<String>,
    default_provider: Option<String>,
    default_model: Option<String>,
    // v2 字段
    prompt: Option<ProfilePromptRaw>,
    model: Option<ProfileModelRaw>,
    effort: Option<ReasoningEffort>,
    tools: Option<ProfileToolRulesRaw>,
    skills: Option<Vec<ProfileRef>>,
    mcp: Option<Vec<ProfileRef>>,
    permissions: Option<Vec<ProfileRef>>,
    hooks: Option<Vec<ProfileRef>>,
    memory: Option<ProfileMemoryRaw>,
    max_turns: Option<u64>,
    background: Option<bool>,
    isolation: Option<ProfileIsolation>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProfilePromptRaw {
    system: String,
    #[serde(default)]
    instructions: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProfileModelRaw {
    #[serde(default)]
    provider: Option<String>,
    #[serde(default)]
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProfileToolRulesRaw {
    #[serde(default)]
    allowed: Vec<String>,
    #[serde(default)]
    denied: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProfileMemoryRaw {
    #[serde(default)]
    enabled: bool,
    #[serde(default)]
    privacy: MemoryPrivacy,
    #[serde(default)]
    unavailable: Option<String>,
}

impl ProfileFileRaw {
    fn v1_only_fields(&self) -> Vec<&'static str> {
        let mut fields = Vec::new();
        if self.instructions.is_some() {
            fields.push("instructions");
        }
        if self.default_provider.is_some() {
            fields.push("default_provider");
        }
        if self.default_model.is_some() {
            fields.push("default_model");
        }
        fields
    }

    fn v2_only_fields(&self) -> Vec<&'static str> {
        let mut fields = Vec::new();
        if self.prompt.is_some() {
            fields.push("prompt");
        }
        if self.model.is_some() {
            fields.push("model");
        }
        if self.effort.is_some() {
            fields.push("effort");
        }
        if self.tools.is_some() {
            fields.push("tools");
        }
        if self.skills.is_some() {
            fields.push("skills");
        }
        if self.mcp.is_some() {
            fields.push("mcp");
        }
        if self.permissions.is_some() {
            fields.push("permissions");
        }
        if self.hooks.is_some() {
            fields.push("hooks");
        }
        if self.memory.is_some() {
            fields.push("memory");
        }
        if self.max_turns.is_some() {
            fields.push("max_turns");
        }
        if self.background.is_some() {
            fields.push("background");
        }
        if self.isolation.is_some() {
            fields.push("isolation");
        }
        fields
    }

    fn validate_no_plaintext_secrets(
        &self,
        resource_id: &str,
        source_key: &str,
    ) -> Result<(), ResourceIssue> {
        let check = |field: &str, value: &str| {
            if contains_plaintext_secret(value) {
                return Err(plaintext_secret_issue(resource_id, source_key, field));
            }
            Ok(())
        };

        for (field, value) in [
            ("schema", self.schema.as_deref()),
            ("name", self.name.as_deref()),
            ("instructions", self.instructions.as_deref()),
            ("default_provider", self.default_provider.as_deref()),
            ("default_model", self.default_model.as_deref()),
            (
                "prompt.system",
                self.prompt.as_ref().map(|prompt| prompt.system.as_str()),
            ),
            (
                "prompt.instructions",
                self.prompt
                    .as_ref()
                    .and_then(|prompt| prompt.instructions.as_deref()),
            ),
            (
                "model.provider",
                self.model
                    .as_ref()
                    .and_then(|model| model.provider.as_deref()),
            ),
            (
                "model.name",
                self.model.as_ref().and_then(|model| model.name.as_deref()),
            ),
            (
                "memory.unavailable",
                self.memory
                    .as_ref()
                    .and_then(|memory| memory.unavailable.as_deref()),
            ),
        ] {
            if let Some(value) = value {
                check(field, value)?;
            }
        }
        for (category, values) in [
            (
                "tools.allowed",
                self.tools.as_ref().map(|tools| &tools.allowed),
            ),
            (
                "tools.denied",
                self.tools.as_ref().map(|tools| &tools.denied),
            ),
        ] {
            if let Some(values) = values {
                for (index, value) in values.iter().enumerate() {
                    check(&format!("{category}[{index}]"), value)?;
                }
            }
        }
        for (category, references) in [
            ("skills", self.skills.as_deref()),
            ("mcp", self.mcp.as_deref()),
            ("permissions", self.permissions.as_deref()),
            ("hooks", self.hooks.as_deref()),
        ] {
            if let Some(references) = references {
                for (index, reference) in references.iter().enumerate() {
                    check(&format!("{category}[{index}].id"), &reference.id)?;
                    if let Some(version) = &reference.version {
                        check(&format!("{category}[{index}].version"), version)?;
                    }
                }
            }
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// 加载入口
// ---------------------------------------------------------------------------

/// 加载全部 profile（Global + 各 workspace root），v1 自动迁移 v2，同 name
/// 高 tier 覆盖低 tier。跨类引用（skills）由
/// [`resolve_profile_references`] 在 bundle 内解析。
pub(crate) fn load_profiles(
    global_resource_dir: Option<&Path>,
    workspace_roots: &[PathBuf],
    workspace_resource_dir: &str,
    selection: &ResourceSelection,
    limits: ResourceLimits,
    memory_available: bool,
) -> ProfileResolution {
    let mut resolution = ProfileResolution {
        selection: selection.clone(),
        ..ProfileResolution::default()
    };
    if let Some(global_root) = global_resource_dir {
        load_directory(
            &global_root.join("profiles"),
            global_root,
            ConfigTier::Global,
            None,
            limits,
            memory_available,
            &mut resolution,
        );
    }
    for (root_index, root) in workspace_roots.iter().enumerate() {
        let directory = match join_under_root(root, &format!("{workspace_resource_dir}/profiles")) {
            Ok(directory) => directory,
            Err(_) => root.join(workspace_resource_dir).join("profiles"),
        };
        load_directory(
            &directory,
            root,
            ConfigTier::Workspace,
            Some(root_index),
            limits,
            memory_available,
            &mut resolution,
        );
    }
    finalize(&mut resolution);
    resolution
}

/// 跨类引用解析（fail-closed）：skills 引用必须在本 bundle 已加载的集合中
/// 解析成功；解析失败的 profile 被移除并给出 error 诊断。hooks 引用本波
/// 不解析（无 hooks 目录），字段留在 [`AgentProfileV2`]，不因 hooks 缺失
/// 剔除 profile。mcp / permissions 由消费方子系统解析，本层只做格式校验。
pub(crate) fn resolve_profile_references(
    resolution: &mut ProfileResolution,
    skills: &SkillResolution,
) {
    let mut failed: Vec<(String, ResourceProvenance, Vec<ResourceIssue>)> = Vec::new();
    resolution.effective.retain(|name, loaded| {
        let mut issues = Vec::new();
        for reference in &loaded.profile.skills {
            let resolvable = skills.skills.iter().any(|skill| {
                skill.manifest.id == reference.id
                    && version_matches(reference, &skill.manifest.version)
            });
            if !resolvable {
                issues.push(
                    ResourceIssue::error(
                        "agent_profile_skill_ref_unresolved",
                        format!(
                            "profile '{name}' references skill '{}'{} which is not loaded",
                            reference.id,
                            reference
                                .version
                                .as_deref()
                                .map(|v| format!(" (requirement '{v}')"))
                                .unwrap_or_default()
                        ),
                    )
                    .for_resource(
                        ResourceKind::AgentProfile,
                        name,
                        loaded.provenance.source_key.clone(),
                    ),
                );
            }
        }
        if issues.is_empty() {
            true
        } else {
            failed.push((name.clone(), loaded.provenance.clone(), issues));
            false
        }
    });
    for (_, _, issues) in failed {
        resolution.diagnostics.issues.extend(issues);
    }
    finalize(resolution);
}

fn version_matches(reference: &ProfileRef, available: &Version) -> bool {
    match reference.version.as_deref() {
        None | Some("*") | Some("latest") => true,
        Some(requirement) => {
            VersionReq::parse(requirement).is_ok_and(|parsed| parsed.matches(available))
        }
    }
}

/// 由 effective 集合重建：v1 兼容视图、指令分层、诊断条目。
fn finalize(resolution: &mut ProfileResolution) {
    let mut entries = resolution.overridden.clone();
    resolution.profiles = resolution
        .effective
        .values()
        .map(LoadedAgentProfileV2::compat_view)
        .collect();
    resolution.profiles_v2 = resolution.effective.values().cloned().collect();

    let selected_name = resolution
        .selection
        .profile
        .as_ref()
        .filter(|name| resolution.effective.contains_key(*name));
    for loaded in resolution.effective.values() {
        entries.push(ResourceDiagnosticEntry {
            kind: ResourceKind::AgentProfile,
            resource_id: loaded.profile.name.clone(),
            status: if selected_name == Some(&loaded.profile.name) {
                ResourceDiagnosticStatus::Active
            } else {
                ResourceDiagnosticStatus::Loaded
            },
            provenance: loaded.provenance.clone(),
        });
    }

    let mut instruction_issues = Vec::new();
    if let Some(name) = &resolution.selection.profile {
        if selected_name.is_none() {
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
    let selected_profile = selected_name.and_then(|name| resolution.effective.get(name));
    resolution.instructions = ResolvedInstructions {
        profile: selected_profile.map(LoadedAgentProfileV2::compat_view),
        session: resolution
            .selection
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
        run: resolution
            .selection
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
    // finalize 可能被多次调用（加载后、引用解析后），选择类 issue 需先清理
    // 旧副本再写入，避免重复；其余（解析 / 引用 / memory）issue 只产生一次。
    resolution
        .diagnostics
        .issues
        .retain(|issue| issue.code != "agent_profile_not_found");
    resolution.diagnostics.issues.extend(instruction_issues);
    if let Some(session) = &resolution.instructions.session {
        entries.push(ResourceDiagnosticEntry {
            kind: ResourceKind::Instructions,
            resource_id: "session".into(),
            status: ResourceDiagnosticStatus::Active,
            provenance: session.provenance.clone(),
        });
    }
    if let Some(run) = &resolution.instructions.run {
        entries.push(ResourceDiagnosticEntry {
            kind: ResourceKind::Instructions,
            resource_id: "run".into(),
            status: ResourceDiagnosticStatus::Active,
            provenance: run.provenance.clone(),
        });
    }
    resolution.diagnostics.entries = entries;
    resolution.diagnostics.sort_deterministically();
}

fn load_directory(
    directory: &Path,
    source_root: &Path,
    tier: ConfigTier,
    root_index: Option<usize>,
    limits: ResourceLimits,
    memory_available: bool,
    resolution: &mut ProfileResolution,
) {
    let paths = match sorted_children_within(directory, source_root, limits.max_resources_per_kind)
    {
        Ok(paths) => paths,
        Err(error) => {
            resolution.diagnostics.issues.push(ResourceIssue::error(
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
            Ok((mut loaded, mut warnings)) => {
                apply_memory_availability(
                    &mut loaded.profile,
                    memory_available,
                    &source_key,
                    &mut warnings,
                );
                resolution.diagnostics.issues.append(&mut warnings);
                if let Some(overridden) = resolution
                    .effective
                    .insert(loaded.profile.name.clone(), loaded)
                {
                    resolution.overridden.push(ResourceDiagnosticEntry {
                        kind: ResourceKind::AgentProfile,
                        resource_id: overridden.profile.name,
                        status: ResourceDiagnosticStatus::Overridden,
                        provenance: overridden.provenance,
                    });
                }
            }
            Err(issue) => resolution.diagnostics.issues.push(issue),
        }
    }
}

fn parse_profile(
    path: &Path,
    source_root: &Path,
    provenance: ResourceProvenance,
    max_file_bytes: u64,
) -> Result<(LoadedAgentProfileV2, Vec<ResourceIssue>), ResourceIssue> {
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
    let file: ProfileFileRaw = crate::io::parse_toml_resource(
        &content,
        "agent_profile_invalid",
        "agent profile has invalid TOML syntax or unsupported fields",
    )
    .map_err(|issue| {
        issue.for_resource(
            ResourceKind::AgentProfile,
            &fallback_name,
            source_key.clone(),
        )
    })?;
    file.validate_no_plaintext_secrets(&fallback_name, &source_key)?;
    let v1_only_fields = file.v1_only_fields();
    let v2_only_fields = file.v2_only_fields();
    let schema_kind = match file.schema.as_deref() {
        Some("v1") if !v2_only_fields.is_empty() => {
            return Err(profile_schema_conflict(
                &fallback_name,
                &source_key,
                "schema 'v1' cannot contain v2-only fields",
                &v2_only_fields,
            ));
        }
        Some("v1") => SchemaKind::V1,
        Some("v2") if !v1_only_fields.is_empty() => {
            return Err(profile_schema_conflict(
                &fallback_name,
                &source_key,
                "schema 'v2' cannot contain v1-only fields",
                &v1_only_fields,
            ));
        }
        Some("v2") => SchemaKind::V2,
        Some(other) => {
            return Err(ResourceIssue::error(
                "agent_profile_schema_invalid",
                format!("unsupported agent profile schema '{other}' (expected 'v1' or 'v2')"),
            )
            .for_resource(
                ResourceKind::AgentProfile,
                &fallback_name,
                source_key.clone(),
            ))
        }
        None if !v1_only_fields.is_empty() && !v2_only_fields.is_empty() => {
            let mut mixed_fields = v1_only_fields;
            mixed_fields.extend(v2_only_fields);
            return Err(profile_schema_conflict(
                &fallback_name,
                &source_key,
                "schema-less profile cannot mix v1-only and v2-only fields",
                &mixed_fields,
            ));
        }
        None if !v2_only_fields.is_empty() => SchemaKind::V2,
        None => SchemaKind::V1,
    };

    let name = file.name.clone().unwrap_or_else(|| fallback_name.clone());
    validate_name(&name).map_err(|message| {
        ResourceIssue::error("agent_profile_name_invalid", message).for_resource(
            ResourceKind::AgentProfile,
            &name,
            source_key.clone(),
        )
    })?;

    let profile = match schema_kind {
        SchemaKind::V1 => migrate_v1(file, &name)?,
        SchemaKind::V2 => build_v2(file, &name)?,
    };
    validate_profile(&profile, &name, &source_key)?;

    Ok((
        LoadedAgentProfileV2 {
            profile,
            provenance,
        },
        Vec::new(),
    ))
}

fn profile_schema_conflict(
    resource_id: &str,
    source_key: &str,
    message: &str,
    fields: &[&str],
) -> ResourceIssue {
    ResourceIssue::error(
        "agent_profile_schema_field_conflict",
        format!("{message}: {}", fields.join(", ")),
    )
    .for_resource(ResourceKind::AgentProfile, resource_id, source_key)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SchemaKind {
    V1,
    V2,
}

/// v1 → v2 迁移：`instructions` → `prompt.system`，`default_provider` /
/// `default_model` → `model.provider` / `model.name`；其余维度取默认。
fn migrate_v1(file: ProfileFileRaw, name: &str) -> Result<AgentProfileV2, ResourceIssue> {
    let instructions = file.instructions.unwrap_or_default();
    if instructions.trim().is_empty() {
        return Err(ResourceIssue::error(
            "agent_profile_instructions_empty",
            "agent profile instructions may not be empty",
        )
        .for_resource(ResourceKind::AgentProfile, name, name));
    }
    Ok(AgentProfileV2 {
        name: name.to_owned(),
        prompt: ProfilePrompt {
            system: instructions,
            instructions: None,
        },
        model: ProfileModel {
            provider: file.default_provider,
            name: file.default_model,
        },
        effort: ReasoningEffort::default(),
        tools: ProfileToolRules::default(),
        skills: Vec::new(),
        mcp: Vec::new(),
        permissions: Vec::new(),
        hooks: Vec::new(),
        memory: ProfileMemory::default(),
        max_turns: None,
        background: false,
        isolation: ProfileIsolation::default(),
    })
}

fn build_v2(file: ProfileFileRaw, name: &str) -> Result<AgentProfileV2, ResourceIssue> {
    let prompt = file.prompt.as_ref().ok_or_else(|| {
        ResourceIssue::error(
            "agent_profile_prompt_missing",
            "v2 agent profile requires a [prompt] section with 'system'",
        )
        .for_resource(ResourceKind::AgentProfile, name, name)
    })?;
    Ok(AgentProfileV2 {
        name: name.to_owned(),
        prompt: ProfilePrompt {
            system: prompt.system.clone(),
            instructions: prompt.instructions.clone(),
        },
        model: ProfileModel {
            provider: file.model.as_ref().and_then(|model| model.provider.clone()),
            name: file.model.as_ref().and_then(|model| model.name.clone()),
        },
        effort: file.effort.unwrap_or_default(),
        tools: ProfileToolRules {
            allowed: file
                .tools
                .as_ref()
                .map_or_else(Vec::new, |tools| tools.allowed.clone()),
            denied: file
                .tools
                .as_ref()
                .map_or_else(Vec::new, |tools| tools.denied.clone()),
        },
        skills: file.skills.unwrap_or_default(),
        mcp: file.mcp.unwrap_or_default(),
        permissions: file.permissions.unwrap_or_default(),
        hooks: file.hooks.unwrap_or_default(),
        memory: ProfileMemory {
            enabled: file.memory.as_ref().is_some_and(|memory| memory.enabled),
            privacy: file
                .memory
                .as_ref()
                .map_or_else(MemoryPrivacy::default, |memory| memory.privacy),
            unavailable: file
                .memory
                .as_ref()
                .and_then(|memory| memory.unavailable.clone()),
        },
        max_turns: file.max_turns,
        background: file.background.unwrap_or(false),
        isolation: file.isolation.unwrap_or_default(),
    })
}

// ---------------------------------------------------------------------------
// v2 校验（fail-closed）
// ---------------------------------------------------------------------------

fn validate_profile(
    profile: &AgentProfileV2,
    name: &str,
    source_key: &str,
) -> Result<(), ResourceIssue> {
    validate_no_plaintext_secrets(profile, name, source_key)?;

    if profile.prompt.system.trim().is_empty() {
        return Err(ResourceIssue::error(
            "agent_profile_prompt_system_empty",
            "agent profile prompt.system may not be empty",
        )
        .for_resource(ResourceKind::AgentProfile, name, source_key));
    }
    if let Some(instructions) = &profile.prompt.instructions {
        if instructions.trim().is_empty() {
            return Err(ResourceIssue::error(
                "agent_profile_prompt_instructions_empty",
                "agent profile prompt.instructions may not be empty when present",
            )
            .for_resource(ResourceKind::AgentProfile, name, source_key));
        }
    }
    for (label, value) in [
        ("model.provider", profile.model.provider.as_deref()),
        ("model.name", profile.model.name.as_deref()),
    ] {
        if let Some(value) = value {
            if value.trim().is_empty() {
                return Err(ResourceIssue::error(
                    "agent_profile_model_field_empty",
                    format!("agent profile {label} may not be empty when present"),
                )
                .for_resource(ResourceKind::AgentProfile, name, source_key));
            }
        }
    }

    validate_tool_rules(&profile.tools, name, source_key)?;
    for (category, references) in [
        ("skills", &profile.skills),
        ("mcp", &profile.mcp),
        ("permissions", &profile.permissions),
        ("hooks", &profile.hooks),
    ] {
        validate_references(category, references, name, source_key)?;
    }

    if let Some(max_turns) = profile.max_turns {
        if max_turns == 0 {
            return Err(ResourceIssue::error(
                "agent_profile_max_turns_invalid",
                "agent profile max_turns must be at least 1 when present",
            )
            .for_resource(ResourceKind::AgentProfile, name, source_key));
        }
    }

    Ok(())
}

/// 扫描 canonical profile 的全部自由字符串。校验必须先于会回显原值的格式
/// 检查；错误只携带字段路径，绝不把可疑值写入诊断。
fn validate_no_plaintext_secrets(
    profile: &AgentProfileV2,
    resource_id: &str,
    source_key: &str,
) -> Result<(), ResourceIssue> {
    let check = |field: &str, value: &str| {
        if contains_plaintext_secret(value) {
            return Err(plaintext_secret_issue(resource_id, source_key, field));
        }
        Ok(())
    };

    check("name", &profile.name)?;
    check("prompt.system", &profile.prompt.system)?;
    if let Some(instructions) = &profile.prompt.instructions {
        check("prompt.instructions", instructions)?;
    }
    if let Some(provider) = &profile.model.provider {
        check("model.provider", provider)?;
    }
    if let Some(model) = &profile.model.name {
        check("model.name", model)?;
    }
    for (index, tool) in profile.tools.allowed.iter().enumerate() {
        check(&format!("tools.allowed[{index}]"), tool)?;
    }
    for (index, tool) in profile.tools.denied.iter().enumerate() {
        check(&format!("tools.denied[{index}]"), tool)?;
    }
    for (category, references) in [
        ("skills", &profile.skills),
        ("mcp", &profile.mcp),
        ("permissions", &profile.permissions),
        ("hooks", &profile.hooks),
    ] {
        for (index, reference) in references.iter().enumerate() {
            check(&format!("{category}[{index}].id"), &reference.id)?;
            if let Some(version) = &reference.version {
                check(&format!("{category}[{index}].version"), version)?;
            }
        }
    }
    if let Some(reason) = &profile.memory.unavailable {
        check("memory.unavailable", reason)?;
    }
    Ok(())
}

fn plaintext_secret_issue(resource_id: &str, source_key: &str, field: &str) -> ResourceIssue {
    ResourceIssue::error(
        "agent_profile_plaintext_secret",
        format!("agent profile field '{field}' must not carry a plaintext secret"),
    )
    .for_resource(ResourceKind::AgentProfile, resource_id, source_key)
}

fn validate_tool_rules(
    rules: &ProfileToolRules,
    name: &str,
    source_key: &str,
) -> Result<(), ResourceIssue> {
    for (label, list) in [
        ("tools.allowed", &rules.allowed),
        ("tools.denied", &rules.denied),
    ] {
        let mut seen = std::collections::BTreeSet::new();
        for tool in list {
            if !crate::io::is_valid_identifier(tool, true) {
                return Err(ResourceIssue::error(
                    "agent_profile_tool_name_invalid",
                    format!("agent profile {label} entry '{tool}' must be a valid tool identifier"),
                )
                .for_resource(ResourceKind::AgentProfile, name, source_key));
            }
            if !seen.insert(tool.clone()) {
                return Err(ResourceIssue::error(
                    "agent_profile_tool_duplicate",
                    format!("agent profile {label} lists tool '{tool}' more than once"),
                )
                .for_resource(ResourceKind::AgentProfile, name, source_key));
            }
        }
    }
    Ok(())
}

fn validate_references(
    category: &str,
    references: &[ProfileRef],
    name: &str,
    source_key: &str,
) -> Result<(), ResourceIssue> {
    let mut seen = std::collections::BTreeSet::new();
    for reference in references {
        if !crate::io::is_valid_identifier(&reference.id, true) {
            return Err(ResourceIssue::error(
                "agent_profile_ref_id_invalid",
                format!(
                    "agent profile {category} reference id '{}' must be a valid identifier",
                    reference.id
                ),
            )
            .for_resource(ResourceKind::AgentProfile, name, source_key));
        }
        if let Some(version) = &reference.version {
            if version != "*" && version != "latest" && VersionReq::parse(version).is_err() {
                return Err(
                    ResourceIssue::error(
                        "agent_profile_ref_version_invalid",
                        format!(
                            "agent profile {category} reference '{}' has invalid version pin '{version}'",
                            reference.id
                        ),
                    )
                    .for_resource(ResourceKind::AgentProfile, name, source_key),
                );
            }
        }
        if !seen.insert(reference.id.clone()) {
            return Err(ResourceIssue::error(
                "agent_profile_ref_duplicate",
                format!(
                    "agent profile {category} references '{}' more than once",
                    reference.id
                ),
            )
            .for_resource(ResourceKind::AgentProfile, name, source_key));
        }
    }
    Ok(())
}

/// 高信号明文 secret 模式扫描（值级兜底；结构上 v2 文件格式无 secret 字段，
/// `deny_unknown_fields` 已拒绝任何 secret 键）。
fn contains_plaintext_secret(text: &str) -> bool {
    fn has_token(text: &str, prefix: &str, min_tail: usize, tail: impl Fn(char) -> bool) -> bool {
        let mut rest = text;
        while let Some(index) = rest.find(prefix) {
            let tail_text = &rest[index + prefix.len()..];
            let count = tail_text.chars().take_while(|c| tail(*c)).count();
            if count >= min_tail {
                return true;
            }
            rest = &rest[index + 1..];
        }
        false
    }

    let alnum = |c: char| c.is_ascii_alphanumeric();
    if has_token(text, "sk-", 8, alnum)
        || has_token(text, "ghp_", 16, alnum)
        || has_token(text, "xoxb-", 8, |c| c.is_ascii_alphanumeric() || c == '-')
        || has_token(text, "xoxp-", 8, |c| c.is_ascii_alphanumeric() || c == '-')
        || has_token(text, "AKIA", 16, |c| {
            c.is_ascii_uppercase() || c.is_ascii_digit()
        })
        || text.contains("-----BEGIN") && text.contains("PRIVATE KEY-----")
        || has_token(text, "eyJ", 24, |c| {
            c.is_ascii_alphanumeric() || matches!(c, '-' | '_')
        })
    {
        return true;
    }
    // `Bearer <long token>` 形态。
    has_token(text, "Bearer ", 24, |c| !c.is_whitespace())
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

/// memory 显式可用性标注：default-off 已由类型默认保证；显式启用但生产记忆
/// 不可用时，把 `unavailable` 显式写入 profile 并产生 warning 诊断——绝不把
/// 不可用的记忆标记为可用。文件作者显式声明的 `unavailable` 保持原样。
fn apply_memory_availability(
    profile: &mut AgentProfileV2,
    memory_available: bool,
    source_key: &str,
    warnings: &mut Vec<ResourceIssue>,
) {
    if !profile.memory.enabled || profile.memory.unavailable.is_some() || memory_available {
        return;
    }
    profile.memory.unavailable = Some(
        "production long-term memory is not wired (P16-10 deferred); \
         memory is explicitly unavailable"
            .to_owned(),
    );
    warnings.push(
        ResourceIssue::warning(
            "agent_profile_memory_unavailable",
            format!(
                "profile '{}' enables memory, but production long-term memory is not \
                 available; memory is explicitly marked unavailable (never falsely available)",
                profile.name
            ),
        )
        .for_resource(ResourceKind::AgentProfile, &profile.name, source_key),
    );
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

    fn load(
        global: Option<&Path>,
        workspace: &Path,
        selection: ResourceSelection,
    ) -> ProfileResolution {
        let mut resolution = load_profiles(
            global,
            &[workspace.to_path_buf()],
            ".pawork",
            &selection,
            ResourceLimits::default(),
            false,
        );
        resolve_profile_references(&mut resolution, &SkillResolution::default());
        resolution
    }

    fn probe(workspace: &Path, selection: ResourceSelection) -> ProfileResolution {
        load(None, workspace, selection)
    }

    fn load_raw(workspace: &Path, selection: ResourceSelection) -> ProfileResolution {
        load_profiles(
            None,
            &[workspace.to_path_buf()],
            ".pawork",
            &selection,
            ResourceLimits::default(),
            false,
        )
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
        let resolution = probe(workspace.path(), selection);
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
        let resolution = probe(workspace.path(), selection);
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
        // finalize 被调用多次（加载 + 引用解析）也不得产生重复诊断。
        assert_eq!(
            resolution
                .diagnostics
                .issues
                .iter()
                .filter(|issue| issue.code == "agent_profile_not_found")
                .count(),
            1
        );
    }

    #[test]
    fn v2_profile_loads_all_dimensions_and_migrates_v1_compat_view() {
        let workspace = tempfile::tempdir().expect("workspace");
        write_profile(
            workspace.path(),
            ".pawork/profiles/reviewer.toml",
            r#"
                schema = "v2"
                name = "reviewer"
                effort = "high"
                max_turns = 120
                background = true
                isolation = "restricted"

                [prompt]
                system = "You are a careful reviewer."
                instructions = "Prefer minimal diffs."

                [model]
                provider = "default"
                name = "review-model"

                [tools]
                allowed = ["read_file"]
                denied = ["shell"]

                [[skills]]
                id = "rust"
                version = "^1.2.0"

                [[mcp]]
                id = "filesystem"

                [[permissions]]
                id = "read-only"

                [[hooks]]
                id = "on-completion"

                [memory]
                enabled = false
                privacy = "workspace_local"
            "#,
        );
        // 全维度往返不涉及跨类引用解析（该语义由专门测试覆盖）。
        let resolution = load_raw(workspace.path(), ResourceSelection::default());
        assert!(
            resolution.diagnostics.issues.is_empty(),
            "{:?}",
            resolution.diagnostics.issues
        );
        let loaded = &resolution.profiles_v2[0];
        assert_eq!(loaded.profile.name, "reviewer");
        assert_eq!(loaded.profile.prompt.system, "You are a careful reviewer.");
        assert_eq!(loaded.profile.effort, ReasoningEffort::High);
        assert_eq!(loaded.profile.model.provider.as_deref(), Some("default"));
        assert_eq!(loaded.profile.model.name.as_deref(), Some("review-model"));
        assert_eq!(
            loaded.profile.tools.policy("shell"),
            pawork_domain::ToolPolicyDecision::Denied
        );
        assert_eq!(
            loaded.profile.tools.policy("read_file"),
            pawork_domain::ToolPolicyDecision::Allowed
        );
        assert_eq!(
            loaded.profile.skills,
            vec![ProfileRef {
                id: "rust".into(),
                version: Some("^1.2.0".into())
            }]
        );
        assert_eq!(loaded.profile.mcp, vec![ProfileRef::new("filesystem")]);
        assert_eq!(
            loaded.profile.permissions,
            vec![ProfileRef::new("read-only")]
        );
        assert_eq!(loaded.profile.hooks, vec![ProfileRef::new("on-completion")]);
        assert_eq!(
            loaded.profile.memory.availability(),
            pawork_domain::ProfileMemoryAvailability::Disabled
        );
        assert_eq!(loaded.profile.max_turns, Some(120));
        assert!(loaded.profile.background);
        assert_eq!(loaded.profile.isolation, ProfileIsolation::Restricted);

        // v1 兼容视图与指令分层仍可用（system + instructions 拼接）。
        let compat = &resolution.profiles[0];
        assert!(compat.instructions.contains("You are a careful reviewer."));
        assert!(compat.instructions.contains("Prefer minimal diffs."));
    }

    #[test]
    fn v1_profile_migrates_to_v2_automatically() {
        let workspace = tempfile::tempdir().expect("workspace");
        write_profile(
            workspace.path(),
            ".pawork/profiles/legacy.toml",
            "name='legacy'\ninstructions='keep focused'\ndefault_provider='p1'\ndefault_model='m1'",
        );
        let selection = ResourceSelection {
            profile: Some("legacy".into()),
            ..ResourceSelection::default()
        };
        let resolution = probe(workspace.path(), selection);
        assert!(
            resolution.diagnostics.issues.is_empty(),
            "{:?}",
            resolution.diagnostics.issues
        );
        let loaded = &resolution.profiles_v2[0];
        assert_eq!(loaded.profile.prompt.system, "keep focused");
        assert_eq!(loaded.profile.prompt.instructions, None);
        assert_eq!(loaded.profile.model.provider.as_deref(), Some("p1"));
        assert_eq!(loaded.profile.model.name.as_deref(), Some("m1"));
        assert_eq!(loaded.profile.effort, ReasoningEffort::default());
        assert_eq!(
            loaded.profile.memory.availability(),
            pawork_domain::ProfileMemoryAvailability::Disabled
        );
        assert!(!loaded.profile.background);
        assert_eq!(loaded.profile.isolation, ProfileIsolation::default());
        // 指令层与 v1 行为一致。
        assert_eq!(
            resolution
                .instructions
                .profile
                .as_ref()
                .map(|p| p.instructions.as_str()),
            Some("keep focused")
        );
    }

    #[test]
    fn denied_tool_wins_over_allowed_and_cannot_be_bypassed() {
        let rules = ProfileToolRules {
            allowed: vec!["shell".into(), "read_file".into()],
            denied: vec!["shell".into()],
        };
        assert_eq!(
            rules.policy("shell"),
            pawork_domain::ToolPolicyDecision::Denied
        );
        assert!(rules.is_denied("shell"));

        // 加载层：重叠清单合法，但裁决始终 deny 优先。
        let workspace = tempfile::tempdir().expect("workspace");
        write_profile(
            workspace.path(),
            ".pawork/profiles/tooly.toml",
            r#"
                schema = "v2"
                [prompt]
                system = "s"
                [tools]
                allowed = ["shell", "read_file"]
                denied = ["shell"]
            "#,
        );
        let resolution = probe(workspace.path(), ResourceSelection::default());
        assert!(
            resolution.diagnostics.issues.is_empty(),
            "{:?}",
            resolution.diagnostics.issues
        );
        assert_eq!(
            resolution.profiles_v2[0].profile.tools.policy("shell"),
            pawork_domain::ToolPolicyDecision::Denied
        );
    }

    #[test]
    fn duplicate_tool_names_and_invalid_names_fail_closed() {
        let workspace = tempfile::tempdir().expect("workspace");
        write_profile(
            workspace.path(),
            ".pawork/profiles/dup.toml",
            r#"
                schema = "v2"
                [prompt]
                system = "s"
                [tools]
                allowed = ["read_file", "read_file"]
            "#,
        );
        let resolution = probe(workspace.path(), ResourceSelection::default());
        assert!(resolution
            .diagnostics
            .issues
            .iter()
            .any(|issue| issue.code == "agent_profile_tool_duplicate"));
        assert!(resolution.profiles_v2.is_empty());

        let workspace = tempfile::tempdir().expect("workspace");
        write_profile(
            workspace.path(),
            ".pawork/profiles/bad.toml",
            r#"
                schema = "v2"
                [prompt]
                system = "s"
                [tools]
                denied = ["bad tool name"]
            "#,
        );
        let resolution = probe(workspace.path(), ResourceSelection::default());
        assert!(resolution
            .diagnostics
            .issues
            .iter()
            .any(|issue| issue.code == "agent_profile_tool_name_invalid"));
    }

    #[test]
    fn unresolved_skill_refs_are_errors_but_hooks_are_skipped() {
        let workspace = tempfile::tempdir().expect("workspace");
        write_profile(
            workspace.path(),
            ".pawork/profiles/refy.toml",
            r#"
                schema = "v2"
                [prompt]
                system = "s"
                [[skills]]
                id = "rust"
                version = "^1.2.0"
                [[hooks]]
                id = "on-completion"
                [[mcp]]
                id = "filesystem"
                [[permissions]]
                id = "read-only"
            "#,
        );
        let mut resolution = load_profiles(
            None,
            &[workspace.path().to_path_buf()],
            ".pawork",
            &ResourceSelection::default(),
            ResourceLimits::default(),
            false,
        );
        resolve_profile_references(&mut resolution, &SkillResolution::default());
        assert!(resolution
            .diagnostics
            .issues
            .iter()
            .any(|issue| issue.code == "agent_profile_skill_ref_unresolved"));
        assert!(
            resolution
                .diagnostics
                .issues
                .iter()
                .all(|issue| issue.code != "agent_profile_hook_ref_unresolved"),
            "hooks refs are not resolved this wave"
        );
        assert!(
            resolution.profiles_v2.is_empty(),
            "profile with unresolved skill refs must be dropped"
        );

        let workspace = tempfile::tempdir().expect("workspace");
        write_profile(
            workspace.path(),
            ".pawork/profiles/hooks-only.toml",
            r#"
                schema = "v2"
                [prompt]
                system = "s"
                [[hooks]]
                id = "on-completion"
                [[mcp]]
                id = "filesystem"
                [[permissions]]
                id = "read-only"
            "#,
        );
        let resolution = probe(workspace.path(), ResourceSelection::default());
        assert!(
            resolution.diagnostics.issues.is_empty(),
            "{:?}",
            resolution.diagnostics.issues
        );
        assert_eq!(resolution.profiles_v2.len(), 1);
        assert_eq!(
            resolution.profiles_v2[0].profile.hooks,
            vec![ProfileRef::new("on-completion")]
        );
    }

    #[test]
    fn skill_ref_resolves_against_loaded_skill_with_version_req() {
        let skill = crate::skills::LoadedSkill {
            manifest: crate::skills::SkillManifest {
                id: "rust".into(),
                version: Version::new(1, 3, 0),
                description: String::new(),
                parameters: Vec::new(),
                dependencies: Vec::new(),
                conflicts: Vec::new(),
                scripts: Vec::new(),
                assets: Vec::new(),
                permissions: Vec::new(),
            },
            skill_markdown: String::new(),
            provenance: ResourceProvenance::new(
                ConfigTier::Workspace,
                "workspace:00000000:skill:skills/rust/manifest.toml",
                ResourceOrigin::Workspace {
                    root_index: 0,
                    relative_path: "skills/rust/manifest.toml".into(),
                },
            ),
        };
        let skills = SkillResolution {
            skills: vec![skill],
            diagnostics: ResourceDiagnostics::default(),
        };
        let workspace = tempfile::tempdir().expect("workspace");
        write_profile(
            workspace.path(),
            ".pawork/profiles/ok.toml",
            r#"
                schema = "v2"
                [prompt]
                system = "s"
                [[skills]]
                id = "rust"
                version = "^1.2.0"
            "#,
        );
        let mut resolution = load_profiles(
            None,
            &[workspace.path().to_path_buf()],
            ".pawork",
            &ResourceSelection::default(),
            ResourceLimits::default(),
            false,
        );
        resolve_profile_references(&mut resolution, &skills);
        assert!(
            resolution.diagnostics.issues.is_empty(),
            "{:?}",
            resolution.diagnostics.issues
        );
        assert_eq!(resolution.profiles_v2.len(), 1);

        // 版本不满足 → fail-closed。
        let workspace = tempfile::tempdir().expect("workspace");
        write_profile(
            workspace.path(),
            ".pawork/profiles/old.toml",
            r#"
                schema = "v2"
                [prompt]
                system = "s"
                [[skills]]
                id = "rust"
                version = "=1.2.0"
            "#,
        );
        let mut resolution = load_profiles(
            None,
            &[workspace.path().to_path_buf()],
            ".pawork",
            &ResourceSelection::default(),
            ResourceLimits::default(),
            false,
        );
        resolve_profile_references(&mut resolution, &skills);
        assert!(resolution
            .diagnostics
            .issues
            .iter()
            .any(|issue| issue.code == "agent_profile_skill_ref_unresolved"));
    }

    #[test]
    fn invalid_version_pin_and_duplicate_refs_fail_closed() {
        let workspace = tempfile::tempdir().expect("workspace");
        write_profile(
            workspace.path(),
            ".pawork/profiles/pin.toml",
            r#"
                schema = "v2"
                [prompt]
                system = "s"
                [[skills]]
                id = "rust"
                version = "not-a-version"
            "#,
        );
        let resolution = probe(workspace.path(), ResourceSelection::default());
        assert!(resolution
            .diagnostics
            .issues
            .iter()
            .any(|issue| issue.code == "agent_profile_ref_version_invalid"));

        let workspace = tempfile::tempdir().expect("workspace");
        write_profile(
            workspace.path(),
            ".pawork/profiles/dupref.toml",
            r#"
                schema = "v2"
                [prompt]
                system = "s"
                [[mcp]]
                id = "filesystem"
                [[mcp]]
                id = "filesystem"
            "#,
        );
        let resolution = probe(workspace.path(), ResourceSelection::default());
        assert!(resolution
            .diagnostics
            .issues
            .iter()
            .any(|issue| issue.code == "agent_profile_ref_duplicate"));
    }

    #[test]
    fn memory_is_default_off_and_never_falsely_available() {
        let workspace = tempfile::tempdir().expect("workspace");
        write_profile(
            workspace.path(),
            ".pawork/profiles/mem.toml",
            r#"
                schema = "v2"
                [prompt]
                system = "s"
                [memory]
                enabled = true
            "#,
        );
        // 生产记忆不可用 → 显式 Unavailable + warning，不虚假可用。
        let mut resolution = load_profiles(
            None,
            &[workspace.path().to_path_buf()],
            ".pawork",
            &ResourceSelection::default(),
            ResourceLimits::default(),
            false,
        );
        resolve_profile_references(&mut resolution, &SkillResolution::default());
        assert!(resolution
            .diagnostics
            .issues
            .iter()
            .any(|issue| issue.code == "agent_profile_memory_unavailable"));
        let profile = &resolution.profiles_v2[0].profile;
        assert_eq!(
            profile.memory.availability(),
            pawork_domain::ProfileMemoryAvailability::Unavailable
        );

        // 生产记忆可用 → Enabled。
        let mut resolution = load_profiles(
            None,
            &[workspace.path().to_path_buf()],
            ".pawork",
            &ResourceSelection::default(),
            ResourceLimits::default(),
            true,
        );
        resolve_profile_references(&mut resolution, &SkillResolution::default());
        assert!(
            resolution.diagnostics.issues.is_empty(),
            "{:?}",
            resolution.diagnostics.issues
        );
        assert_eq!(
            resolution.profiles_v2[0].profile.memory.availability(),
            pawork_domain::ProfileMemoryAvailability::Enabled
        );

        // 文件显式标注 unavailable 被保留（诚实的显式不可用）。
        let workspace = tempfile::tempdir().expect("workspace");
        write_profile(
            workspace.path(),
            ".pawork/profiles/memoff.toml",
            r#"
                schema = "v2"
                [prompt]
                system = "s"
                [memory]
                enabled = true
                unavailable = "explicitly unavailable by author"
            "#,
        );
        let resolution = probe(workspace.path(), ResourceSelection::default());
        assert_eq!(
            resolution.profiles_v2[0].profile.memory.availability(),
            pawork_domain::ProfileMemoryAvailability::Unavailable
        );
    }

    #[test]
    fn plaintext_secrets_are_rejected_by_structure_and_content() {
        let workspace = tempfile::tempdir().expect("workspace");
        write_profile(
            workspace.path(),
            ".pawork/profiles/leak.toml",
            r#"
                schema = "v2"
                [prompt]
                system = "use api_key = 'sk-1234567890abcdef' to fetch"
            "#,
        );
        let resolution = probe(workspace.path(), ResourceSelection::default());
        assert!(resolution
            .diagnostics
            .issues
            .iter()
            .any(|issue| issue.code == "agent_profile_plaintext_secret"));

        // 结构层：任何 secret 键都被 deny_unknown_fields 拒绝。
        let workspace = tempfile::tempdir().expect("workspace");
        write_profile(
            workspace.path(),
            ".pawork/profiles/key.toml",
            r#"
                schema = "v2"
                [prompt]
                system = "s"
                api_key = "sk-1234567890abcdef"
            "#,
        );
        let resolution = probe(workspace.path(), ResourceSelection::default());
        assert!(resolution
            .diagnostics
            .issues
            .iter()
            .any(|issue| issue.code == "agent_profile_invalid"));
    }

    #[test]
    fn every_v2_only_field_without_schema_is_never_silently_migrated_as_v1() {
        let cases = [
            ("prompt", "[prompt]\nsystem = 's'", true),
            ("model", "[model]\nname = 'm'", false),
            ("effort", "effort = 'high'", false),
            ("tools", "[tools]\nallowed = ['read_file']", false),
            ("skills", "[[skills]]\nid = 'rust'", false),
            ("mcp", "[[mcp]]\nid = 'filesystem'", false),
            ("permissions", "[[permissions]]\nid = 'read-only'", false),
            ("hooks", "[[hooks]]\nid = 'on-completion'", false),
            ("memory", "[memory]\nenabled = false", false),
            ("max_turns", "max_turns = 1", false),
            ("background", "background = true", false),
            ("isolation", "isolation = 'restricted'", false),
        ];

        for (field, fragment, has_prompt) in cases {
            let workspace = tempfile::tempdir().expect("workspace");
            write_profile(
                workspace.path(),
                &format!(".pawork/profiles/{field}.toml"),
                fragment,
            );
            let resolution = load_raw(workspace.path(), ResourceSelection::default());
            if has_prompt {
                assert_eq!(
                    resolution.profiles_v2.len(),
                    1,
                    "schema-less {field} must infer v2"
                );
            } else {
                assert!(
                    resolution
                        .diagnostics
                        .issues
                        .iter()
                        .any(|issue| issue.code == "agent_profile_prompt_missing"),
                    "schema-less v2-only field {field} must be parsed as v2 and fail explicitly: {:?}",
                    resolution.diagnostics.issues
                );
                assert!(
                    !resolution
                        .diagnostics
                        .issues
                        .iter()
                        .any(|issue| issue.code == "agent_profile_instructions_empty"),
                    "v2-only field {field} was silently treated as v1"
                );
            }

            let workspace = tempfile::tempdir().expect("workspace");
            write_profile(
                workspace.path(),
                &format!(".pawork/profiles/v1-{field}.toml"),
                &format!("schema = 'v1'\n{fragment}"),
            );
            let resolution = load_raw(workspace.path(), ResourceSelection::default());
            assert!(
                resolution
                    .diagnostics
                    .issues
                    .iter()
                    .any(|issue| issue.code == "agent_profile_schema_field_conflict"),
                "explicit v1 with v2-only field {field} must fail explicitly: {:?}",
                resolution.diagnostics.issues
            );
        }
    }

    #[test]
    fn schema_less_mixed_v1_v2_fields_fail_explicitly() {
        let workspace = tempfile::tempdir().expect("workspace");
        write_profile(
            workspace.path(),
            ".pawork/profiles/mixed.toml",
            "instructions = 'legacy'\neffort = 'high'",
        );
        let resolution = load_raw(workspace.path(), ResourceSelection::default());
        let issue = resolution
            .diagnostics
            .issues
            .iter()
            .find(|issue| issue.code == "agent_profile_schema_field_conflict")
            .expect("mixed schema must fail explicitly");
        assert!(issue.message.contains("instructions"));
        assert!(issue.message.contains("effort"));
        assert!(resolution.profiles_v2.is_empty());
    }

    #[test]
    fn plaintext_secret_scan_covers_nested_free_strings_without_leaking_values() {
        const TOKEN: &str = "ghp_abcdefghijklmnopqrst";
        let cases = [
            ("name", format!("name = '{TOKEN}'\n[prompt]\nsystem = 's'")),
            (
                "prompt.instructions",
                format!("[prompt]\nsystem = 's'\ninstructions = '{TOKEN}'"),
            ),
            (
                "model.provider",
                format!("[prompt]\nsystem = 's'\n[model]\nprovider = '{TOKEN}'"),
            ),
            (
                "tools.allowed",
                format!("[prompt]\nsystem = 's'\n[tools]\nallowed = ['{TOKEN}']"),
            ),
            (
                "skills.id",
                format!("[prompt]\nsystem = 's'\n[[skills]]\nid = '{TOKEN}'"),
            ),
            (
                "mcp.version",
                format!("[prompt]\nsystem = 's'\n[[mcp]]\nid = 'filesystem'\nversion = '{TOKEN}'"),
            ),
            (
                "memory.unavailable",
                format!("[prompt]\nsystem = 's'\n[memory]\nunavailable = '{TOKEN}'"),
            ),
            (
                "v1.default_model",
                format!("instructions = 'legacy'\ndefault_model = '{TOKEN}'"),
            ),
        ];

        for (field, content) in cases {
            let workspace = tempfile::tempdir().expect("workspace");
            write_profile(
                workspace.path(),
                &format!(".pawork/profiles/secret-{field}.toml"),
                &content,
            );
            let resolution = load_raw(workspace.path(), ResourceSelection::default());
            let issue = resolution
                .diagnostics
                .issues
                .iter()
                .find(|issue| issue.code == "agent_profile_plaintext_secret")
                .unwrap_or_else(|| panic!("nested field {field} leaked past scanner"));
            assert!(issue
                .message
                .contains(field.rsplit('.').next().expect("field")));
            assert!(!issue.message.contains(TOKEN));
            assert!(!issue
                .resource_id
                .as_deref()
                .unwrap_or_default()
                .contains(TOKEN));
            assert!(resolution.profiles_v2.is_empty());
        }
    }

    #[test]
    fn invalid_schema_max_turns_and_empty_prompt_fail_closed() {
        let workspace = tempfile::tempdir().expect("workspace");
        write_profile(
            workspace.path(),
            ".pawork/profiles/badschema.toml",
            "schema = 'v9'\n[prompt]\nsystem = 's'",
        );
        let resolution = probe(workspace.path(), ResourceSelection::default());
        assert!(resolution
            .diagnostics
            .issues
            .iter()
            .any(|issue| issue.code == "agent_profile_schema_invalid"));

        let workspace = tempfile::tempdir().expect("workspace");
        write_profile(
            workspace.path(),
            ".pawork/profiles/noturns.toml",
            "schema = 'v2'\nmax_turns = 0\n[prompt]\nsystem = 's'",
        );
        let resolution = probe(workspace.path(), ResourceSelection::default());
        assert!(resolution
            .diagnostics
            .issues
            .iter()
            .any(|issue| issue.code == "agent_profile_max_turns_invalid"));

        let workspace = tempfile::tempdir().expect("workspace");
        write_profile(
            workspace.path(),
            ".pawork/profiles/emptyprompt.toml",
            "schema = 'v2'\n[prompt]\nsystem = '  '",
        );
        let resolution = probe(workspace.path(), ResourceSelection::default());
        assert!(resolution
            .diagnostics
            .issues
            .iter()
            .any(|issue| issue.code == "agent_profile_prompt_system_empty"));
    }

    #[test]
    fn contains_plaintext_secret_detects_high_signal_patterns() {
        assert!(contains_plaintext_secret("key sk-abcdefghijklmnop"));
        assert!(contains_plaintext_secret("ghp_abcdefghijklmnopqrst"));
        assert!(contains_plaintext_secret("xoxb-abcdefghijklmnop"));
        assert!(contains_plaintext_secret("AKIAABCDEFGHIJKLMNOP"));
        assert!(contains_plaintext_secret("-----BEGIN RSA PRIVATE KEY-----"));
        assert!(contains_plaintext_secret(
            "Bearer abcdefghijklmnopqrstuvwxyz123456"
        ));
        assert!(!contains_plaintext_secret("keep focused and ask for help"));
        assert!(!contains_plaintext_secret("sk-01"));
        // 合法 credential / secret locator 只保存引用名，不是明文。
        assert!(!contains_plaintext_secret(
            "credential:provider-account-prod"
        ));
        assert!(!contains_plaintext_secret("secret:API_KEY"));
        assert!(!contains_plaintext_secret("${secret:API_KEY}"));
        assert!(!contains_plaintext_secret("secret_ref:pawork.mcp/cred-1"));
    }
}
