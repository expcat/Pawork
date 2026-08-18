//! 来源探测：识别五类来源在 workspace 内的已知配置位置（及调用方显式
//! 启用的全局来源根），只产出路径清单，不做解析、不执行任何内容。
//!
//! 候选路径全部硬编码在静态表中；未知文件 / 未知版本不猜测执行。
//! AGENTS.md 层级探测按目录深度有界遍历，symlink 目录不跟随。
//! 总文件数 / 每类 / 层级 / 单目录枚举均有硬上限，超限即时截断而非先收集。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::config::ConfigTier;

use super::io::is_file_within;
use super::limits::CompatLimits;
use super::model::{CompatIssue, DetectedSourceSummary};
use super::source::{ExternalSource, SourceFileKind};

/// 检测到的源文件（按相对路径去重；同一文件可被多个来源声明）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DetectedFile {
    pub relative_path: String,
    pub tier: ConfigTier,
    pub kind: SourceFileKind,
    /// 声明该文件的所有来源（已排序去重）。
    pub claimants: Vec<ExternalSource>,
}

impl DetectedFile {
    /// 主要声明者：确定性取序最小的来源。
    pub(crate) fn primary(&self) -> ExternalSource {
        self.claimants[0]
    }
}

/// 静态候选：相对路径 → 类别。
fn static_candidates(workspace: bool) -> Vec<(ExternalSource, &'static str, SourceFileKind)> {
    let mut candidates = vec![
        (
            ExternalSource::Claude,
            "CLAUDE.md",
            SourceFileKind::InstructionsDoc,
        ),
        (
            ExternalSource::Claude,
            "CLAUDE.local.md",
            SourceFileKind::InstructionsDoc,
        ),
        (
            ExternalSource::Claude,
            ".claude/settings.json",
            SourceFileKind::ClaudeSettings,
        ),
        (
            ExternalSource::Claude,
            ".claude/settings.local.json",
            SourceFileKind::ClaudeSettings,
        ),
        (
            ExternalSource::Codex,
            ".codex/config.toml",
            SourceFileKind::ConfigToml,
        ),
        (
            ExternalSource::Codex,
            ".codex/agents.json",
            SourceFileKind::AgentsJson,
        ),
        (
            ExternalSource::Codex,
            ".codex/mcp.json",
            SourceFileKind::McpJson,
        ),
        (
            ExternalSource::Grok,
            ".grok/config.toml",
            SourceFileKind::ConfigToml,
        ),
        (
            ExternalSource::Cursor,
            ".cursor/mcp.json",
            SourceFileKind::McpJson,
        ),
        (
            ExternalSource::Cursor,
            ".cursor/instructions.md",
            SourceFileKind::InstructionsDoc,
        ),
        (
            ExternalSource::Cursor,
            ".cursorrules",
            SourceFileKind::InstructionsDoc,
        ),
        (
            ExternalSource::Pi,
            ".pi/settings.json",
            SourceFileKind::PiSettings,
        ),
        (
            ExternalSource::Pi,
            ".pi/SYSTEM.md",
            SourceFileKind::InstructionsDoc,
        ),
        (
            ExternalSource::Pi,
            ".pi/APPEND_SYSTEM.md",
            SourceFileKind::InstructionsDoc,
        ),
    ];
    // 共享声明者（跨来源同文件，按路径去重后合并声明者）。
    candidates.extend([
        (ExternalSource::Claude, ".mcp.json", SourceFileKind::McpJson),
        (ExternalSource::Grok, ".mcp.json", SourceFileKind::McpJson),
        (ExternalSource::Cursor, ".mcp.json", SourceFileKind::McpJson),
    ]);
    if workspace {
        candidates.extend([
            (
                ExternalSource::Codex,
                "AGENTS.md",
                SourceFileKind::InstructionsDoc,
            ),
            (
                ExternalSource::Grok,
                "AGENTS.md",
                SourceFileKind::InstructionsDoc,
            ),
            (
                ExternalSource::Pi,
                "AGENTS.md",
                SourceFileKind::InstructionsDoc,
            ),
        ]);
    }
    candidates
}

/// 目录 glob：目录 → 内部文件模式。
enum GlobSpec {
    /// 目录下的直接文件（按扩展名）。
    Files {
        dir: &'static str,
        ext: &'static str,
        kind: SourceFileKind,
    },
    /// 子目录中的指定文件。
    Named {
        dir: &'static str,
        name: &'static str,
        kind: SourceFileKind,
    },
}

fn glob_candidates(_workspace: bool) -> Vec<(ExternalSource, GlobSpec)> {
    use GlobSpec::{Files, Named};
    vec![
        (
            ExternalSource::Claude,
            Files {
                dir: ".claude/rules",
                ext: "md",
                kind: SourceFileKind::InstructionsDoc,
            },
        ),
        (
            ExternalSource::Claude,
            Named {
                dir: ".claude/skills",
                name: "SKILL.md",
                kind: SourceFileKind::SkillMarkdown,
            },
        ),
        (
            ExternalSource::Claude,
            Files {
                dir: ".claude/agents",
                ext: "md",
                kind: SourceFileKind::AgentMarkdown,
            },
        ),
        (
            ExternalSource::Codex,
            Named {
                dir: ".codex/skills",
                name: "SKILL.md",
                kind: SourceFileKind::SkillMarkdown,
            },
        ),
        (
            ExternalSource::Codex,
            Files {
                dir: ".codex/agents",
                ext: "md",
                kind: SourceFileKind::AgentMarkdown,
            },
        ),
        (
            ExternalSource::Codex,
            Named {
                dir: ".codex/agents",
                name: "AGENT.md",
                kind: SourceFileKind::AgentMarkdown,
            },
        ),
        (
            ExternalSource::Grok,
            Named {
                dir: ".grok/skills",
                name: "SKILL.md",
                kind: SourceFileKind::SkillMarkdown,
            },
        ),
        (
            ExternalSource::Grok,
            Named {
                dir: ".grok/agents",
                name: "AGENT.md",
                kind: SourceFileKind::AgentMarkdown,
            },
        ),
        (
            ExternalSource::Cursor,
            Files {
                dir: ".cursor/rules",
                ext: "mdc",
                kind: SourceFileKind::InstructionsDoc,
            },
        ),
        (
            ExternalSource::Cursor,
            Files {
                dir: ".cursor/commands",
                ext: "md",
                kind: SourceFileKind::InstructionsDoc,
            },
        ),
        (
            ExternalSource::Pi,
            Named {
                dir: ".pi/skills",
                name: "SKILL.md",
                kind: SourceFileKind::SkillMarkdown,
            },
        ),
    ]
}

pub(crate) fn detect_files(
    workspace_root: Option<&Path>,
    global_roots: &[(ExternalSource, PathBuf)],
    limits: CompatLimits,
    issues: &mut Vec<CompatIssue>,
    summaries: &mut Vec<DetectedSourceSummary>,
) -> Vec<DetectedFile> {
    let mut by_path: BTreeMap<String, DetectedFile> = BTreeMap::new();
    let mut total = 0usize;
    let mut capped = false;

    if let Some(root) = workspace_root {
        for (source, rel, kind) in static_candidates(true) {
            register(
                root,
                source,
                rel,
                kind,
                ConfigTier::Workspace,
                limits,
                &mut total,
                &mut capped,
                &mut by_path,
                summaries,
            );
        }
        for (source, spec) in glob_candidates(true) {
            scan_glob(
                root,
                source,
                spec,
                ConfigTier::Workspace,
                limits,
                &mut total,
                &mut capped,
                &mut by_path,
                issues,
                summaries,
            );
        }
        scan_agents_hierarchy(
            root,
            limits,
            &mut total,
            &mut capped,
            &mut by_path,
            issues,
            summaries,
        );
    }

    for (source, global_root) in global_roots {
        for (candidate_source, rel, kind) in static_candidates(false) {
            if candidate_source != *source {
                continue;
            }
            register(
                global_root,
                *source,
                rel,
                kind,
                ConfigTier::Global,
                limits,
                &mut total,
                &mut capped,
                &mut by_path,
                summaries,
            );
        }
        for (candidate_source, spec) in glob_candidates(false) {
            if candidate_source != *source {
                continue;
            }
            scan_glob(
                global_root,
                *source,
                spec,
                ConfigTier::Global,
                limits,
                &mut total,
                &mut capped,
                &mut by_path,
                issues,
                summaries,
            );
        }
        if matches!(
            *source,
            ExternalSource::Codex | ExternalSource::Grok | ExternalSource::Pi
        ) {
            register(
                global_root,
                *source,
                "AGENTS.md",
                SourceFileKind::InstructionsDoc,
                ConfigTier::Global,
                limits,
                &mut total,
                &mut capped,
                &mut by_path,
                summaries,
            );
        }
    }

    if capped {
        issues.push(CompatIssue::warning(
            "scan_total_limit",
            format!(
                "total candidate files hard-capped at limit {}",
                limits.max_total_files
            ),
        ));
    }

    summaries.sort();
    summaries.dedup();

    let mut files: Vec<DetectedFile> = by_path.into_values().collect();
    files.sort_by(|left, right| {
        left.tier
            .cmp(&right.tier)
            .then_with(|| left.primary().cmp(&right.primary()))
            .then_with(|| left.relative_path.cmp(&right.relative_path))
    });
    files
}

/// 注册一个候选文件：命中硬上限（total）即记录截断并返回 false，不再写入
/// by_path。同一文件的后续声明者只合并 claimants，不重复计数。
#[allow(clippy::too_many_arguments)]
fn register(
    root: &Path,
    source: ExternalSource,
    rel: &str,
    kind: SourceFileKind,
    tier: ConfigTier,
    limits: CompatLimits,
    total: &mut usize,
    capped: &mut bool,
    by_path: &mut BTreeMap<String, DetectedFile>,
    summaries: &mut Vec<DetectedSourceSummary>,
) -> bool {
    let key = format!("{tier:?}:{rel}");
    if let Some(entry) = by_path.get_mut(&key) {
        if !entry.claimants.contains(&source) {
            entry.claimants.push(source);
            entry.claimants.sort();
        }
        summaries.push(DetectedSourceSummary {
            external: source,
            tier,
            relative_path: rel.to_string(),
            kind: format!("{kind:?}"),
        });
        return false;
    }
    if !is_file_within(root, Path::new(rel)) {
        return false;
    }
    if *total >= limits.max_total_files {
        *capped = true;
        return false;
    }
    by_path.insert(
        key.clone(),
        DetectedFile {
            relative_path: rel.to_string(),
            tier,
            kind,
            claimants: vec![source],
        },
    );
    summaries.push(DetectedSourceSummary {
        external: source,
        tier,
        relative_path: rel.to_string(),
        kind: format!("{kind:?}"),
    });
    *total += 1;
    true
}

/// 扫描目录 glob：直接文件（按扩展名）或子目录中的指定文件。
/// 单目录枚举按 max_dir_entries 硬截断；同类候选按 max_files_per_kind 硬截断。
#[allow(clippy::too_many_arguments)]
fn scan_glob(
    root: &Path,
    source: ExternalSource,
    spec: GlobSpec,
    tier: ConfigTier,
    limits: CompatLimits,
    total: &mut usize,
    capped: &mut bool,
    by_path: &mut BTreeMap<String, DetectedFile>,
    issues: &mut Vec<CompatIssue>,
    summaries: &mut Vec<DetectedSourceSummary>,
) {
    let (dir, ext, name, kind) = match &spec {
        GlobSpec::Files { dir, ext, kind } => (dir.to_string(), Some(ext.to_string()), None, *kind),
        GlobSpec::Named { dir, name, kind } => {
            (dir.to_string(), None, Some(name.to_string()), *kind)
        }
    };
    let scan_dir = root.join(&dir);
    let (entries, dir_truncated) =
        match super::io::sorted_children(&scan_dir, limits.max_dir_entries) {
            Ok(pair) => pair,
            Err(_) => return,
        };
    if dir_truncated {
        issues.push(CompatIssue::warning(
            "scan_dir_limit",
            format!(
                "{dir} entries hard-capped at limit {}",
                limits.max_dir_entries
            ),
        ));
    }
    let mut count = 0usize;
    for entry in entries {
        if count >= limits.max_files_per_kind {
            issues.push(CompatIssue::warning(
                "scan_kind_limit",
                format!(
                    "{} candidate files exceed limit {}",
                    dir, limits.max_files_per_kind
                ),
            ));
            break;
        }
        let Some(file_name) = entry
            .file_name()
            .map(|value| value.to_string_lossy().to_string())
        else {
            continue;
        };
        let rel = if let Some(ext) = &ext {
            if entry.extension().and_then(|value| value.to_str()) != Some(ext.as_str()) {
                continue;
            }
            format!("{dir}/{file_name}")
        } else {
            let name = name.as_deref().unwrap_or("");
            if !entry.is_dir() {
                continue;
            }
            let target = format!("{dir}/{file_name}/{name}");
            if !is_file_within(root, Path::new(&target)) {
                continue;
            }
            target
        };
        count += 1;
        register(
            root, source, &rel, kind, tier, limits, total, capped, by_path, summaries,
        );
    }
}

/// AGENTS.md 层级：从根到最大深度遍历（不跟随 symlink 目录、跳过隐藏目录）。
/// 单目录枚举按 max_dir_entries 硬截断；深度按 max_scan_depth 硬截断。
#[allow(clippy::too_many_arguments)]
fn scan_agents_hierarchy(
    root: &Path,
    limits: CompatLimits,
    total: &mut usize,
    capped: &mut bool,
    by_path: &mut BTreeMap<String, DetectedFile>,
    issues: &mut Vec<CompatIssue>,
    summaries: &mut Vec<DetectedSourceSummary>,
) {
    let mut stack: Vec<(String, usize)> = vec![(String::new(), 0)];
    while let Some((dir, depth)) = stack.pop() {
        let current = root.join(dir.as_str());
        let (entries, dir_truncated) =
            match super::io::sorted_children(&current, limits.max_dir_entries) {
                Ok(pair) => pair,
                Err(_) => continue,
            };
        if dir_truncated {
            issues.push(CompatIssue::warning(
                "scan_dir_limit",
                format!(
                    "AGENTS.md hierarchy entries hard-capped at limit {}",
                    limits.max_dir_entries
                ),
            ));
        }
        for entry in entries {
            let Some(name) = entry
                .file_name()
                .map(|value| value.to_string_lossy().to_string())
            else {
                continue;
            };
            if name.starts_with(".") {
                continue;
            }
            let rel = if dir.is_empty() {
                name.clone()
            } else {
                format!("{dir}/{name}")
            };
            if name == "AGENTS.md" && entry.is_file() {
                for source in [
                    ExternalSource::Codex,
                    ExternalSource::Grok,
                    ExternalSource::Pi,
                ] {
                    register(
                        root,
                        source,
                        &rel,
                        SourceFileKind::InstructionsDoc,
                        ConfigTier::Workspace,
                        limits,
                        total,
                        capped,
                        by_path,
                        summaries,
                    );
                }
            } else if entry.is_dir() && !entry.is_symlink() {
                if depth >= limits.max_scan_depth {
                    issues.push(CompatIssue::warning(
                        "scan_depth_limit",
                        format!(
                            "AGENTS.md hierarchy depth exceeds limit {}",
                            limits.max_scan_depth
                        ),
                    ));
                    continue;
                }
                stack.push((rel, depth + 1));
            }
        }
    }
}
