//! 会话改动集 → git diff（工作区）或快照对比（非 git）。

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use pawork_domain::{CancellationToken, SessionId};
use pawork_git::diff::FileStatus;
use pawork_git::{
    paginate, DiffFile, DiffHunk, DiffLine, DiffOptions, DiffPage, DiffService, GitError,
    GitService, Head, HunkId, LineKind, StatusService,
};
use pawork_storage::blob::{ArtifactStore, FileSnapshot};

use crate::checkpoint::{first_snapshots, run_checkpoints, session_changed_paths, session_run_ids};
use crate::{AppCore, AppError};

/// 会话累计 diff。
#[derive(Clone, Debug, serde::Serialize)]
pub struct SessionDiff {
    pub session_id: String,
    pub files: Vec<DiffFile>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub git: Option<GitDiffHeader>,
}

/// git 仓库头信息（CLI `--json` 时打到 stderr）。
#[derive(Clone, Debug, serde::Serialize)]
pub struct GitDiffHeader {
    pub branch: String,
    pub work_dir: PathBuf,
    pub dirty_files: usize,
}

pub async fn session_diff(core: &AppCore, session_id: &SessionId) -> Result<SessionDiff, AppError> {
    let runs = match core.checkpoints.as_ref() {
        Some(service) => {
            let run_ids = session_run_ids(core, session_id).await?;
            run_checkpoints(service, &run_ids)
        }
        None => Vec::new(),
    };
    let session_paths = session_changed_paths(&runs);
    let roots = core.workspace_for_session_or_unbound(session_id)?.roots;
    let root = roots.first().cloned();
    let (files, git) = match root.as_deref() {
        Some(root) => match try_git_diff(root, &session_paths).await {
            Ok(pair) => pair,
            Err(AppError::Git(GitError::NotARepository(_)))
            | Err(AppError::Git(GitError::GitNotFound(_))) => (
                snapshot_diff(core.artifacts.as_ref(), &roots, &runs).await?,
                None,
            ),
            Err(error) => return Err(error),
        },
        None => (
            snapshot_diff(core.artifacts.as_ref(), &roots, &runs).await?,
            None,
        ),
    };
    Ok(SessionDiff {
        session_id: session_id.as_str().to_string(),
        files,
        git,
    })
}

pub fn paginate_diff(files: Vec<DiffFile>, page: usize, page_size: usize) -> DiffPage {
    paginate(files, page, page_size)
}

pub fn render_session_diff(diff: &SessionDiff) -> String {
    if diff.files.is_empty() {
        return String::new();
    }
    let mut out = String::new();
    for file in &diff.files {
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&render_diff_file(file));
    }
    out
}

pub fn render_diff_file(file: &DiffFile) -> String {
    let old = file.previous_path.as_deref().unwrap_or(file.path.as_str());
    let mut out = format!("--- {old}\n+++ {}\n", file.path);
    if file.binary {
        out.push_str("Binary files differ\n");
        return out;
    }
    for hunk in &file.hunks {
        out.push_str(&hunk.header);
        if !hunk.header.ends_with('\n') {
            out.push('\n');
        }
        for line in &hunk.lines {
            let prefix = match line.kind {
                LineKind::Context => ' ',
                LineKind::Addition => '+',
                LineKind::Deletion => '-',
            };
            out.push(prefix);
            out.push_str(&line.text);
            out.push('\n');
        }
    }
    out
}

/// 注入 provider 请求的短 git 状态；失败时省略，不阻断对话。
pub async fn git_status_note(roots: &[PathBuf]) -> Option<String> {
    let root = roots.first()?;
    let git = match GitService::open(root, CancellationToken::new()).await {
        Ok(git) => git,
        Err(_) => return None,
    };
    let branch = match git.current_head(CancellationToken::new()).await.ok()? {
        Head::Branch(name) => name,
        Head::Detached(sha) => {
            let short: String = sha.chars().take(8).collect();
            format!("detached:{short}")
        }
        Head::Unborn => "unborn".into(),
    };
    let status = StatusService::new(git.runner(), git.work_dir())
        .status(CancellationToken::new())
        .await
        .ok()?;
    Some(format!(
        "Git: branch `{branch}`, {} dirty files",
        status.changes.len()
    ))
}

async fn try_git_diff(
    root: &Path,
    session_paths: &BTreeSet<String>,
) -> Result<(Vec<DiffFile>, Option<GitDiffHeader>), AppError> {
    let git = match GitService::open(root, CancellationToken::new()).await {
        Ok(git) => git,
        Err(GitError::NotARepository(_)) | Err(GitError::GitNotFound(_)) => {
            return Err(AppError::Git(GitError::NotARepository(
                root.display().to_string(),
            )));
        }
        Err(error) => return Err(AppError::Git(error)),
    };
    let header = git_header(&git).await;
    if session_paths.is_empty() {
        return Ok((Vec::new(), header));
    }
    let files = DiffService::new(git.runner().clone(), git.work_dir())
        .diff(&DiffOptions::default(), CancellationToken::new())
        .await?;
    let git_root = git.work_dir().to_path_buf();
    let mut filtered: Vec<DiffFile> = files
        .into_iter()
        .filter(|file| {
            path_in_session(&file.path, session_paths, root, &git_root)
                || file.previous_path.as_deref().is_some_and(|previous| {
                    path_in_session(previous, session_paths, root, &git_root)
                })
        })
        .collect();
    fill_untracked_hunks(&mut filtered, root, &git_root);
    Ok((filtered, header))
}

async fn git_header(git: &GitService) -> Option<GitDiffHeader> {
    let branch = match git.current_head(CancellationToken::new()).await.ok()? {
        Head::Branch(name) => name,
        Head::Detached(sha) => {
            let short: String = sha.chars().take(8).collect();
            format!("detached:{short}")
        }
        Head::Unborn => "unborn".into(),
    };
    let dirty_files = StatusService::new(git.runner(), git.work_dir())
        .status(CancellationToken::new())
        .await
        .ok()?
        .changes
        .len();
    Some(GitDiffHeader {
        branch,
        work_dir: git.work_dir().to_path_buf(),
        dirty_files,
    })
}

fn fill_untracked_hunks(files: &mut [DiffFile], workspace: &Path, git_root: &Path) {
    for file in files {
        if file.status != FileStatus::Untracked || file.binary || !file.hunks.is_empty() {
            continue;
        }
        let bytes = std::fs::read(workspace.join(&file.path))
            .or_else(|_| std::fs::read(git_root.join(&file.path)));
        let Ok(bytes) = bytes else {
            continue;
        };
        let filled = bytes_to_diff_file(&file.path, &[], &bytes);
        file.additions = filled.additions;
        file.deletions = filled.deletions;
        file.binary = filled.binary;
        file.hunks = filled.hunks;
    }
}

fn path_in_session(
    git_path: &str,
    session_paths: &BTreeSet<String>,
    workspace: &Path,
    git_root: &Path,
) -> bool {
    let git_norm = normalize_path(git_path);
    if session_paths.contains(&git_norm) {
        return true;
    }
    for session in session_paths {
        if normalize_path(session) == git_norm {
            return true;
        }
        let candidate = workspace.join(session);
        if let Ok(rel) = candidate.strip_prefix(git_root) {
            if normalize_path(&rel.to_string_lossy()) == git_norm {
                return true;
            }
        }
    }
    false
}

fn normalize_path(path: &str) -> String {
    path.replace('\\', "/")
}

async fn snapshot_diff(
    artifacts: Option<&ArtifactStore>,
    roots: &[PathBuf],
    runs: &[pawork_storage::blob::RunCheckpoint],
) -> Result<Vec<DiffFile>, AppError> {
    let Some(artifacts) = artifacts else {
        return Ok(Vec::new());
    };
    let mut files = Vec::new();
    for snap in first_snapshots(runs) {
        if let Some(file) = snapshot_file_diff(artifacts, roots, &snap).await? {
            files.push(file);
        }
    }
    Ok(files)
}

async fn snapshot_file_diff(
    artifacts: &ArtifactStore,
    roots: &[PathBuf],
    snap: &FileSnapshot,
) -> Result<Option<DiffFile>, AppError> {
    let old = match &snap.pre_blob {
        Some(blob) => artifacts.get(blob).await?,
        None => Vec::new(),
    };
    let current_path = current_path(roots, &snap.relative_path);
    let new = match current_path.as_ref() {
        Some(path) => match tokio::fs::read(path).await {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(error) => return Err(error.into()),
        },
        None => Vec::new(),
    };
    if old == new {
        return Ok(None);
    }
    Ok(Some(bytes_to_diff_file(&snap.relative_path, &old, &new)))
}

fn current_path(roots: &[PathBuf], relative: &str) -> Option<PathBuf> {
    for root in roots {
        let candidate = root.join(relative);
        if candidate.exists() {
            return Some(candidate);
        }
    }
    roots.first().map(|root| root.join(relative))
}

fn bytes_to_diff_file(path: &str, old: &[u8], new: &[u8]) -> DiffFile {
    if looks_binary(old) || looks_binary(new) {
        return DiffFile {
            path: path.to_string(),
            previous_path: None,
            status: file_status(old, new),
            staged: false,
            binary: true,
            additions: 0,
            deletions: 0,
            hunks: Vec::new(),
        };
    }
    let old_text = String::from_utf8_lossy(old);
    let new_text = String::from_utf8_lossy(new);
    line_replacement_diff(path, &old_text, &new_text)
}

fn looks_binary(bytes: &[u8]) -> bool {
    bytes.contains(&0)
}

fn file_status(old: &[u8], new: &[u8]) -> FileStatus {
    match (old.is_empty(), new.is_empty()) {
        (true, false) => FileStatus::Added,
        (false, true) => FileStatus::Deleted,
        _ => FileStatus::Modified,
    }
}

fn line_replacement_diff(path: &str, old: &str, new: &str) -> DiffFile {
    let old_lines = split_lines(old);
    let new_lines = split_lines(new);
    let old_count = old_lines.len() as u32;
    let new_count = new_lines.len() as u32;
    let mut lines = Vec::with_capacity(old_lines.len() + new_lines.len());
    for text in old_lines {
        lines.push(DiffLine {
            kind: LineKind::Deletion,
            text,
            old_no_newline: false,
            new_no_newline: false,
        });
    }
    for text in new_lines {
        lines.push(DiffLine {
            kind: LineKind::Addition,
            text,
            old_no_newline: false,
            new_no_newline: false,
        });
    }
    let old_start = if old_count == 0 { 0 } else { 1 };
    let new_start = if new_count == 0 { 0 } else { 1 };
    DiffFile {
        path: path.to_string(),
        previous_path: None,
        status: file_status(old.as_bytes(), new.as_bytes()),
        staged: false,
        binary: false,
        additions: new_count,
        deletions: old_count,
        hunks: vec![DiffHunk {
            id: HunkId(1),
            old_start,
            old_lines: old_count,
            new_start,
            new_lines: new_count,
            header: format!("@@ -{old_start},{old_count} +{new_start},{new_count} @@"),
            lines,
        }],
    }
}

fn split_lines(text: &str) -> Vec<String> {
    if text.is_empty() {
        return Vec::new();
    }
    text.split('\n')
        .map(|line| line.trim_end_matches('\r').to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testsupport::mock_core;

    fn init_git(path: &Path) {
        let status = std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(path)
            .env("GIT_TERMINAL_PROMPT", "0")
            .status()
            .expect("git init");
        assert!(status.success(), "git init");
    }

    #[tokio::test]
    async fn session_diff_uses_session_workspace_roots() {
        let first = tempfile::tempdir().expect("first");
        let second = tempfile::tempdir().expect("second");
        init_git(first.path());
        init_git(second.path());
        let (mut core, _dir) = mock_core(Vec::new()).await;
        let first_record = core
            .register_workspace(first.path())
            .await
            .expect("register first");
        let second_record = core
            .register_workspace(second.path())
            .await
            .expect("register second");
        core.attach_workspace(&first_record.root_path)
            .expect("switch current to first");
        let session = core
            .create_session_with_workspace("in-second", second_record.workspace_id.clone())
            .await
            .expect("session in second");
        let diff = core.session_diff(&session).await.expect("diff");
        let git = diff.git.expect("git header");
        assert_eq!(git.work_dir, second_record.root_path);
        assert_ne!(git.work_dir, first_record.root_path);
        core.shutdown().await.expect("shutdown");
    }
}
