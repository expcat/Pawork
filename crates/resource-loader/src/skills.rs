//! Skill manifest、激活、依赖与冲突解析。
//!
//! 约定目录形态：`skills/<name>/{manifest.toml,SKILL.md}`。脚本与资产只被声明，
//! 加载阶段绝不执行；声明的路径必须相对且不含 `..`/绝对前缀。Workspace 层覆盖
//! Global 层；同层重复以警告隔离并按稳定键保留其一。激活由显式 `active` 集驱动并
//! 递归拉取依赖，`disabled` 集优先；版本依赖、缺失依赖与双向冲突均校验并隔离为
//! [`ResourceIssue`]。全流程只遍历 `BTreeMap`/`BTreeSet` 与排序后的目录项，因此
//! 输出与文件系统扫描顺序无关。

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Path, PathBuf};

use config_service::ConfigTier;
use semver::{Version, VersionReq};
use serde::{Deserialize, Serialize};

use crate::error::ResourceFileError;
use crate::io::{read_utf8_bounded_within, sorted_children_within};
use crate::request::{ResourceLimits, ResourceSelection};
use crate::source::{
    ResourceDiagnosticEntry, ResourceDiagnosticStatus, ResourceDiagnostics, ResourceIssue,
    ResourceKind, ResourceOrigin, ResourceProvenance,
};

/// 一个 Skill 的可执行参数声明。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillParameter {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub default: Option<String>,
}

/// 只声明、不执行的脚本入口；`path` 在加载阶段被校验为安全的相对路径。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillScript {
    pub name: String,
    pub path: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub arguments: Vec<String>,
}

/// 对另一个 Skill 的依赖；`version` 为 semver 需求字符串（默认 `*`）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillDependency {
    pub id: String,
    #[serde(default = "any_version_requirement")]
    pub version: String,
}

fn any_version_requirement() -> String {
    "*".to_string()
}

/// 解析后的 Skill manifest。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillManifest {
    pub id: String,
    pub version: Version,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub parameters: Vec<SkillParameter>,
    #[serde(default)]
    pub dependencies: Vec<SkillDependency>,
    #[serde(default)]
    pub conflicts: Vec<String>,
    #[serde(default)]
    pub scripts: Vec<SkillScript>,
    #[serde(default)]
    pub assets: Vec<String>,
    #[serde(default)]
    pub permissions: Vec<String>,
}

/// 已落盘加载、可被激活使用的 Skill。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoadedSkill {
    pub manifest: SkillManifest,
    pub skill_markdown: String,
    pub provenance: ResourceProvenance,
}

/// Skills 加载与激活的解析结果：`skills` 为最终生效（已激活）的集合，
/// `diagnostics` 携带全部已发现 Skill 的状态与隔离问题。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillResolution {
    pub skills: Vec<LoadedSkill>,
    pub diagnostics: ResourceDiagnostics,
}

/// 直接反序列化的原始 manifest：`version` 暂存字符串，便于给出精确的 semver 错误。
#[derive(Debug, Deserialize)]
struct RawManifest {
    id: String,
    version: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    parameters: Vec<SkillParameter>,
    #[serde(default)]
    dependencies: Vec<SkillDependency>,
    #[serde(default)]
    conflicts: Vec<String>,
    #[serde(default)]
    scripts: Vec<SkillScript>,
    #[serde(default)]
    assets: Vec<String>,
    #[serde(default)]
    permissions: Vec<String>,
}

/// 加载并解析 Global / Workspace 下的 Skills。
///
/// - `global_resource_dir`：宿主解析出的用户全局资源目录（来自配置，非模型输入）。
/// - `workspace_roots`：工作区根列表，每个根下查找 `<workspace_resource_dir>/skills`。
/// - `workspace_resource_dir`：工作区资源目录名（如 `.pawork`）。
/// - `selection`：显式激活/禁用集合。
/// - `limits`：读取与数量上限。
pub(crate) fn load_skills(
    global_resource_dir: Option<&Path>,
    workspace_roots: &[PathBuf],
    workspace_resource_dir: &str,
    selection: &ResourceSelection,
    limits: &ResourceLimits,
) -> SkillResolution {
    let mut issues: Vec<ResourceIssue> = Vec::new();
    let mut global_map: BTreeMap<String, LoadedSkill> = BTreeMap::new();
    let mut workspace_map: BTreeMap<String, LoadedSkill> = BTreeMap::new();

    if let Some(global) = global_resource_dir {
        let skills_dir = global.join("skills");
        let directories = skill_directories(&skills_dir, global, limits, &mut issues);
        for (name, dir) in directories {
            let relative_path = format!("skills/{name}/manifest.toml");
            let source_key = format!("global:skill:{relative_path}");
            let origin = ResourceOrigin::Global { relative_path };
            match load_skill_dir(
                &dir,
                global,
                &name,
                ConfigTier::Global,
                origin,
                source_key,
                limits,
            ) {
                Ok(skill) => insert_skill(&mut global_map, skill, &mut issues),
                Err(issue) => issues.push(issue),
            }
        }
    }

    for (index, root) in workspace_roots.iter().enumerate() {
        let skills_dir = root.join(workspace_resource_dir).join("skills");
        let directories = skill_directories(&skills_dir, root, limits, &mut issues);
        for (name, dir) in directories {
            let relative_path = format!("{workspace_resource_dir}/skills/{name}/manifest.toml");
            let source_key = format!("workspace:{index:08}:skill:{relative_path}");
            let origin = ResourceOrigin::Workspace {
                root_index: index,
                relative_path,
            };
            match load_skill_dir(
                &dir,
                root,
                &name,
                ConfigTier::Workspace,
                origin,
                source_key,
                limits,
            ) {
                Ok(skill) => insert_skill(&mut workspace_map, skill, &mut issues),
                Err(issue) => issues.push(issue),
            }
        }
    }

    // Workspace 覆盖 Global：合并后以 workspace 版本为准。
    let mut merged: BTreeMap<String, LoadedSkill> = BTreeMap::new();
    for (id, skill) in &global_map {
        merged.insert(id.clone(), skill.clone());
    }
    for (id, skill) in &workspace_map {
        merged.insert(id.clone(), skill.clone());
    }

    let disabled = &selection.disabled_skills;
    let active_request = &selection.active_skills;

    // 显式引用了未加载的技能：仅作警告，不影响其他技能。
    for id in active_request {
        if !disabled.contains(id) && !merged.contains_key(id) {
            issues.push(
                ResourceIssue::warning(
                    "skill_active_unknown",
                    format!("active skill '{id}' is not loaded"),
                )
                .for_resource(
                    ResourceKind::Skill,
                    id.clone(),
                    "selection".to_string(),
                ),
            );
        }
    }
    for id in disabled {
        if !merged.contains_key(id) {
            issues.push(
                ResourceIssue::warning(
                    "skill_disabled_unknown",
                    format!("disabled skill '{id}' is not loaded"),
                )
                .for_resource(
                    ResourceKind::Skill,
                    id.clone(),
                    "selection".to_string(),
                ),
            );
        }
    }

    let seeds: BTreeSet<String> = active_request
        .iter()
        .filter(|id| !disabled.contains(*id))
        .cloned()
        .collect();

    // 单次 BFS：可达集与每条依赖边的判定一次性算出；激活收敛与诊断均复用其结果，
    // 不再二次匹配 semver。
    let traversal = traverse_dependencies(&merged, &seeds, disabled);
    let mut active = active_from(&seeds, &traversal);
    issues.extend(dep_issues(&merged, &traversal));

    // 双向冲突：任一方声明即视为冲突，冲突方从激活集中剔除并重新收敛依赖闭包。
    let (conflict_ids, conflict_issues) = detect_conflicts(&merged, &active);
    issues.extend(conflict_issues);
    if !conflict_ids.is_empty() {
        let mut excluded = disabled.clone();
        excluded.extend(conflict_ids);
        active = active_from(&seeds, &traverse_dependencies(&merged, &seeds, &excluded));
    }

    // 诊断条目：global 被覆盖记 Overridden，其余按激活状态标注。
    let mut entries: Vec<ResourceDiagnosticEntry> = Vec::new();
    for (id, skill) in &global_map {
        let status = if workspace_map.contains_key(id) {
            ResourceDiagnosticStatus::Overridden
        } else {
            status_for(id, &active, disabled, &traversal.candidates)
        };
        entries.push(make_entry(skill, status));
    }
    for skill in workspace_map.values() {
        let status = status_for(&skill.manifest.id, &active, disabled, &traversal.candidates);
        entries.push(make_entry(skill, status));
    }

    let mut skills_out: Vec<LoadedSkill> = active
        .iter()
        .filter_map(|id| merged.get(id).cloned())
        .collect();
    skills_out.sort_by(|left, right| left.manifest.id.cmp(&right.manifest.id));

    let mut diagnostics = ResourceDiagnostics { entries, issues };
    diagnostics.sort_deterministically();

    SkillResolution {
        skills: skills_out,
        diagnostics,
    }
}

fn skill_directories(
    skills_dir: &Path,
    source_root: &Path,
    limits: &ResourceLimits,
    issues: &mut Vec<ResourceIssue>,
) -> Vec<(String, PathBuf)> {
    let children =
        match sorted_children_within(skills_dir, source_root, limits.max_resources_per_kind) {
            Ok(children) => children,
            Err(error) => {
                issues.push(ResourceIssue::error(
                    error.code(),
                    format!("skill directory could not be loaded: {error}"),
                ));
                return Vec::new();
            }
        };
    children
        .into_iter()
        .filter_map(|path| {
            if !path.is_dir() {
                return None;
            }
            let name = path.file_name()?.to_string_lossy().into_owned();
            Some((name, path))
        })
        .collect()
}

fn insert_skill(
    map: &mut BTreeMap<String, LoadedSkill>,
    skill: LoadedSkill,
    issues: &mut Vec<ResourceIssue>,
) {
    let id = skill.manifest.id.clone();
    if let Some(existing) = map.get(&id).cloned() {
        // 同层重复：与 config-service 一致，按 source_key 升序合并、最大 key 最终覆盖。
        let (kept, dropped) = if existing.provenance.source_key <= skill.provenance.source_key {
            map.insert(id.clone(), skill.clone());
            (skill, existing)
        } else {
            (existing, skill)
        };
        issues.push(
            ResourceIssue::warning(
                "skill_duplicate",
                format!(
                    "duplicate skill '{id}' at the same tier; keeping '{}'",
                    kept.provenance.source_key
                ),
            )
            .for_resource(
                ResourceKind::Skill,
                id,
                dropped.provenance.source_key.clone(),
            ),
        );
    } else {
        map.insert(id, skill);
    }
}

fn load_skill_dir(
    dir: &Path,
    source_root: &Path,
    skill_dir_name: &str,
    tier: ConfigTier,
    origin: ResourceOrigin,
    source_key: String,
    limits: &ResourceLimits,
) -> Result<LoadedSkill, ResourceIssue> {
    let manifest_path = dir.join("manifest.toml");
    let manifest_content =
        match read_utf8_bounded_within(&manifest_path, source_root, limits.max_file_bytes) {
            Ok(content) => content,
            Err(error) => {
                return Err(file_issue(
                    "skill_manifest",
                    &manifest_path,
                    error,
                    skill_dir_name,
                    &source_key,
                ))
            }
        };

    let raw: RawManifest = crate::io::parse_toml_resource(
        &manifest_content,
        "skill_manifest_parse",
        "skill manifest has invalid TOML syntax",
    )
    .map_err(|issue| {
        issue.for_resource(
            ResourceKind::Skill,
            skill_dir_name.to_string(),
            source_key.clone(),
        )
    })?;

    let resource_id = if raw.id.trim().is_empty() {
        skill_dir_name.to_string()
    } else {
        raw.id.clone()
    };
    let tag = |code: &'static str, message: String| {
        ResourceIssue::error(code, message).for_resource(
            ResourceKind::Skill,
            resource_id.clone(),
            source_key.clone(),
        )
    };

    if raw.id.trim().is_empty() {
        return Err(tag(
            "skill_manifest_parse",
            "manifest id must not be empty".to_string(),
        ));
    }
    if !valid_identifier(&raw.id) {
        return Err(tag(
            "skill_manifest_id_invalid",
            "manifest id must use only ASCII letters, digits, '.', '-' or '_'".to_string(),
        ));
    }

    let mut parameter_names = BTreeSet::new();
    for parameter in &raw.parameters {
        if !valid_identifier(&parameter.name) || !parameter_names.insert(parameter.name.clone()) {
            return Err(tag(
                "skill_parameter_invalid",
                "parameter names must be unique safe identifiers".to_string(),
            ));
        }
    }
    let mut script_names = BTreeSet::new();
    for script in &raw.scripts {
        if !valid_identifier(&script.name) || !script_names.insert(script.name.clone()) {
            return Err(tag(
                "skill_script_invalid",
                "script names must be unique safe identifiers".to_string(),
            ));
        }
    }
    if raw
        .dependencies
        .iter()
        .any(|dependency| !valid_identifier(&dependency.id))
        || raw.conflicts.iter().any(|id| !valid_identifier(id))
    {
        return Err(tag(
            "skill_relationship_invalid",
            "dependency and conflict ids must be safe identifiers".to_string(),
        ));
    }

    let version = match Version::parse(raw.version.trim()) {
        Ok(version) => version,
        Err(error) => {
            return Err(tag(
                "skill_invalid_version",
                format!("version '{}' is not valid semver: {error}", raw.version),
            ))
        }
    };

    for script in &raw.scripts {
        if validate_declared_path(&script.path).is_err() {
            return Err(tag(
                "skill_script_path_invalid",
                format!(
                    "script '{}' path '{}' must be relative and contain no '..'",
                    script.name, script.path
                ),
            ));
        }
    }
    for asset in &raw.assets {
        if validate_declared_path(asset).is_err() {
            return Err(tag(
                "skill_asset_path_invalid",
                format!("asset path '{asset}' must be relative and contain no '..'"),
            ));
        }
    }
    for dependency in &raw.dependencies {
        if VersionReq::parse(dependency.version.trim()).is_err() {
            return Err(tag(
                "skill_dependency_invalid_version",
                format!(
                    "dependency '{}' has invalid version requirement '{}'",
                    dependency.id, dependency.version
                ),
            ));
        }
    }

    let manifest = SkillManifest {
        id: raw.id,
        version,
        description: raw.description,
        parameters: raw.parameters,
        dependencies: raw.dependencies,
        conflicts: raw.conflicts,
        scripts: raw.scripts,
        assets: raw.assets,
        permissions: raw.permissions,
    };

    let md_path = dir.join("SKILL.md");
    let skill_markdown =
        match read_utf8_bounded_within(&md_path, source_root, limits.max_file_bytes) {
            Ok(content) => content,
            Err(error) => {
                return Err(file_issue(
                    "skill_skill_md",
                    &md_path,
                    error,
                    &resource_id,
                    &source_key,
                ))
            }
        };

    Ok(LoadedSkill {
        manifest,
        skill_markdown,
        provenance: ResourceProvenance::new(tier, source_key, origin),
    })
}

fn file_issue(
    prefix: &str,
    path: &Path,
    error: ResourceFileError,
    resource_id: &str,
    source_key: &str,
) -> ResourceIssue {
    let code = match error.code() {
        "resource_not_found" => format!("{prefix}_not_found"),
        "resource_too_large" => format!("{prefix}_too_large"),
        "resource_invalid_utf8" => format!("{prefix}_invalid_utf8"),
        other => other.to_string(),
    };
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("skill resource");
    ResourceIssue::error(code, format!("{file_name}: {error}")).for_resource(
        ResourceKind::Skill,
        resource_id.to_string(),
        source_key.to_string(),
    )
}

/// 声明路径必须非空、相对、且仅由正常分量构成（拒绝 `..`、绝对路径、前缀）。
fn validate_declared_path(raw: &str) -> Result<(), ()> {
    if crate::io::is_safe_relative_reference(raw) {
        Ok(())
    } else {
        Err(())
    }
}

fn valid_identifier(value: &str) -> bool {
    crate::io::is_valid_identifier(value, true)
}

fn parse_req(raw: &str) -> VersionReq {
    VersionReq::parse(raw.trim()).unwrap_or(VersionReq::STAR)
}

/// 依赖遍历结果：`candidates` 为单次 BFS 的可达集（含依赖断裂的技能本身），
/// `outcomes` 记录每条依赖边的判定，供激活收敛与诊断直接复用（不再二次匹配 semver）。
struct DepTraversal {
    candidates: BTreeSet<String>,
    outcomes: BTreeMap<String, Vec<DepEdge>>,
}

/// 一条依赖边的判定结果。
enum DepEdge {
    /// 依赖存在、未排除且版本满足；该边参与 BFS 扩展。
    Valid(String),
    /// 依赖被显式禁用。
    Disabled(String),
    /// 依赖未加载。
    Missing(String),
    /// 依赖已加载但版本不满足。
    VersionMismatch {
        id: String,
        requirement: String,
        actual: Version,
    },
}

/// 从 `seeds` 出发的单次 BFS：跳过被排除/缺失/版本不满足的依赖边，把可达技能
/// 收入 `candidates`，同时记录每条边的判定（供诊断复用，不做第二次 semver 匹配）。
fn traverse_dependencies(
    merged: &BTreeMap<String, LoadedSkill>,
    seeds: &BTreeSet<String>,
    excluded: &BTreeSet<String>,
) -> DepTraversal {
    let mut candidates: BTreeSet<String> = BTreeSet::new();
    let mut outcomes: BTreeMap<String, Vec<DepEdge>> = BTreeMap::new();
    let mut queue: VecDeque<String> = seeds
        .iter()
        .filter(|id| !excluded.contains(*id) && merged.contains_key(*id))
        .cloned()
        .collect();
    while let Some(id) = queue.pop_front() {
        if !candidates.insert(id.clone()) {
            continue;
        }
        let edges: Vec<DepEdge> = merged[&id]
            .manifest
            .dependencies
            .iter()
            .map(|dependency| {
                if excluded.contains(&dependency.id) {
                    DepEdge::Disabled(dependency.id.clone())
                } else if let Some(dep_skill) = merged.get(&dependency.id) {
                    let requirement = parse_req(&dependency.version);
                    if requirement.matches(&dep_skill.manifest.version) {
                        DepEdge::Valid(dependency.id.clone())
                    } else {
                        DepEdge::VersionMismatch {
                            id: dependency.id.clone(),
                            requirement: dependency.version.clone(),
                            actual: dep_skill.manifest.version.clone(),
                        }
                    }
                } else {
                    DepEdge::Missing(dependency.id.clone())
                }
            })
            .collect();
        for edge in &edges {
            if let DepEdge::Valid(dep) = edge {
                if !candidates.contains(dep) {
                    queue.push_back(dep.clone());
                }
            }
        }
        outcomes.insert(id, edges);
    }
    DepTraversal {
        candidates,
        outcomes,
    }
}

/// 由遍历结果收敛激活集：先反向剔除「依赖链断裂」的技能（依赖缺失/被排除/
/// 版本不满足，或递归地依赖这类技能），再从 `seeds` 仅沿完好技能的有效边 BFS。
fn active_from(seeds: &BTreeSet<String>, traversal: &DepTraversal) -> BTreeSet<String> {
    // 1) 坏集：直接断裂的技能，再沿有效依赖边反向传播。
    let mut bad: BTreeSet<String> = traversal
        .outcomes
        .iter()
        .filter(|(_, edges)| edges.iter().any(|edge| !matches!(edge, DepEdge::Valid(_))))
        .map(|(id, _)| id.clone())
        .collect();
    let mut reverse: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (id, edges) in &traversal.outcomes {
        for edge in edges {
            if let DepEdge::Valid(dep) = edge {
                reverse.entry(dep.as_str()).or_default().push(id.as_str());
            }
        }
    }
    let mut queue: VecDeque<String> = bad.iter().cloned().collect();
    while let Some(id) = queue.pop_front() {
        for dependent in reverse.get(id.as_str()).into_iter().flatten() {
            if bad.insert(dependent.to_string()) {
                queue.push_back(dependent.to_string());
            }
        }
    }
    // 2) 从 seeds 出发，仅经「依赖链完好」的技能沿有效边 BFS。
    let mut active: BTreeSet<String> = BTreeSet::new();
    let mut queue: VecDeque<String> = seeds
        .iter()
        .filter(|id| traversal.outcomes.contains_key(*id) && !bad.contains(*id))
        .cloned()
        .collect();
    while let Some(id) = queue.pop_front() {
        if !active.insert(id.clone()) {
            continue;
        }
        for edge in traversal.outcomes.get(&id).into_iter().flatten() {
            if let DepEdge::Valid(dep) = edge {
                if !bad.contains(dep) && !active.contains(dep) {
                    queue.push_back(dep.clone());
                }
            }
        }
    }
    active
}

/// 从遍历记录的边判定生成依赖诊断（复用 BFS 的匹配结果，不做第二次 semver 匹配）。
fn dep_issues(
    merged: &BTreeMap<String, LoadedSkill>,
    traversal: &DepTraversal,
) -> Vec<ResourceIssue> {
    let mut issues = Vec::new();
    for id in &traversal.candidates {
        let Some(edges) = traversal.outcomes.get(id) else {
            continue;
        };
        let source = merged[id].provenance.source_key.clone();
        for edge in edges {
            let issue = match edge {
                DepEdge::Disabled(dep) => ResourceIssue::error(
                    "skill_dependency_disabled",
                    format!("skill '{id}' depends on disabled skill '{dep}'"),
                ),
                DepEdge::Missing(dep) => ResourceIssue::error(
                    "skill_dependency_missing",
                    format!("skill '{id}' depends on missing skill '{dep}'"),
                ),
                DepEdge::VersionMismatch {
                    id: dep,
                    requirement,
                    actual,
                } => ResourceIssue::error(
                    "skill_dependency_version",
                    format!("skill '{id}' requires '{dep} {requirement}' but {actual} is loaded"),
                ),
                DepEdge::Valid(_) => continue,
            };
            issues.push(issue.for_resource(ResourceKind::Skill, id.clone(), source.clone()));
        }
    }
    issues
}

fn detect_conflicts(
    merged: &BTreeMap<String, LoadedSkill>,
    active: &BTreeSet<String>,
) -> (BTreeSet<String>, Vec<ResourceIssue>) {
    let ordered: Vec<&String> = active.iter().collect();
    let mut conflict_ids: BTreeSet<String> = BTreeSet::new();
    let mut issues = Vec::new();
    for (i, left) in ordered.iter().enumerate() {
        for right in ordered.iter().skip(i + 1) {
            let left_skill = &merged[*left];
            let right_skill = &merged[*right];
            let forward = left_skill
                .manifest
                .conflicts
                .iter()
                .any(|conflict| conflict == *right);
            let reverse = right_skill
                .manifest
                .conflicts
                .iter()
                .any(|conflict| conflict == *left);
            if forward || reverse {
                conflict_ids.insert((*left).clone());
                conflict_ids.insert((*right).clone());
                issues.push(
                    ResourceIssue::error(
                        "skill_conflict",
                        format!("skill '{left}' conflicts with skill '{right}'"),
                    )
                    .for_resource(
                        ResourceKind::Skill,
                        (*left).clone(),
                        left_skill.provenance.source_key.clone(),
                    ),
                );
            }
        }
    }
    (conflict_ids, issues)
}

fn status_for(
    id: &str,
    active: &BTreeSet<String>,
    disabled: &BTreeSet<String>,
    candidates: &BTreeSet<String>,
) -> ResourceDiagnosticStatus {
    if disabled.contains(id) {
        ResourceDiagnosticStatus::Disabled
    } else if active.contains(id) {
        ResourceDiagnosticStatus::Active
    } else if candidates.contains(id) {
        ResourceDiagnosticStatus::Rejected
    } else {
        ResourceDiagnosticStatus::Loaded
    }
}

fn make_entry(skill: &LoadedSkill, status: ResourceDiagnosticStatus) -> ResourceDiagnosticEntry {
    ResourceDiagnosticEntry {
        kind: ResourceKind::Skill,
        resource_id: skill.manifest.id.clone(),
        status,
        provenance: skill.provenance.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::ResourceIssueSeverity;
    use std::fs;
    use tempfile::tempdir;

    fn limits() -> ResourceLimits {
        ResourceLimits::default()
    }

    fn selection(active: &[&str], disabled: &[&str]) -> ResourceSelection {
        ResourceSelection {
            active_skills: active.iter().map(|value| (*value).to_string()).collect(),
            disabled_skills: disabled.iter().map(|value| (*value).to_string()).collect(),
            ..Default::default()
        }
    }

    fn global_skills_dir(global: &Path) -> PathBuf {
        global.join("skills")
    }

    fn workspace_skills_dir(root: &Path) -> PathBuf {
        root.join(".pawork").join("skills")
    }

    fn write_skill(skills_dir: &Path, name: &str, manifest: &str) {
        let dir = skills_dir.join(name);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("manifest.toml"), manifest).unwrap();
        fs::write(dir.join("SKILL.md"), format!("# {name}")).unwrap();
    }

    fn active_ids(res: &SkillResolution) -> Vec<String> {
        res.skills
            .iter()
            .map(|skill| skill.manifest.id.clone())
            .collect()
    }

    fn statuses_for(res: &SkillResolution, id: &str) -> Vec<ResourceDiagnosticStatus> {
        res.diagnostics
            .entries
            .iter()
            .filter(|entry| entry.resource_id == id)
            .map(|entry| entry.status)
            .collect()
    }

    fn has_issue(res: &SkillResolution, code: &str) -> bool {
        res.diagnostics
            .issues
            .iter()
            .any(|issue| issue.code == code)
    }

    #[test]
    fn loads_and_activates_global_skill() {
        let global = tempdir().unwrap();
        let skills_dir = global_skills_dir(global.path());
        write_skill(
            &skills_dir,
            "rust-review",
            r#"id = "rust-review"
version = "1.0.0"
description = "Review Rust"
"#,
        );

        let res = load_skills(
            Some(global.path()),
            &[],
            ".pawork",
            &selection(&["rust-review"], &[]),
            &limits(),
        );

        assert_eq!(active_ids(&res), vec!["rust-review".to_string()]);
        assert_eq!(res.skills[0].manifest.version, Version::new(1, 0, 0));
        assert!(statuses_for(&res, "rust-review").contains(&ResourceDiagnosticStatus::Active));
        assert!(res.diagnostics.issues.is_empty());
    }

    #[test]
    fn workspace_overrides_global() {
        let global = tempdir().unwrap();
        let workspace = tempdir().unwrap();
        write_skill(
            &global_skills_dir(global.path()),
            "shared",
            r#"id = "shared"
version = "1.0.0"
description = "global"
"#,
        );
        write_skill(
            &workspace_skills_dir(workspace.path()),
            "shared",
            r#"id = "shared"
version = "2.0.0"
description = "workspace"
"#,
        );

        let res = load_skills(
            Some(global.path()),
            &[workspace.path().to_path_buf()],
            ".pawork",
            &selection(&["shared"], &[]),
            &limits(),
        );

        assert_eq!(active_ids(&res), vec!["shared".to_string()]);
        assert_eq!(res.skills[0].manifest.version, Version::new(2, 0, 0));
        let mut seen = Vec::new();
        for entry in &res.diagnostics.entries {
            if entry.resource_id == "shared" {
                seen.push((entry.provenance.tier, entry.status));
            }
        }
        assert!(seen.contains(&(ConfigTier::Global, ResourceDiagnosticStatus::Overridden)));
        assert!(seen.contains(&(ConfigTier::Workspace, ResourceDiagnosticStatus::Active)));
    }

    #[test]
    fn same_tier_duplicate_warns_and_keeps_one() {
        let global = tempdir().unwrap();
        let skills_dir = global_skills_dir(global.path());
        write_skill(
            &skills_dir,
            "dup-a",
            r#"id = "dup"
version = "1.0.0"
"#,
        );
        write_skill(
            &skills_dir,
            "dup-b",
            r#"id = "dup"
version = "2.0.0"
"#,
        );

        let res = load_skills(
            Some(global.path()),
            &[],
            ".pawork",
            &selection(&["dup"], &[]),
            &limits(),
        );

        assert_eq!(active_ids(&res), vec!["dup".to_string()]);
        assert_eq!(res.skills[0].manifest.version, Version::new(2, 0, 0));
        assert!(res.diagnostics.issues.iter().any(|issue| {
            issue.code == "skill_duplicate" && issue.severity == ResourceIssueSeverity::Warning
        }));
    }

    #[test]
    fn transitive_dependency_activation() {
        let global = tempdir().unwrap();
        let skills_dir = global_skills_dir(global.path());
        write_skill(
            &skills_dir,
            "a",
            r#"id = "a"
version = "1.0.0"
[[dependencies]]
id = "b"
version = "*"
"#,
        );
        write_skill(
            &skills_dir,
            "b",
            r#"id = "b"
version = "1.0.0"
[[dependencies]]
id = "c"
version = "^1.0"
"#,
        );
        write_skill(
            &skills_dir,
            "c",
            r#"id = "c"
version = "1.2.0"
"#,
        );

        let res = load_skills(
            Some(global.path()),
            &[],
            ".pawork",
            &selection(&["a"], &[]),
            &limits(),
        );

        assert_eq!(
            active_ids(&res),
            vec!["a".to_string(), "b".to_string(), "c".to_string()]
        );
        assert!(res.diagnostics.issues.is_empty());
    }

    #[test]
    fn disabled_takes_priority_over_active() {
        let global = tempdir().unwrap();
        let skills_dir = global_skills_dir(global.path());
        write_skill(
            &skills_dir,
            "x",
            r#"id = "x"
version = "1.0.0"
"#,
        );

        let res = load_skills(
            Some(global.path()),
            &[],
            ".pawork",
            &selection(&["x"], &["x"]),
            &limits(),
        );

        assert!(res.skills.is_empty());
        assert!(statuses_for(&res, "x").contains(&ResourceDiagnosticStatus::Disabled));
    }

    #[test]
    fn dependency_disabled_rejects_dependent() {
        let global = tempdir().unwrap();
        let skills_dir = global_skills_dir(global.path());
        write_skill(
            &skills_dir,
            "a",
            r#"id = "a"
version = "1.0.0"
[[dependencies]]
id = "b"
version = "*"
"#,
        );
        write_skill(
            &skills_dir,
            "b",
            r#"id = "b"
version = "1.0.0"
"#,
        );

        let res = load_skills(
            Some(global.path()),
            &[],
            ".pawork",
            &selection(&["a"], &["b"]),
            &limits(),
        );

        assert!(res.skills.is_empty());
        assert!(statuses_for(&res, "a").contains(&ResourceDiagnosticStatus::Rejected));
        assert!(statuses_for(&res, "b").contains(&ResourceDiagnosticStatus::Disabled));
        assert!(has_issue(&res, "skill_dependency_disabled"));
    }

    #[test]
    fn missing_dependency_rejects_dependent() {
        let global = tempdir().unwrap();
        let skills_dir = global_skills_dir(global.path());
        write_skill(
            &skills_dir,
            "a",
            r#"id = "a"
version = "1.0.0"
[[dependencies]]
id = "ghost"
version = "*"
"#,
        );

        let res = load_skills(
            Some(global.path()),
            &[],
            ".pawork",
            &selection(&["a"], &[]),
            &limits(),
        );

        assert!(res.skills.is_empty());
        assert!(statuses_for(&res, "a").contains(&ResourceDiagnosticStatus::Rejected));
        assert!(has_issue(&res, "skill_dependency_missing"));
    }

    #[test]
    fn version_mismatch_rejects_and_leaves_dependency_loaded() {
        let global = tempdir().unwrap();
        let skills_dir = global_skills_dir(global.path());
        write_skill(
            &skills_dir,
            "a",
            r#"id = "a"
version = "1.0.0"
[[dependencies]]
id = "b"
version = "^2.0"
"#,
        );
        write_skill(
            &skills_dir,
            "b",
            r#"id = "b"
version = "1.5.0"
"#,
        );

        let res = load_skills(
            Some(global.path()),
            &[],
            ".pawork",
            &selection(&["a"], &[]),
            &limits(),
        );

        assert!(res.skills.is_empty());
        assert!(statuses_for(&res, "a").contains(&ResourceDiagnosticStatus::Rejected));
        assert!(statuses_for(&res, "b").contains(&ResourceDiagnosticStatus::Loaded));
        assert!(has_issue(&res, "skill_dependency_version"));
    }

    #[test]
    fn cascade_rejection_when_inner_dependency_missing() {
        let global = tempdir().unwrap();
        let skills_dir = global_skills_dir(global.path());
        write_skill(
            &skills_dir,
            "a",
            r#"id = "a"
version = "1.0.0"
[[dependencies]]
id = "b"
version = "*"
"#,
        );
        write_skill(
            &skills_dir,
            "b",
            r#"id = "b"
version = "1.0.0"
[[dependencies]]
id = "c"
version = "*"
"#,
        );

        let res = load_skills(
            Some(global.path()),
            &[],
            ".pawork",
            &selection(&["a"], &[]),
            &limits(),
        );

        assert!(res.skills.is_empty());
        assert!(statuses_for(&res, "a").contains(&ResourceDiagnosticStatus::Rejected));
        assert!(statuses_for(&res, "b").contains(&ResourceDiagnosticStatus::Rejected));
        assert!(has_issue(&res, "skill_dependency_missing"));
    }

    #[test]
    fn bidirectional_conflict_either_direction() {
        let global = tempdir().unwrap();
        let skills_dir = global_skills_dir(global.path());
        write_skill(
            &skills_dir,
            "a",
            r#"id = "a"
version = "1.0.0"
"#,
        );
        write_skill(
            &skills_dir,
            "b",
            r#"id = "b"
version = "1.0.0"
conflicts = ["a"]
"#,
        );

        let res = load_skills(
            Some(global.path()),
            &[],
            ".pawork",
            &selection(&["a", "b"], &[]),
            &limits(),
        );

        assert!(res.skills.is_empty());
        assert!(statuses_for(&res, "a").contains(&ResourceDiagnosticStatus::Rejected));
        assert!(statuses_for(&res, "b").contains(&ResourceDiagnosticStatus::Rejected));
        assert_eq!(
            res.diagnostics
                .issues
                .iter()
                .filter(|issue| issue.code == "skill_conflict")
                .count(),
            1
        );
    }

    #[test]
    fn invalid_script_path_rejects_skill() {
        let global = tempdir().unwrap();
        let skills_dir = global_skills_dir(global.path());
        write_skill(
            &skills_dir,
            "a",
            r#"id = "a"
version = "1.0.0"
[[scripts]]
name = "evil"
path = "../escape.sh"
"#,
        );

        let res = load_skills(
            Some(global.path()),
            &[],
            ".pawork",
            &selection(&["a"], &[]),
            &limits(),
        );

        assert!(res.skills.is_empty());
        assert!(has_issue(&res, "skill_script_path_invalid"));
    }

    #[test]
    fn invalid_asset_path_rejects_skill() {
        let global = tempdir().unwrap();
        let skills_dir = global_skills_dir(global.path());
        write_skill(
            &skills_dir,
            "a",
            r#"id = "a"
version = "1.0.0"
assets = ["/etc/passwd"]
"#,
        );

        let res = load_skills(
            Some(global.path()),
            &[],
            ".pawork",
            &selection(&["a"], &[]),
            &limits(),
        );

        assert!(res.skills.is_empty());
        assert!(has_issue(&res, "skill_asset_path_invalid"));
    }

    #[test]
    fn invalid_manifest_version_rejects_skill() {
        let global = tempdir().unwrap();
        let skills_dir = global_skills_dir(global.path());
        write_skill(
            &skills_dir,
            "a",
            r#"id = "a"
version = "not-a-version"
"#,
        );

        let res = load_skills(
            Some(global.path()),
            &[],
            ".pawork",
            &selection(&["a"], &[]),
            &limits(),
        );

        assert!(res.skills.is_empty());
        assert!(has_issue(&res, "skill_invalid_version"));
    }

    #[test]
    fn unsafe_manifest_identifier_is_rejected() {
        let global = tempdir().unwrap();
        let skills_dir = global_skills_dir(global.path());
        write_skill(&skills_dir, "bad", "id = 'bad id'\nversion = '1.0.0'\n");

        let res = load_skills(
            Some(global.path()),
            &[],
            ".pawork",
            &selection(&["bad id"], &[]),
            &limits(),
        );

        assert!(res.skills.is_empty());
        assert!(has_issue(&res, "skill_manifest_id_invalid"));
    }

    #[test]
    fn missing_skill_md_rejects_skill() {
        let global = tempdir().unwrap();
        let dir = global_skills_dir(global.path()).join("a");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("manifest.toml"),
            r#"id = "a"
version = "1.0.0"
"#,
        )
        .unwrap();

        let res = load_skills(
            Some(global.path()),
            &[],
            ".pawork",
            &selection(&["a"], &[]),
            &limits(),
        );

        assert!(res.skills.is_empty());
        assert!(has_issue(&res, "skill_skill_md_not_found"));
    }

    #[test]
    fn missing_manifest_rejects_skill() {
        let global = tempdir().unwrap();
        let dir = global_skills_dir(global.path()).join("a");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("SKILL.md"), "# a").unwrap();

        let res = load_skills(
            Some(global.path()),
            &[],
            ".pawork",
            &selection(&["a"], &[]),
            &limits(),
        );

        assert!(res.skills.is_empty());
        assert!(has_issue(&res, "skill_manifest_not_found"));
        let serialized = serde_json::to_string(&res.diagnostics).expect("serialize diagnostics");
        assert!(!serialized.contains(&global.path().to_string_lossy().into_owned()));
        assert!(serialized.contains("global:skill:skills/a/manifest.toml"));
    }

    #[test]
    fn invalid_dependency_requirement_rejects_skill() {
        let global = tempdir().unwrap();
        let skills_dir = global_skills_dir(global.path());
        write_skill(
            &skills_dir,
            "a",
            r#"id = "a"
version = "1.0.0"
[[dependencies]]
id = "b"
version = "not a req"
"#,
        );

        let res = load_skills(
            Some(global.path()),
            &[],
            ".pawork",
            &selection(&["a"], &[]),
            &limits(),
        );

        assert!(res.skills.is_empty());
        assert!(has_issue(&res, "skill_dependency_invalid_version"));
    }

    #[test]
    fn parameters_and_scripts_are_parsed() {
        let global = tempdir().unwrap();
        let skills_dir = global_skills_dir(global.path());
        write_skill(
            &skills_dir,
            "a",
            r#"id = "a"
version = "1.0.0"
[[parameters]]
name = "depth"
description = "review depth"
required = true
default = "shallow"
[[scripts]]
name = "run"
path = "scripts/run.sh"
arguments = ["--fast"]
"#,
        );

        let res = load_skills(
            Some(global.path()),
            &[],
            ".pawork",
            &selection(&["a"], &[]),
            &limits(),
        );

        let manifest = &res.skills[0].manifest;
        assert_eq!(manifest.parameters.len(), 1);
        assert_eq!(manifest.parameters[0].name, "depth");
        assert!(manifest.parameters[0].required);
        assert_eq!(manifest.parameters[0].default.as_deref(), Some("shallow"));
        assert_eq!(manifest.scripts[0].path, "scripts/run.sh");
        assert_eq!(manifest.scripts[0].arguments, vec!["--fast".to_string()]);
    }

    #[test]
    fn output_is_deterministic_regardless_of_scan_order() {
        let global = tempdir().unwrap();
        let skills_dir = global_skills_dir(global.path());
        for name in ["c", "a", "b"] {
            write_skill(
                &skills_dir,
                name,
                &format!("id = \"{name}\"\nversion = \"1.0.0\"\n"),
            );
        }

        let first = load_skills(
            Some(global.path()),
            &[],
            ".pawork",
            &selection(&["a", "b", "c"], &[]),
            &limits(),
        );
        let second = load_skills(
            Some(global.path()),
            &[],
            ".pawork",
            &selection(&["a", "b", "c"], &[]),
            &limits(),
        );

        assert_eq!(
            active_ids(&first),
            vec!["a".to_string(), "b".to_string(), "c".to_string()]
        );
        assert_eq!(first, second);
    }

    #[test]
    fn active_and_disabled_unknown_ids_warn() {
        let global = tempdir().unwrap();

        let res = load_skills(
            Some(global.path()),
            &[],
            ".pawork",
            &selection(&["ghost"], &["phantom"]),
            &limits(),
        );

        assert!(res.skills.is_empty());
        assert!(has_issue(&res, "skill_active_unknown"));
        assert!(has_issue(&res, "skill_disabled_unknown"));
    }

    #[test]
    fn no_global_dir_and_empty_workspace_is_clean() {
        let res = load_skills(None, &[], ".pawork", &selection(&[], &[]), &limits());

        assert!(res.skills.is_empty());
        assert!(res.diagnostics.entries.is_empty());
        assert!(res.diagnostics.issues.is_empty());
    }
}
