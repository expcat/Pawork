//! 跨类型 / 跨包冲突检测（P17-2）。
//!
//! 加载多个 package 时把六类资源合并到统一资源表，检测：
//! - 同 scope 内重名 skill（路径冲突）、重复 MCP server 名、重复 monitor_id、
//!   重复 LSP id、重复 profile name；
//! - 同 scope 内重复 hook trigger+scope 组合；
//! - 同 package id 不同版本（跨包）。
//!
//! 冲突以 [`ConflictIssue`] 列表回报；严重级别为 Error 时视为不可安装。本检测只看
//! 声明性内容（manifest + 已校验归档内子 manifest），不读取运行时。path 引用先
//! 解析子 manifest 的 id / name / trigger 作为稳定键，再按 id/name 做跨包冲突
//! 检测——两个 package 即使路径不同，只要资源身份相同即冲突；inline 引用与
//! path 引用解析后使用同一键空间。
//!
//! # 安全模型（2026-08 安全复审）
//!
//! [`LoadedPackage`] **强制消费已验证的 [`PackageArchive`]**（类型层保证「无验证
//! 上下文」不可构造）；子 manifest 一律经 [`PackageArchive::read_file`] 安全读取
//! （逐级 no-follow + blake3 复核）。坏子 manifest（不可解析 / 缺身份字段）与
//! symlink 替换不再静默回退到路径键，一律 fail-closed 返回错误。
//! package 级身份（package id / scope）同样只取自已验证 manifest；
//! [`PackageProvenance`] 仅用于诊断排序，不参与任何身份判定。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::archive::PackageArchive;
use crate::error::PackageError;
use crate::manifest::PackageManifest;
use crate::scope::{PackageProvenance, PackageRelativePath, PackageScope};

/// 已加载、待做冲突检测的 package 视图（manifest + 来源）。
#[derive(Clone, Debug)]
pub struct LoadedPackage {
    pub provenance: PackageProvenance,
    /// 已校验的归档（manifest + 内容条目 + 根句柄）。path 引用的子 manifest
    /// 一律经 [`PackageArchive::read_file`] 安全读取；无归档的 package 无法构造
    /// 本类型（无验证上下文 fail-closed）。
    archive: PackageArchive,
}

impl LoadedPackage {
    /// 从已校验归档构造。`provenance` 应与归档 manifest 的 id / version / scope
    /// 一致（来源记录仅用于诊断排序，不参与身份判定）。
    pub fn new(provenance: PackageProvenance, archive: PackageArchive) -> Self {
        Self {
            provenance,
            archive,
        }
    }

    /// 该 package 的 manifest（已随归档验证）。
    pub fn manifest(&self) -> &PackageManifest {
        &self.archive.manifest
    }

    fn scope_key(&self) -> String {
        // 作用域同样以已验证 manifest 为准（provenance.scope 仅诊断）。
        scope_key(&self.manifest().scope)
    }
}

/// 冲突作用域：global 或单个 workspace。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConflictScope {
    Global,
    Workspace,
}

/// 冲突类别。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConflictKind {
    Skill,
    AgentProfile,
    HookTrigger,
    McpServer,
    LanguageServer,
    Monitor,
    PackageId,
}

/// 单条冲突。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConflictIssue {
    pub severity: ConflictSeverity,
    pub kind: ConflictKind,
    pub scope: ConflictScope,
    /// 冲突的稳定键（skill 路径 / mcp server 名 / monitor id 等）。
    pub key: String,
    /// 卷入冲突的 package source keys。
    pub packages: Vec<String>,
    pub message: String,
}

/// 冲突严重级别。当前所有检测到的冲突均为 Error（不可安装）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConflictSeverity {
    Warning,
    Error,
}

/// 冲突检测报告。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConflictReport {
    pub issues: Vec<ConflictIssue>,
}

impl ConflictReport {
    /// 是否含任一 Error 级冲突（不可安装）。
    pub fn has_blocking(&self) -> bool {
        self.issues
            .iter()
            .any(|issue| issue.severity == ConflictSeverity::Error)
    }
}

/// 检测一组已加载 package 之间的冲突。
///
/// 任一 path 引用的子 manifest 无法经安全读取解析（缺失 / 损坏 / symlink 替换 /
/// 摘要不匹配 / 缺身份字段）时返回错误（fail-closed），不静默降级。
pub fn detect_conflicts(packages: &[LoadedPackage]) -> Result<ConflictReport, PackageError> {
    let mut report = ConflictReport::default();
    detect_package_id_conflicts(packages, &mut report);
    detect_typed_conflicts(packages, &mut report)?;
    report.issues.sort_by(|left, right| {
        left.kind
            .cmp(&right.kind)
            .then_with(|| left.key.cmp(&right.key))
    });
    Ok(report)
}

fn detect_package_id_conflicts(packages: &[LoadedPackage], report: &mut ConflictReport) {
    // 同 package id 的不同版本视为冲突（跨包重复安装）。身份一律取自已验证归档
    // manifest（内容寻址 + blake3 复核）；provenance 仅诊断，不参与身份判定。
    let mut by_id: BTreeMap<&str, Vec<&LoadedPackage>> = BTreeMap::new();
    for package in packages {
        by_id
            .entry(package.manifest().id.as_str())
            .or_default()
            .push(package);
    }
    for (id, group) in by_id {
        if group.len() > 1 {
            let packages = group
                .iter()
                .map(|item| item.provenance.source_key.clone())
                .collect::<Vec<_>>();
            report.issues.push(ConflictIssue {
                severity: ConflictSeverity::Error,
                kind: ConflictKind::PackageId,
                scope: ConflictScope::Global,
                key: id.to_string(),
                packages,
                message: format!("package id `{id}` is declared by multiple packages"),
            });
        }
    }
}

fn detect_typed_conflicts(
    packages: &[LoadedPackage],
    report: &mut ConflictReport,
) -> Result<(), PackageError> {
    // 按 (scope_key, kind, resource_key) 聚合；同名即冲突。
    let mut buckets: BTreeMap<(String, KeyedKind, String), Vec<String>> = BTreeMap::new();
    for package in packages {
        let scope = package.scope_key();
        for (kind, key) in resource_keys(package)? {
            buckets
                .entry((scope.clone(), kind, key))
                .or_default()
                .push(package.provenance.source_key.clone());
        }
    }
    for ((scope, kind, key), owners) in buckets {
        if owners.len() > 1 {
            let conflict_scope = if scope == GLOBAL_SCOPE {
                ConflictScope::Global
            } else {
                ConflictScope::Workspace
            };
            report.issues.push(ConflictIssue {
                severity: ConflictSeverity::Error,
                kind: kind.to_conflict_kind(),
                scope: conflict_scope,
                key: key.clone(),
                packages: owners,
                message: format!(
                    "{kind_desc} `{key}` collides within scope `{scope}`",
                    kind_desc = kind.description()
                ),
            });
        }
    }
    Ok(())
}

const GLOBAL_SCOPE: &str = "global";

fn scope_key(scope: &PackageScope) -> String {
    match scope {
        PackageScope::Global => GLOBAL_SCOPE.to_string(),
        PackageScope::Workspace { workspace_id } => {
            format!("workspace:{}", workspace_id.as_str())
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum KeyedKind {
    Skill,
    AgentProfile,
    HookTrigger,
    McpServer,
    LanguageServer,
    Monitor,
}

impl KeyedKind {
    fn to_conflict_kind(self) -> ConflictKind {
        match self {
            Self::Skill => ConflictKind::Skill,
            Self::AgentProfile => ConflictKind::AgentProfile,
            Self::HookTrigger => ConflictKind::HookTrigger,
            Self::McpServer => ConflictKind::McpServer,
            Self::LanguageServer => ConflictKind::LanguageServer,
            Self::Monitor => ConflictKind::Monitor,
        }
    }

    fn description(self) -> &'static str {
        match self {
            Self::Skill => "skill",
            Self::AgentProfile => "agent profile",
            Self::HookTrigger => "hook trigger",
            Self::McpServer => "mcp server",
            Self::LanguageServer => "language server",
            Self::Monitor => "monitor",
        }
    }
}

/// 枚举一个 package 在其 scope 内声明的所有稳定资源键。
fn resource_keys(package: &LoadedPackage) -> Result<Vec<(KeyedKind, String)>, PackageError> {
    let manifest = package.manifest();
    let mut keys = Vec::new();
    for skill in &manifest.skills {
        let path = skill.path().ok_or_else(|| {
            PackageError::field(
                "skills",
                "skill entries must use a path reference (skill directory)",
            )
        })?;
        push_path_keys(
            package,
            KeyedKind::Skill,
            path,
            Some("manifest.toml"),
            &["id"],
            &mut keys,
        )?;
    }
    for agent in &manifest.agents {
        if let Some(path) = agent.path() {
            push_path_keys(
                package,
                KeyedKind::AgentProfile,
                path,
                None,
                &["name"],
                &mut keys,
            )?;
        } else if let Some(inline) = agent.inline() {
            let name = inline
                .get("name")
                .and_then(|value| value.as_str())
                .ok_or_else(|| {
                    PackageError::field(
                        "agents",
                        "inline agent manifest must include a string `name` field",
                    )
                })?;
            keys.push((KeyedKind::AgentProfile, name.to_string()));
        }
    }
    for hook in &manifest.hooks {
        // hook 冲突键 = trigger（+ 可选 lifecycle）；同 trigger 重复触发即冲突。
        if let Some(inline) = hook.inline() {
            let trigger = inline
                .get("trigger")
                .and_then(|value| value.as_str())
                .ok_or_else(|| {
                    PackageError::field(
                        "hooks",
                        "inline hook manifest must include a string `trigger` field",
                    )
                })?;
            keys.push((KeyedKind::HookTrigger, trigger.to_string()));
        } else if let Some(path) = hook.path() {
            push_path_keys(
                package,
                KeyedKind::HookTrigger,
                path,
                None,
                &["trigger"],
                &mut keys,
            )?;
        }
    }
    for server in &manifest.mcp {
        keys.push((KeyedKind::McpServer, server.name.clone()));
    }
    for lsp in &manifest.lsp {
        if let Some(inline) = lsp.inline() {
            let id = inline
                .get("id")
                .and_then(|value| value.as_str())
                .ok_or_else(|| {
                    PackageError::field(
                        "lsp",
                        "inline lsp manifest must include a string `id` field",
                    )
                })?;
            keys.push((KeyedKind::LanguageServer, id.to_string()));
        } else if let Some(path) = lsp.path() {
            push_path_keys(
                package,
                KeyedKind::LanguageServer,
                path,
                None,
                &["id"],
                &mut keys,
            )?;
        }
    }
    for monitor in &manifest.monitors {
        keys.push((KeyedKind::Monitor, monitor.monitor_id.as_str().to_string()));
    }
    Ok(keys)
}

/// 解析 path 引用并加入冲突键：子 manifest 的身份字段解析失败（缺失、不可解析、
/// 缺字段、symlink 替换、摘要不匹配）一律 fail-closed，不再静默回退到路径键。
/// 解析成功后同时保留路径键（同路径即冲突的既有行为）。
fn push_path_keys(
    package: &LoadedPackage,
    kind: KeyedKind,
    path: &PackageRelativePath,
    sub_file: Option<&str>,
    fields: &[&str],
    keys: &mut Vec<(KeyedKind, String)>,
) -> Result<(), PackageError> {
    let identity = resolve_path_identity(&package.archive, path, sub_file, fields)?;
    keys.push((kind, identity));
    keys.push((kind, path.to_posix_string()));
    Ok(())
}

/// 经已验证归档的安全读取接口读取子 manifest 并解析身份字段（skill 目录读
/// `manifest.toml`，其余读引用文件本身；TOML 优先，JSON 兜底）。读取经
/// [`PackageArchive::read_file`]（逐级 no-follow + blake3 复核），解析失败或
/// 身份字段缺失一律返回错误（fail-closed）。
fn resolve_path_identity(
    archive: &PackageArchive,
    path: &PackageRelativePath,
    sub_file: Option<&str>,
    fields: &[&str],
) -> Result<String, PackageError> {
    let file_path = match sub_file {
        Some(name) => PackageRelativePath::new(path.as_path().join(name))?,
        None => path.clone(),
    };
    let bytes = archive.read_file(&file_path)?;
    let text = std::str::from_utf8(&bytes).map_err(|_| {
        PackageError::field(
            format!("sub-manifest `{}`", file_path.to_posix_string()),
            "must be valid UTF-8",
        )
    })?;
    if let Ok(table) = toml::from_str::<toml::Value>(text) {
        if let Some(value) = first_string_field(&table, fields) {
            return Ok(value);
        }
        return Err(missing_identity_error(&file_path, fields));
    }
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(text) {
        if let Some(found) = first_json_string_field(&value, fields) {
            return Ok(found);
        }
        return Err(missing_identity_error(&file_path, fields));
    }
    Err(PackageError::field(
        format!("sub-manifest `{}`", file_path.to_posix_string()),
        "must be valid TOML or JSON",
    ))
}

fn missing_identity_error(path: &PackageRelativePath, fields: &[&str]) -> PackageError {
    PackageError::field(
        format!("sub-manifest `{}`", path.to_posix_string()),
        format!("is missing required identity field `{}`", fields[0]),
    )
}

fn first_string_field(table: &toml::Value, fields: &[&str]) -> Option<String> {
    fields.iter().find_map(|field| {
        table
            .get(field)
            .and_then(toml::Value::as_str)
            .map(str::to_string)
    })
}

fn first_json_string_field(value: &serde_json::Value, fields: &[&str]) -> Option<String> {
    fields.iter().find_map(|field| {
        value
            .get(field)
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
    })
}

/// 把冲突报告中的 Error 转为 [`PackageError::Conflict`]（用于安装前置校验）。
pub fn blocking_error(report: &ConflictReport) -> Option<PackageError> {
    report
        .issues
        .iter()
        .find(|issue| issue.severity == ConflictSeverity::Error)
        .map(|issue| PackageError::Conflict(issue.message.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive::{read_archive, write_archive};
    use crate::manifest::{McpServerDeclaration, McpTransportSpec};
    use crate::monitor::{MonitorDeclaration, MonitorDriverEntry, MonitorLifecycle};
    use crate::scope::PackageId;
    use crate::scope::PackageRelativePath;
    use agent_domain::MonitorId;
    use semver::Version;
    use std::fs;

    fn provenance(id: &str) -> PackageProvenance {
        PackageProvenance::new(
            PackageId::new(id).unwrap(),
            Version::new(1, 0, 0),
            PackageScope::Global,
        )
    }

    fn empty_manifest(id: &str) -> PackageManifest {
        PackageManifest {
            manifest_version: crate::PACKAGE_MANIFEST_VERSION,
            id: PackageId::new(id).unwrap(),
            name: id.into(),
            version: Version::new(1, 0, 0),
            license: None,
            description: None,
            entrypoint: None,
            scope: PackageScope::Global,
            dependencies: Vec::new(),
            skills: Vec::new(),
            agents: Vec::new(),
            hooks: Vec::new(),
            mcp: Vec::new(),
            lsp: Vec::new(),
            monitors: Vec::new(),
        }
    }

    /// 把 manifest 写入真实归档并构造 LoadedPackage；返回的 TempDir 必须存活到
    /// 冲突检测结束（归档根句柄生命周期）。
    fn loaded(
        provenance: PackageProvenance,
        manifest: PackageManifest,
    ) -> (tempfile::TempDir, LoadedPackage) {
        let temp = tempfile::tempdir().unwrap();
        write_archive(temp.path(), &manifest).expect("write archive");
        let archive = read_archive(temp.path()).expect("read archive");
        let package = LoadedPackage::new(provenance, archive);
        (temp, package)
    }

    /// 在归档根下创建 skill 目录形态资源（manifest.toml + SKILL.md）。
    fn add_skill(root: &std::path::Path, dir: &str) {
        fs::create_dir_all(root.join(dir)).unwrap();
        fs::write(
            root.join(dir).join("manifest.toml"),
            "id='search'\nversion='1.0.0'",
        )
        .unwrap();
        fs::write(root.join(dir).join("SKILL.md"), "# Search").unwrap();
    }

    #[test]
    fn detects_mcp_server_name_collision() {
        let mut a = empty_manifest("acme.a");
        a.mcp.push(McpServerDeclaration {
            name: "fs".into(),
            transport: McpTransportSpec::Stdio {
                command: "npx".into(),
                args: Vec::new(),
                env: Default::default(),
            },
            auto_start: false,
        });
        let mut b = empty_manifest("acme.b");
        b.mcp.push(McpServerDeclaration {
            name: "fs".into(),
            transport: McpTransportSpec::Http {
                url: "https://example.com".into(),
                headers: Default::default(),
            },
            auto_start: false,
        });
        let (_temp_a, a) = loaded(provenance("acme.a"), a);
        let (_temp_b, b) = loaded(provenance("acme.b"), b);
        let report = detect_conflicts(&[a, b]).expect("detect");
        assert!(report.has_blocking());
        assert!(report
            .issues
            .iter()
            .any(|issue| issue.kind == ConflictKind::McpServer && issue.key == "fs"));
        assert!(blocking_error(&report).is_some());
    }

    #[test]
    fn detects_duplicate_monitor_id_and_skill_path() {
        let mut a = empty_manifest("acme.a");
        a.skills.push(crate::manifest::ResourceRef::Path {
            path: PackageRelativePath::new("skills/search").unwrap(),
        });
        a.monitors.push({
            let mut decl = MonitorDeclaration::new(
                MonitorId::new("watch"),
                MonitorDriverEntry::new("monitor_service.evaluate"),
                MonitorLifecycle::TaskManager,
            );
            decl.config = serde_json::json!({"kind": "file_change", "paths": ["target/debug/app"]});
            decl.source = agent_domain::MonitorSourceKind::FileChange;
            decl
        });
        let mut b = empty_manifest("acme.b");
        b.skills.push(crate::manifest::ResourceRef::Path {
            path: PackageRelativePath::new("skills/search").unwrap(),
        });
        // 两个归档都需要真实的 skill 目录内容（skill path 引用经安全读取解析）。
        let temp_a = tempfile::tempdir().unwrap();
        let temp_b = tempfile::tempdir().unwrap();
        add_skill(temp_a.path(), "skills/search");
        add_skill(temp_b.path(), "skills/search");
        write_archive(temp_a.path(), &a).expect("write a");
        write_archive(temp_b.path(), &b).expect("write b");
        let a = LoadedPackage::new(
            provenance("acme.a"),
            read_archive(temp_a.path()).expect("read a"),
        );
        let b = LoadedPackage::new(
            provenance("acme.b"),
            read_archive(temp_b.path()).expect("read b"),
        );
        let report = detect_conflicts(&[a, b]).expect("detect");
        assert!(report
            .issues
            .iter()
            .any(|issue| issue.kind == ConflictKind::Skill));
    }

    #[test]
    fn detects_duplicate_package_id_across_versions() {
        let (_temp_1, p1) = loaded(provenance("acme.dup"), empty_manifest("acme.dup"));
        let (_temp_2, p2) = loaded(
            PackageProvenance::new(
                PackageId::new("acme.dup").unwrap(),
                Version::new(2, 0, 0),
                PackageScope::Global,
            ),
            empty_manifest("acme.dup"),
        );
        let report = detect_conflicts(&[p1, p2]).expect("detect");
        assert!(report
            .issues
            .iter()
            .any(|issue| issue.kind == ConflictKind::PackageId));
    }

    #[test]
    fn different_scopes_do_not_collide() {
        let mut a = empty_manifest("acme.a");
        a.mcp.push(McpServerDeclaration {
            name: "fs".into(),
            transport: McpTransportSpec::Stdio {
                command: "npx".into(),
                args: Vec::new(),
                env: Default::default(),
            },
            auto_start: false,
        });
        let mut b = empty_manifest("acme.b");
        b.scope = PackageScope::Workspace {
            workspace_id: agent_domain::WorkspaceId::new("ws"),
        };
        b.mcp.push(McpServerDeclaration {
            name: "fs".into(),
            transport: McpTransportSpec::Stdio {
                command: "npx".into(),
                args: Vec::new(),
                env: Default::default(),
            },
            auto_start: false,
        });
        let (_temp_a, a) = loaded(provenance("acme.a"), a);
        let (_temp_b, b) = loaded(
            PackageProvenance::new(
                PackageId::new("acme.b").unwrap(),
                Version::new(1, 0, 0),
                PackageScope::Workspace {
                    workspace_id: agent_domain::WorkspaceId::new("ws"),
                },
            ),
            b,
        );
        let report = detect_conflicts(&[a, b]).expect("detect");
        // 不同 scope 不冲突；同 package id 不同名也不冲突。
        assert!(!report.has_blocking(), "{report:?}");
    }

    #[test]
    fn same_skill_id_at_different_paths_conflicts_after_path_resolution() {
        // 两个 package 用不同相对路径、但子 manifest id 相同（"search"）：
        // 解析 path 引用得到 id 后必须冲突，而不是只看路径。
        let root_a = tempfile::tempdir().unwrap();
        add_skill(root_a.path(), "skills/a-search");
        let root_b = tempfile::tempdir().unwrap();
        add_skill(root_b.path(), "skills/b-search");

        let mut a = empty_manifest("acme.a");
        a.skills.push(crate::manifest::ResourceRef::Path {
            path: PackageRelativePath::new("skills/a-search").unwrap(),
        });
        let mut b = empty_manifest("acme.b");
        b.skills.push(crate::manifest::ResourceRef::Path {
            path: PackageRelativePath::new("skills/b-search").unwrap(),
        });
        write_archive(root_a.path(), &a).expect("write a");
        write_archive(root_b.path(), &b).expect("write b");
        let a = LoadedPackage::new(
            provenance("acme.a"),
            read_archive(root_a.path()).expect("read a"),
        );
        let b = LoadedPackage::new(
            provenance("acme.b"),
            read_archive(root_b.path()).expect("read b"),
        );
        let report = detect_conflicts(&[a, b]).expect("detect");
        assert!(
            report
                .issues
                .iter()
                .any(|issue| issue.kind == ConflictKind::Skill && issue.key == "search"),
            "{report:?}"
        );
    }

    #[test]
    fn path_agent_and_inline_agent_with_same_name_conflict() {
        // path 引用的 agent 解析出 name 后与 inline agent 的 name 同键空间。
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join("agents")).unwrap();
        fs::write(
            root.path().join("agents/default.toml"),
            "name='acme-default'\ninstructions='be helpful'",
        )
        .unwrap();

        let mut a = empty_manifest("acme.a");
        a.agents.push(crate::manifest::ResourceRef::Path {
            path: PackageRelativePath::new("agents/default.toml").unwrap(),
        });
        let mut b = empty_manifest("acme.b");
        b.agents.push(crate::manifest::ResourceRef::Inline {
            manifest: serde_json::json!({"name": "acme-default"}),
        });
        write_archive(root.path(), &a).expect("write a");
        let a = LoadedPackage::new(
            provenance("acme.a"),
            read_archive(root.path()).expect("read a"),
        );
        let (_temp_b, b) = loaded(provenance("acme.b"), b);
        let report = detect_conflicts(&[a, b]).expect("detect");
        assert!(
            report.issues.iter().any(
                |issue| issue.kind == ConflictKind::AgentProfile && issue.key == "acme-default"
            ),
            "{report:?}"
        );
    }

    #[test]
    fn bad_sub_manifest_is_fail_closed() {
        // skill 子 manifest 损坏（不可解析）：不得回退到路径键，必须报错。
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir_all(temp.path().join("skills/search")).unwrap();
        fs::write(
            temp.path().join("skills/search/manifest.toml"),
            "id = 'search'\nbroken [[[",
        )
        .unwrap();
        fs::write(temp.path().join("skills/search/SKILL.md"), "# Search").unwrap();
        let mut a = empty_manifest("acme.a");
        a.skills.push(crate::manifest::ResourceRef::Path {
            path: PackageRelativePath::new("skills/search").unwrap(),
        });
        write_archive(temp.path(), &a).expect("write");
        let a = LoadedPackage::new(
            provenance("acme.a"),
            read_archive(temp.path()).expect("read"),
        );

        let error = detect_conflicts(&[a]).unwrap_err();
        assert!(
            matches!(error, PackageError::ManifestField { .. }),
            "{error}"
        );
    }

    #[test]
    fn missing_identity_field_is_fail_closed() {
        // 子 manifest 可解析但缺身份字段（agent 无 name）：fail-closed。
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir_all(temp.path().join("agents")).unwrap();
        fs::write(
            temp.path().join("agents/default.toml"),
            "instructions='be helpful'",
        )
        .unwrap();
        let mut a = empty_manifest("acme.a");
        a.agents.push(crate::manifest::ResourceRef::Path {
            path: PackageRelativePath::new("agents/default.toml").unwrap(),
        });
        write_archive(temp.path(), &a).expect("write");
        let a = LoadedPackage::new(
            provenance("acme.a"),
            read_archive(temp.path()).expect("read"),
        );

        let error = detect_conflicts(&[a]).unwrap_err();
        assert!(
            error.to_string().contains("identity field `name`"),
            "{error}"
        );
    }

    #[test]
    fn inline_manifest_missing_identity_is_fail_closed() {
        let mut a = empty_manifest("acme.a");
        a.agents.push(crate::manifest::ResourceRef::Inline {
            manifest: serde_json::json!({"instructions": "be helpful"}),
        });
        let (_temp_a, a) = loaded(provenance("acme.a"), a);

        let error = detect_conflicts(&[a]).unwrap_err();
        assert!(error.to_string().contains("`name`"), "{error}");
    }

    #[cfg(unix)]
    #[test]
    fn symlink_swap_after_verification_is_fail_closed() {
        use std::os::unix::fs::symlink;

        // 归档验证完成后、冲突检测读取前，把子 manifest 替换为指向归档外
        // 同内容文件的 symlink：冲突检测经 read_file（根句柄逐级 no-follow）
        // 读取，必须 fail-closed。
        let temp = tempfile::tempdir().unwrap();
        add_skill(temp.path(), "skills/search");
        let mut a = empty_manifest("acme.a");
        a.skills.push(crate::manifest::ResourceRef::Path {
            path: PackageRelativePath::new("skills/search").unwrap(),
        });
        write_archive(temp.path(), &a).expect("write");
        let a = LoadedPackage::new(
            provenance("acme.a"),
            read_archive(temp.path()).expect("read"),
        );

        let outside = tempfile::tempdir().unwrap();
        fs::write(
            outside.path().join("manifest.toml"),
            "id='search'\nversion='1.0.0'",
        )
        .unwrap();
        let manifest_file = temp.path().join("skills/search/manifest.toml");
        fs::remove_file(&manifest_file).unwrap();
        symlink(outside.path().join("manifest.toml"), &manifest_file).unwrap();

        let error = detect_conflicts(&[a]).unwrap_err();
        assert!(matches!(error, PackageError::NotRegularFile(_)), "{error}");
    }

    fn fs_stdio_server() -> McpServerDeclaration {
        McpServerDeclaration {
            name: "fs".into(),
            transport: McpTransportSpec::Stdio {
                command: "npx".into(),
                args: Vec::new(),
                env: Default::default(),
            },
            auto_start: false,
        }
    }

    #[test]
    fn package_id_identity_uses_verified_manifest_not_provenance() {
        // provenance id 相同但归档 manifest id 不同：身份以已验证 manifest 为准，
        // 不报 PackageId 冲突；同 scope 同名 MCP server 仍照常冲突。
        let mut a = empty_manifest("acme.a");
        a.mcp.push(fs_stdio_server());
        let mut b = empty_manifest("acme.b");
        b.mcp.push(fs_stdio_server());
        let (_temp_a, a) = loaded(provenance("acme.same"), a);
        let (_temp_b, b) = loaded(provenance("acme.same"), b);
        let report = detect_conflicts(&[a, b]).expect("detect");
        assert!(
            !report
                .issues
                .iter()
                .any(|issue| issue.kind == ConflictKind::PackageId),
            "{report:?}"
        );
        assert!(
            report
                .issues
                .iter()
                .any(|issue| issue.kind == ConflictKind::McpServer && issue.key == "fs"),
            "{report:?}"
        );
    }

    #[test]
    fn package_id_conflict_fires_from_manifest_id_when_provenance_differs() {
        // 反向：provenance id 不同但归档 manifest id 相同 → 仍必须冲突（fail-closed）。
        let (_temp_a, a) = loaded(provenance("acme.alias-a"), empty_manifest("acme.dup"));
        let (_temp_b, b) = loaded(provenance("acme.alias-b"), empty_manifest("acme.dup"));
        let report = detect_conflicts(&[a, b]).expect("detect");
        assert!(
            report
                .issues
                .iter()
                .any(|issue| issue.kind == ConflictKind::PackageId && issue.key == "acme.dup"),
            "{report:?}"
        );
    }

    #[test]
    fn scope_identity_uses_verified_manifest_not_provenance() {
        // a：manifest scope 为 Workspace(ws)，provenance 记为 Global（仅诊断）；
        // b：Global。作用域以 manifest 为准 → 不同 scope，同名 MCP 不冲突。
        let mut a = empty_manifest("acme.a");
        a.scope = PackageScope::Workspace {
            workspace_id: agent_domain::WorkspaceId::new("ws"),
        };
        a.mcp.push(fs_stdio_server());
        let mut b = empty_manifest("acme.b");
        b.mcp.push(fs_stdio_server());
        let (_temp_a, a) = loaded(provenance("acme.a"), a);
        let (_temp_b, b) = loaded(provenance("acme.b"), b);
        let report = detect_conflicts(&[a, b]).expect("detect");
        assert!(!report.has_blocking(), "{report:?}");
    }
}
