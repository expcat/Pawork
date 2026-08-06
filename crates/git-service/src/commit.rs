//! commit 操作（[`CommitService`]）。
//!
//! `git commit -m <message>` 的封装：空暂存区归一为 [`GitError::NothingToCommit`]，
//! 成功后返回新 HEAD 的完整 SHA。message 经参数数组传入，不拼接 shell 字符串。

use std::path::{Path, PathBuf};

use agent_domain::CancellationToken;

use crate::error::GitError;
use crate::process::GitRunner;

/// commit 选项。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CommitOptions {
    /// 允许空提交（`--allow-empty`）。
    pub allow_empty: bool,
    /// 修订上一次提交（`--amend`）。
    pub amend: bool,
}

/// commit 服务。
pub struct CommitService<'a> {
    runner: &'a GitRunner,
    work_dir: PathBuf,
}

impl<'a> CommitService<'a> {
    pub fn new(runner: &'a GitRunner, work_dir: &Path) -> Self {
        Self {
            runner,
            work_dir: work_dir.to_path_buf(),
        }
    }

    /// 提交当前暂存区内容，返回新 HEAD 的完整 SHA。
    ///
    /// 空 message 视为参数错误（[`GitError::Other`]）；空暂存区且未开
    /// `allow_empty` 时归一为 [`GitError::NothingToCommit`]。
    pub async fn commit(
        &self,
        message: &str,
        opts: &CommitOptions,
        cancel: CancellationToken,
    ) -> Result<String, GitError> {
        if message.trim().is_empty() {
            return Err(GitError::Other("commit message must not be empty".into()));
        }
        let mut args: Vec<String> = vec!["commit".into(), "-q".into(), "-m".into()];
        args.push(message.to_string());
        if opts.allow_empty {
            args.push("--allow-empty".into());
        }
        if opts.amend {
            args.push("--amend".into());
        }
        let arg_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        match self
            .runner
            .run_with_stderr(&self.work_dir, &arg_refs, cancel.clone())
            .await
        {
            Ok(_) => {}
            Err(GitError::GitFailed { code, stderr }) => {
                // 「nothing to commit」由 git 写到 stdout，而错误路径只能拿到
                // stderr，故这里以 stderr 为主、退出码 1 且 stderr 为空兜底。
                if stderr.contains("nothing to commit")
                    || (code == Some(1) && stderr.trim().is_empty())
                {
                    return Err(GitError::NothingToCommit);
                }
                return Err(GitError::GitFailed { code: None, stderr });
            }
            Err(other) => return Err(other),
        }
        let sha = self
            .runner
            .run(&self.work_dir, &["rev-parse", "HEAD"], cancel)
            .await?;
        Ok(sha.trim().to_string())
    }
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

    fn make_repo() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().expect("tmpdir");
        let repo = dir.path().to_path_buf();
        run_git(&repo, &["init", "-q"]);
        run_git(&repo, &["config", "core.autocrlf", "false"]);
        std::fs::write(repo.join("a.txt"), "line1\n").expect("write");
        run_git(&repo, &["add", "a.txt"]);
        run_git(&repo, &["commit", "-q", "-m", "init"]);
        (dir, repo)
    }

    #[tokio::test]
    async fn commit_staged_change_returns_new_sha() {
        let (_dir, repo) = make_repo();
        let runner = GitRunner::new();
        let svc = CommitService::new(&runner, &repo);
        std::fs::write(repo.join("a.txt"), "line1\nline2\n").expect("write");
        run_git(&repo, &["add", "a.txt"]);
        let sha = svc
            .commit(
                "second",
                &CommitOptions::default(),
                CancellationToken::new(),
            )
            .await
            .expect("commit");
        assert_eq!(sha.len(), 40, "应为完整 SHA：{sha}");
        let subject = run_git(&repo, &["log", "-1", "--format=%s"]);
        assert_eq!(subject.trim(), "second");
        let head = run_git(&repo, &["rev-parse", "HEAD"]);
        assert_eq!(head.trim(), sha);
    }

    #[tokio::test]
    async fn commit_empty_index_is_nothing_to_commit() {
        let (_dir, repo) = make_repo();
        let runner = GitRunner::new();
        let svc = CommitService::new(&runner, &repo);
        let err = svc
            .commit("nope", &CommitOptions::default(), CancellationToken::new())
            .await
            .expect_err("空暂存区应报错");
        assert!(matches!(err, GitError::NothingToCommit), "err = {err:?}");
    }

    #[tokio::test]
    async fn commit_allow_empty_succeeds() {
        let (_dir, repo) = make_repo();
        let runner = GitRunner::new();
        let svc = CommitService::new(&runner, &repo);
        let opts = CommitOptions {
            allow_empty: true,
            amend: false,
        };
        let sha = svc
            .commit("empty", &opts, CancellationToken::new())
            .await
            .expect("allow-empty commit");
        assert_eq!(sha.len(), 40);
    }

    #[tokio::test]
    async fn commit_amend_replaces_last_commit() {
        let (_dir, repo) = make_repo();
        let runner = GitRunner::new();
        let svc = CommitService::new(&runner, &repo);
        let before = run_git(&repo, &["rev-parse", "HEAD"]);
        let opts = CommitOptions {
            allow_empty: false,
            amend: true,
        };
        let sha = svc
            .commit("amended", &opts, CancellationToken::new())
            .await
            .expect("amend");
        assert_ne!(sha, before.trim(), "amend 应产生新 SHA");
        let subject = run_git(&repo, &["log", "-1", "--format=%s"]);
        assert_eq!(subject.trim(), "amended");
    }

    #[tokio::test]
    async fn commit_empty_message_rejected() {
        let (_dir, repo) = make_repo();
        let runner = GitRunner::new();
        let svc = CommitService::new(&runner, &repo);
        let err = svc
            .commit("  ", &CommitOptions::default(), CancellationToken::new())
            .await
            .expect_err("空 message 应报错");
        assert!(matches!(err, GitError::Other(_)), "err = {err:?}");
    }
}
