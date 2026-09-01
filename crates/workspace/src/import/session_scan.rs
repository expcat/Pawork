//! 只读发现本机会话文件(Claude Code / Codex rollout)。
//!
//! R6 波 C:供 `sessions import --from` 显式启用;只列路径与大小,不读取文件
//! 内容(解析与 Secret 扫描仍在 storage persist 入口)。目录遍历有界、不跟随
//! symlink;根目录不存在返回空列表,超限返回错误而非静默截断。
//!
//! Claude Code 会把 subagent 会话写成独立 `agent-*.jsonl`,行内 `sessionId`
//! 复用父会话。这些文件的 user/assistant 行均为 `isSidechain=true`,导入会
//! 占用父会话 identity 或与主文件冲突,扫描层直接排除。

use std::path::{Path, PathBuf};

use super::error::CompatError;
use super::io::sorted_children;
use super::limits::CompatLimits;
use super::source::ExternalSource;

/// 本机会话来源(仅限有文档化本地会话目录的两家)。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LocalSessionSource {
    /// Claude Code:`~/.claude/projects/**/*.jsonl`。
    Claude,
    /// Codex CLI:`~/.codex/sessions/**/rollout-*.jsonl`。
    Codex,
}

impl LocalSessionSource {
    /// 稳定小写标签(与配置导入的 ExternalSource 对齐)。
    pub const fn as_str(self) -> &'static str {
        match self {
            LocalSessionSource::Claude => "claude",
            LocalSessionSource::Codex => "codex",
        }
    }

    /// 从配置导入的 ExternalSource 收窄;无本地会话目录的来源返回 None。
    pub const fn from_external(source: ExternalSource) -> Option<Self> {
        match source {
            ExternalSource::Claude => Some(LocalSessionSource::Claude),
            ExternalSource::Codex => Some(LocalSessionSource::Codex),
            ExternalSource::Grok | ExternalSource::Cursor | ExternalSource::Pi => None,
        }
    }
}

impl std::fmt::Display for LocalSessionSource {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// 扫描根(从用户 home 推导;测试与隔离环境可显式构造)。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalSessionRoots {
    /// Claude Code:`~/.claude/projects`。
    pub claude_projects: PathBuf,
    /// Codex CLI:`~/.codex/sessions`。
    pub codex_sessions: PathBuf,
}

impl LocalSessionRoots {
    /// 经 directories 解析用户 home 后推导;无法解析 home 时报错。
    pub fn detect() -> Result<Self, CompatError> {
        let base = directories::BaseDirs::new()
            .ok_or_else(|| CompatError::Invalid("cannot resolve user home directory".into()))?;
        Ok(Self::from_home(base.home_dir()))
    }

    pub fn from_home(home: &Path) -> Self {
        Self {
            claude_projects: home.join(".claude").join("projects"),
            codex_sessions: home.join(".codex").join("sessions"),
        }
    }
}

/// 发现的一个本机会话文件(只含元数据,不含内容)。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalSessionFile {
    pub source: LocalSessionSource,
    pub path: PathBuf,
    pub size_bytes: u64,
}

/// 扫描指定来源的本机会话文件;只列路径不读内容。
///
/// - 根目录不存在 → 空(常见于未安装对应工具);
/// - 目录枚举 / 文件总数超限 → 错误(不静默截断);
/// - symlink 目录与 symlink 文件一律跳过(不跟随);
/// - 结果按路径稳定排序。
pub fn scan_local_sessions(
    source: LocalSessionSource,
    roots: &LocalSessionRoots,
) -> Result<Vec<LocalSessionFile>, CompatError> {
    let root = match source {
        LocalSessionSource::Claude => &roots.claude_projects,
        LocalSessionSource::Codex => &roots.codex_sessions,
    };
    let limits = CompatLimits::default();
    let mut files = Vec::new();
    if !root.exists() {
        return Ok(files);
    }
    let metadata = std::fs::symlink_metadata(root).map_err(|error| CompatError::io(root, error))?;
    if !metadata.is_dir() {
        return Err(CompatError::Invalid(format!(
            "session root is not a directory: {}",
            root.display()
        )));
    }
    walk_session_dir(root, source, &limits, 0, &mut files)?;
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(files)
}

fn walk_session_dir(
    dir: &Path,
    source: LocalSessionSource,
    limits: &CompatLimits,
    depth: usize,
    files: &mut Vec<LocalSessionFile>,
) -> Result<(), CompatError> {
    let (entries, truncated) = sorted_children(dir, limits.max_dir_entries)?;
    if truncated {
        return Err(CompatError::LimitExceeded(format!(
            "directory exceeds {} entries: {}",
            limits.max_dir_entries,
            dir.display()
        )));
    }
    for entry in entries {
        let metadata =
            std::fs::symlink_metadata(&entry).map_err(|error| CompatError::io(&entry, error))?;
        let file_type = metadata.file_type();
        if file_type.is_symlink() {
            continue;
        }
        if metadata.is_dir() {
            if depth + 1 >= limits.max_scan_depth {
                return Err(CompatError::LimitExceeded(format!(
                    "session scan depth exceeds {} at {}",
                    limits.max_scan_depth,
                    entry.display()
                )));
            }
            walk_session_dir(&entry, source, limits, depth + 1, files)?;
        } else if metadata.is_file() && matches_source_file(&entry, source) {
            if files.len() >= limits.max_total_files {
                return Err(CompatError::LimitExceeded(format!(
                    "session files exceed {} entries",
                    limits.max_total_files
                )));
            }
            files.push(LocalSessionFile {
                source,
                path: entry,
                size_bytes: metadata.len(),
            });
        }
    }
    Ok(())
}

fn matches_source_file(path: &Path, source: LocalSessionSource) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    match source {
        // Claude Code 主会话是 UUID.jsonl;subagent sidecar 为 agent-*.jsonl,
        // 行内 sessionId 复用父会话,扫描时排除以免 identity 冲突。
        LocalSessionSource::Claude => name.ends_with(".jsonl") && !name.starts_with("agent-"),
        LocalSessionSource::Codex => name.starts_with("rollout-") && name.ends_with(".jsonl"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scans_claude_projects_recursively_and_stably() {
        let home = tempfile::tempdir().expect("home");
        let projects = home.path().join(".claude/projects/demo/sub");
        std::fs::create_dir_all(&projects).expect("dirs");
        std::fs::create_dir_all(home.path().join(".claude/projects/other")).expect("other dirs");
        std::fs::write(projects.join("b.jsonl"), "{\"type\":\"user\"}\n").expect("b");
        std::fs::write(projects.join("a.jsonl"), "{}\n").expect("a");
        std::fs::write(home.path().join(".claude/projects/other/c.jsonl"), "{}\n").expect("c");
        std::fs::write(projects.join("notes.txt"), "not a session").expect("txt");
        std::fs::write(projects.join("agent-deadbeef.jsonl"), "{}\n").expect("agent sidecar");

        let roots = LocalSessionRoots::from_home(home.path());
        let files = scan_local_sessions(LocalSessionSource::Claude, &roots).expect("scan claude");
        let paths: Vec<&Path> = files.iter().map(|file| file.path.as_path()).collect();
        let mut expected = vec![
            home.path().join(".claude/projects/other/c.jsonl"),
            home.path().join(".claude/projects/demo/sub/a.jsonl"),
            home.path().join(".claude/projects/demo/sub/b.jsonl"),
        ];
        expected.sort();
        assert_eq!(
            paths,
            expected
                .iter()
                .map(|path| path.as_path())
                .collect::<Vec<_>>()
        );
        assert!(files
            .iter()
            .all(|file| file.source == LocalSessionSource::Claude && file.size_bytes > 0));
    }

    #[test]
    fn scans_codex_rollout_files_only() {
        let home = tempfile::tempdir().expect("home");
        let sessions = home.path().join(".codex/sessions/2026/08");
        std::fs::create_dir_all(&sessions).expect("dirs");
        std::fs::write(sessions.join("rollout-1.jsonl"), "{}\n").expect("rollout");
        std::fs::write(sessions.join("plain.jsonl"), "{}\n").expect("plain");
        std::fs::write(sessions.join("rollout-notes.txt"), "{}\n").expect("txt");

        let roots = LocalSessionRoots::from_home(home.path());
        let files = scan_local_sessions(LocalSessionSource::Codex, &roots).expect("scan codex");
        assert_eq!(files.len(), 1);
        assert!(files[0].path.ends_with("rollout-1.jsonl"));
    }

    #[test]
    fn missing_roots_return_empty_without_error() {
        let home = tempfile::tempdir().expect("home");
        let roots = LocalSessionRoots::from_home(home.path());
        assert!(scan_local_sessions(LocalSessionSource::Claude, &roots)
            .expect("claude scan")
            .is_empty());
        assert!(scan_local_sessions(LocalSessionSource::Codex, &roots)
            .expect("codex scan")
            .is_empty());
    }

    #[test]
    fn exceeding_scan_depth_returns_limit_error() {
        let home = tempfile::tempdir().expect("home");
        let mut nested = home.path().join(".claude/projects");
        for index in 0..CompatLimits::default().max_scan_depth {
            nested = nested.join(format!("d{index}"));
        }
        std::fs::create_dir_all(&nested).expect("nested dirs");
        std::fs::write(nested.join("too-deep.jsonl"), "{}\n").expect("file");
        let roots = LocalSessionRoots::from_home(home.path());
        let error =
            scan_local_sessions(LocalSessionSource::Claude, &roots).expect_err("depth limit");
        assert!(matches!(error, CompatError::LimitExceeded(_)));
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_are_skipped_not_followed() {
        let home = tempfile::tempdir().expect("home");
        let real_dir = tempfile::tempdir().expect("real dir");
        std::fs::write(real_dir.path().join("linked.jsonl"), "{}\n").expect("linked");
        let projects = home.path().join(".claude/projects/demo");
        std::fs::create_dir_all(&projects).expect("dirs");
        std::fs::write(projects.join("real.jsonl"), "{}\n").expect("real");
        std::os::unix::fs::symlink(real_dir.path(), projects.join("linked-dir"))
            .expect("dir symlink");
        std::os::unix::fs::symlink(
            real_dir.path().join("linked.jsonl"),
            projects.join("linked-file.jsonl"),
        )
        .expect("file symlink");

        let roots = LocalSessionRoots::from_home(home.path());
        let files = scan_local_sessions(LocalSessionSource::Claude, &roots).expect("scan claude");
        let names: Vec<&str> = files
            .iter()
            .map(|file| {
                file.path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or_default()
            })
            .collect();
        assert_eq!(names, vec!["real.jsonl"]);
    }

    #[test]
    fn claude_agent_sidecars_are_excluded() {
        let home = tempfile::tempdir().expect("home");
        let projects = home.path().join(".claude/projects/demo");
        std::fs::create_dir_all(&projects).expect("dirs");
        std::fs::write(
            projects.join("c8dc6640-15c0-468e-be68-42e8ea4bd3da.jsonl"),
            "{}\n",
        )
        .expect("main");
        std::fs::write(projects.join("agent-ac06716322a12001d.jsonl"), "{}\n").expect("agent");
        std::fs::write(projects.join("agent-notes.jsonl.bak"), "{}\n").expect("bak");

        let roots = LocalSessionRoots::from_home(home.path());
        let files = scan_local_sessions(LocalSessionSource::Claude, &roots).expect("scan claude");
        assert_eq!(files.len(), 1);
        assert!(files[0]
            .path
            .ends_with("c8dc6640-15c0-468e-be68-42e8ea4bd3da.jsonl"));
    }

    #[test]
    fn local_session_source_narrows_external_source() {
        assert_eq!(
            LocalSessionSource::from_external(ExternalSource::Claude),
            Some(LocalSessionSource::Claude)
        );
        assert_eq!(
            LocalSessionSource::from_external(ExternalSource::Codex),
            Some(LocalSessionSource::Codex)
        );
        assert_eq!(LocalSessionSource::from_external(ExternalSource::Pi), None);
        assert_eq!(LocalSessionSource::Claude.as_str(), "claude");
    }
}
