//! 冲突状态识别（[`ConflictService`]）。
//!
//! - [`ConflictService::unmerged`]：`git ls-files --unmerged -z`，按路径聚合
//!   stage 1/2/3（base/ours/theirs）对象，输出 [`UnmergedEntry`] 列表。
//! - [`ConflictService::is_merge_in_progress`]：探测 `MERGE_HEAD` 是否存在。
//!
//! 与 `status` 的 `FileStatus::Unmerged` 互补：这里给出冲突双方的对象级细节，
//! 供上层（GUI/工具）展示与解决。

use std::path::{Path, PathBuf};

use agent_domain::CancellationToken;

use crate::error::GitError;
use crate::process::GitRunner;

/// 一条未合并（冲突）路径的条目。
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct UnmergedEntry {
    pub path: String,
    /// stage 1：共同祖先版本 SHA（可能缺失，如 add/add 冲突）。
    pub base: Option<String>,
    /// stage 2：当前分支（ours）版本 SHA。
    pub ours: Option<String>,
    /// stage 3：被合并分支（theirs）版本 SHA。
    pub theirs: Option<String>,
}

/// 冲突状态服务。
pub struct ConflictService<'a> {
    runner: &'a GitRunner,
    work_dir: PathBuf,
}

impl<'a> ConflictService<'a> {
    pub fn new(runner: &'a GitRunner, work_dir: &Path) -> Self {
        Self {
            runner,
            work_dir: work_dir.to_path_buf(),
        }
    }

    /// 当前 index 中所有未合并路径（含 base/ours/theirs 对象 SHA）。
    pub async fn unmerged(
        &self,
        cancel: CancellationToken,
    ) -> Result<Vec<UnmergedEntry>, GitError> {
        let stdout = self
            .runner
            .run(&self.work_dir, &["ls-files", "--unmerged", "-z"], cancel)
            .await?;
        Ok(parse_unmerged(&stdout))
    }

    /// 是否处于 merge 进行中（`MERGE_HEAD` 存在）。
    pub async fn is_merge_in_progress(&self, cancel: CancellationToken) -> Result<bool, GitError> {
        match self
            .runner
            .run(
                &self.work_dir,
                &["rev-parse", "--verify", "-q", "MERGE_HEAD"],
                cancel,
            )
            .await
        {
            Ok(stdout) => Ok(!stdout.trim().is_empty()),
            // -q 时不存在即安静地非零退出。
            Err(GitError::GitFailed { .. }) => Ok(false),
            Err(other) => Err(other),
        }
    }
}

/// 解析 `git ls-files --unmerged -z` 输出。
///
/// 每条形如 `<mode> <sha> <stage>\t<path>`，以 NUL 分隔；按路径聚合，
/// 保持首次出现顺序。
fn parse_unmerged(stdout: &str) -> Vec<UnmergedEntry> {
    let mut entries: Vec<UnmergedEntry> = Vec::new();
    for record in stdout.split('\0') {
        let record = record.trim_end_matches('\r');
        if record.is_empty() {
            continue;
        }
        // "<mode> <sha> <stage>\t<path>"
        let (meta, path) = match record.split_once('\t') {
            Some(pair) => pair,
            None => continue,
        };
        let mut fields = meta.split_whitespace();
        let _mode = fields.next();
        let sha = fields.next().unwrap_or("");
        let stage: u32 = fields.next().and_then(|s| s.parse().ok()).unwrap_or(0);
        let entry = match entries.iter_mut().find(|e| e.path == path) {
            Some(e) => e,
            None => {
                entries.push(UnmergedEntry {
                    path: path.to_string(),
                    ..Default::default()
                });
                entries.last_mut().expect("just pushed")
            }
        };
        match stage {
            1 => entry.base = Some(sha.to_string()),
            2 => entry.ours = Some(sha.to_string()),
            3 => entry.theirs = Some(sha.to_string()),
            _ => {}
        }
    }
    entries
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn git_env() -> Vec<(&'static str, &'static str)> {
        vec![
            ("GIT_AUTHOR_NAME", "Test"),
            ("GIT_AUTHOR_EMAIL", "test@example.com"),
            ("GIT_COMMITTER_NAME", "Test"),
            ("GIT_COMMITTER_EMAIL", "test@example.com"),
            ("GIT_TERMINAL_PROMPT", "0"),
        ]
    }

    fn run_git(cwd: &Path, args: &[&str]) -> String {
        let mut cmd = Command::new("git");
        cmd.current_dir(cwd).args(args);
        for (k, v) in git_env() {
            cmd.env(k, v);
        }
        let out = cmd.output().expect("git exec");
        assert!(
            out.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    /// 允许失败的 git 调用（merge 冲突时退出非零）。
    fn try_git(cwd: &Path, args: &[&str]) -> (bool, String, String) {
        let mut cmd = Command::new("git");
        cmd.current_dir(cwd).args(args);
        for (k, v) in git_env() {
            cmd.env(k, v);
        }
        let out = cmd.output().expect("git exec");
        (
            out.status.success(),
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    }

    fn make_repo() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().expect("tmpdir");
        let repo = dir.path().to_path_buf();
        run_git(&repo, &["init", "-q"]);
        run_git(&repo, &["config", "core.autocrlf", "false"]);
        std::fs::write(repo.join("a.txt"), "base\n").expect("write");
        run_git(&repo, &["add", "a.txt"]);
        run_git(&repo, &["commit", "-q", "-m", "init"]);
        (dir, repo)
    }

    /// 制造一个真实 merge 冲突：两条分支修改同一行。
    fn make_conflict(repo: &Path) {
        let base_branch = run_git(repo, &["symbolic-ref", "--short", "HEAD"])
            .trim()
            .to_string();
        run_git(repo, &["checkout", "-q", "-b", "other"]);
        std::fs::write(repo.join("a.txt"), "theirs\n").expect("write");
        run_git(repo, &["add", "a.txt"]);
        run_git(repo, &["commit", "-q", "-m", "other change"]);
        run_git(repo, &["checkout", "-q", &base_branch]);
        std::fs::write(repo.join("a.txt"), "ours\n").expect("write");
        run_git(repo, &["add", "a.txt"]);
        run_git(repo, &["commit", "-q", "-m", "base change"]);
        let (ok, _out, _err) = try_git(repo, &["merge", "--no-edit", "other"]);
        assert!(!ok, "merge 应冲突");
    }

    #[tokio::test]
    async fn unmerged_lists_conflicted_paths() {
        let (_dir, repo) = make_repo();
        make_conflict(&repo);
        let runner = GitRunner::new();
        let svc = ConflictService::new(&runner, &repo);
        let entries = svc
            .unmerged(CancellationToken::new())
            .await
            .expect("unmerged");
        assert_eq!(entries.len(), 1);
        let e = &entries[0];
        assert_eq!(e.path, "a.txt");
        assert!(e.base.is_some(), "content 冲突应有 base：{e:?}");
        assert!(e.ours.is_some(), "{e:?}");
        assert!(e.theirs.is_some(), "{e:?}");
        assert!(svc
            .is_merge_in_progress(CancellationToken::new())
            .await
            .expect("merge state"));
    }

    #[tokio::test]
    async fn no_conflict_is_empty_and_not_merging() {
        let (_dir, repo) = make_repo();
        let runner = GitRunner::new();
        let svc = ConflictService::new(&runner, &repo);
        let entries = svc
            .unmerged(CancellationToken::new())
            .await
            .expect("unmerged");
        assert!(entries.is_empty());
        assert!(!svc
            .is_merge_in_progress(CancellationToken::new())
            .await
            .expect("merge state"));
    }

    #[tokio::test]
    async fn abort_returns_to_clean_state() {
        let (_dir, repo) = make_repo();
        make_conflict(&repo);
        run_git(&repo, &["merge", "--abort"]);
        let runner = GitRunner::new();
        let svc = ConflictService::new(&runner, &repo);
        let entries = svc
            .unmerged(CancellationToken::new())
            .await
            .expect("unmerged");
        assert!(entries.is_empty(), "abort 后应无冲突");
        assert!(!svc
            .is_merge_in_progress(CancellationToken::new())
            .await
            .unwrap());
    }

    #[test]
    fn parse_unmerged_groups_stages_by_path() {
        let stdout = "100644 aaa 1\ta.txt\u{0}100644 bbb 2\ta.txt\u{0}100644 ccc 3\ta.txt\u{0}";
        let entries = parse_unmerged(stdout);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path, "a.txt");
        assert_eq!(entries[0].base.as_deref(), Some("aaa"));
        assert_eq!(entries[0].ours.as_deref(), Some("bbb"));
        assert_eq!(entries[0].theirs.as_deref(), Some("ccc"));
    }
}
