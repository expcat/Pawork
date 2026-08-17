//! log / show / merge-base（[`HistoryService`]）。
//!
//! - log：`git log -z --pretty=format:%H<US>%h<US>%an<US>%ae<US>%aI<US>%P<US>%s`，
//!   NUL 分隔记录、`\x1f` 分字段，解析为 [`CommitInfo`]。
//! - show：commit 元信息 + 完整 message + 变更文件清单（不含 patch 正文）。
//! - merge_base：无公共祖先时返回 `Ok(None)`。

use std::path::{Path, PathBuf};

use pawork_domain::CancellationToken;

use crate::error::GitError;
use crate::process::{validate_position_arg, GitRunner};

/// pretty format 中的字段分隔符（unit separator），不会出现在提交信息中。
const FS: &str = "%x1f";

/// 一条 commit 的元信息。
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CommitInfo {
    pub sha: String,
    pub short_sha: String,
    pub author_name: String,
    pub author_email: String,
    /// ISO 8601 严格格式（`%aI`）。
    pub author_date: String,
    pub parents: Vec<String>,
    pub subject: String,
}

/// `show` 的结果：元信息 + 完整 message + 变更文件。
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CommitDetail {
    pub info: CommitInfo,
    /// 完整提交信息（subject + body）。
    pub body: String,
    /// 变更文件路径（merge commit 可能为空）。
    pub files: Vec<String>,
}

/// log 查询选项。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogOptions {
    /// 返回条数上限（0 视为默认 100）。
    pub limit: usize,
    /// 可选 revision range（如 `HEAD~3..HEAD`、分支名）。
    pub range: Option<String>,
    /// 可选路径过滤（`-- <path>`）。
    pub path: Option<String>,
}

impl Default for LogOptions {
    fn default() -> Self {
        Self {
            limit: 100,
            range: None,
            path: None,
        }
    }
}

/// 提交历史服务。
pub struct HistoryService<'a> {
    runner: &'a GitRunner,
    work_dir: PathBuf,
}

impl<'a> HistoryService<'a> {
    pub fn new(runner: &'a GitRunner, work_dir: &Path) -> Self {
        Self {
            runner,
            work_dir: work_dir.to_path_buf(),
        }
    }

    /// 提交历史（新→旧）。
    pub async fn log(
        &self,
        opts: &LogOptions,
        cancel: CancellationToken,
    ) -> Result<Vec<CommitInfo>, GitError> {
        if let Some(range) = &opts.range {
            validate_position_arg("range", range)?;
        }
        let limit = if opts.limit == 0 { 100 } else { opts.limit };
        let format = format!("%H{FS}%h{FS}%an{FS}%ae{FS}%aI{FS}%P{FS}%s");
        let limit_arg = format!("-n{limit}");
        let fmt_arg = format!("--pretty=format:{format}");
        let mut args: Vec<String> = vec!["log".into(), limit_arg, "-z".into(), fmt_arg];
        if let Some(range) = &opts.range {
            args.push(range.clone());
        }
        if let Some(path) = &opts.path {
            args.push("--".into());
            args.push(path.clone());
        }
        let arg_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        let stdout = self.runner.run(&self.work_dir, &arg_refs, cancel).await?;
        // -z 使记录以 NUL 分隔。
        let mut commits = Vec::new();
        for record in stdout.split('\0') {
            let record = record.trim_matches(['\r', '\n']);
            if record.is_empty() {
                continue;
            }
            commits.push(parse_commit_info(record));
        }
        Ok(commits)
    }

    /// 单个 commit 的详情（元信息 + message + 文件清单）。
    pub async fn show(
        &self,
        rev: &str,
        cancel: CancellationToken,
    ) -> Result<CommitDetail, GitError> {
        validate_position_arg("revision", rev)?;
        let format = format!("%H{FS}%h{FS}%an{FS}%ae{FS}%aI{FS}%P{FS}%s{FS}%B");
        let fmt_arg = format!("--pretty=format:{format}");
        match self
            .runner
            .run(
                &self.work_dir,
                &["show", "--no-patch", &fmt_arg, rev],
                cancel.clone(),
            )
            .await
        {
            Ok(stdout) => {
                let record = stdout.trim_matches(['\r', '\n']);
                let info = parse_commit_info(record);
                // body 是最后一个字段（%B，可能含换行）。
                let body = record
                    .split('\x1f')
                    .nth(7)
                    .unwrap_or("")
                    .trim_end_matches(['\r', '\n'])
                    .to_string();
                let files = self.show_files(rev, cancel).await?;
                Ok(CommitDetail { info, body, files })
            }
            Err(GitError::GitFailed { ref stderr, .. }) => {
                if stderr.contains("unknown revision") || stderr.contains("Not a valid object name")
                {
                    return Err(GitError::ReferenceNotFound(rev.to_string()));
                }
                Err(GitError::GitFailed {
                    code: None,
                    stderr: stderr.clone(),
                })
            }
            Err(other) => Err(other),
        }
    }

    /// `git merge-base <a> <b>`：无公共祖先返回 `Ok(None)`。
    pub async fn merge_base(
        &self,
        a: &str,
        b: &str,
        cancel: CancellationToken,
    ) -> Result<Option<String>, GitError> {
        validate_position_arg("revision", a)?;
        validate_position_arg("revision", b)?;
        match self
            .runner
            .run(&self.work_dir, &["merge-base", a, b], cancel)
            .await
        {
            Ok(stdout) => Ok(Some(stdout.trim().to_string())),
            Err(GitError::GitFailed { code, ref stderr }) => {
                if stderr.contains("Not a valid object name") || stderr.contains("unknown revision")
                {
                    return Err(GitError::ReferenceNotFound(format!("{a}..{b}")));
                }
                // 无公共祖先：退出码 1 且无 stderr。
                if code == Some(1) && stderr.trim().is_empty() {
                    return Ok(None);
                }
                Err(GitError::GitFailed {
                    code,
                    stderr: stderr.clone(),
                })
            }
            Err(other) => Err(other),
        }
    }

    /// `git show --name-only`：commit 的变更文件清单。
    async fn show_files(
        &self,
        rev: &str,
        cancel: CancellationToken,
    ) -> Result<Vec<String>, GitError> {
        let stdout = self
            .runner
            .run(
                &self.work_dir,
                &["show", "--name-only", "--pretty=format:", rev],
                cancel,
            )
            .await?;
        Ok(stdout
            .lines()
            .map(|l| l.trim_end_matches('\r'))
            .filter(|l| !l.is_empty())
            .map(|l| l.to_string())
            .collect())
    }
}

/// 解析单条 `\x1f` 分隔的 commit 记录（前 7 个字段为元信息）。
fn parse_commit_info(record: &str) -> CommitInfo {
    let fields: Vec<&str> = record.split('\x1f').collect();
    let get = |i: usize| fields.get(i).copied().unwrap_or("").to_string();
    CommitInfo {
        sha: get(0),
        short_sha: get(1),
        author_name: get(2),
        author_email: get(3),
        author_date: get(4),
        parents: get(5).split_whitespace().map(|s| s.to_string()).collect(),
        subject: get(6),
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

    fn add_commit(repo: &Path, file: &str, content: &str, message: &str) -> String {
        std::fs::write(repo.join(file), content).expect("write");
        run_git(repo, &["add", file]);
        run_git(repo, &["commit", "-q", "-m", message]);
        run_git(repo, &["rev-parse", "HEAD"]).trim().to_string()
    }

    #[tokio::test]
    async fn log_returns_commits_newest_first() {
        let (_dir, repo) = make_repo();
        add_commit(&repo, "b.txt", "b1\n", "second");
        let third = add_commit(&repo, "c.txt", "c1\n", "third");
        let runner = GitRunner::new();
        let svc = HistoryService::new(&runner, &repo);
        let commits = svc
            .log(&LogOptions::default(), CancellationToken::new())
            .await
            .expect("log");
        assert_eq!(commits.len(), 3);
        assert_eq!(commits[0].subject, "third");
        assert_eq!(commits[0].sha, third);
        assert_eq!(commits[0].author_name, "Test");
        assert_eq!(commits[2].subject, "init");
        assert!(commits[0].short_sha.len() >= 7);
    }

    #[tokio::test]
    async fn log_limit_and_path_filter() {
        let (_dir, repo) = make_repo();
        add_commit(&repo, "b.txt", "b1\n", "touch b");
        add_commit(&repo, "b.txt", "b2\n", "touch b again");
        let runner = GitRunner::new();
        let svc = HistoryService::new(&runner, &repo);
        let limited = svc
            .log(
                &LogOptions {
                    limit: 1,
                    ..Default::default()
                },
                CancellationToken::new(),
            )
            .await
            .expect("log limit");
        assert_eq!(limited.len(), 1);
        let by_path = svc
            .log(
                &LogOptions {
                    path: Some("b.txt".into()),
                    ..Default::default()
                },
                CancellationToken::new(),
            )
            .await
            .expect("log path");
        assert_eq!(by_path.len(), 2, "只有两条 commit 触碰 b.txt");
        assert!(by_path.iter().all(|c| c.subject.contains("b")));
    }

    #[tokio::test]
    async fn show_returns_body_and_files() {
        let (_dir, repo) = make_repo();
        std::fs::write(repo.join("b.txt"), "b1\n").expect("write");
        run_git(&repo, &["add", "b.txt"]);
        run_git(
            &repo,
            &["commit", "-q", "-m", "subject line", "-m", "body paragraph"],
        );
        let runner = GitRunner::new();
        let svc = HistoryService::new(&runner, &repo);
        let detail = svc
            .show("HEAD", CancellationToken::new())
            .await
            .expect("show");
        assert_eq!(detail.info.subject, "subject line");
        assert!(
            detail.body.contains("body paragraph"),
            "body = {:?}",
            detail.body
        );
        assert!(
            detail.files.iter().any(|f| f == "b.txt"),
            "files = {:?}",
            detail.files
        );
    }

    #[tokio::test]
    async fn show_missing_rev_is_reference_not_found() {
        let (_dir, repo) = make_repo();
        let runner = GitRunner::new();
        let svc = HistoryService::new(&runner, &repo);
        let err = svc
            .show("no-such-rev", CancellationToken::new())
            .await
            .expect_err("不存在的 rev 应报错");
        assert!(
            matches!(err, GitError::ReferenceNotFound(_)),
            "err = {err:?}"
        );
    }

    #[tokio::test]
    async fn merge_base_finds_common_ancestor() {
        let (_dir, repo) = make_repo();
        let base_branch = run_git(&repo, &["symbolic-ref", "--short", "HEAD"])
            .trim()
            .to_string();
        let base = run_git(&repo, &["rev-parse", "HEAD"]).trim().to_string();
        run_git(&repo, &["checkout", "-q", "-b", "side"]);
        add_commit(&repo, "s.txt", "s\n", "side commit");
        run_git(&repo, &["checkout", "-q", &base_branch]);
        add_commit(&repo, "m.txt", "m\n", "mainline commit");
        let runner = GitRunner::new();
        let svc = HistoryService::new(&runner, &repo);
        let mb = svc
            .merge_base("HEAD", "side", CancellationToken::new())
            .await
            .expect("merge_base")
            .expect("应有公共祖先");
        assert_eq!(mb, base);
    }

    #[tokio::test]
    async fn merge_base_without_common_ancestor_is_none() {
        let (_dir, repo) = make_repo();
        let base_branch = run_git(&repo, &["symbolic-ref", "--short", "HEAD"])
            .trim()
            .to_string();
        // orphan 分支：与主线无公共祖先。
        run_git(&repo, &["checkout", "-q", "--orphan", "lonely"]);
        run_git(&repo, &["rm", "-q", "-rf", "--cached", "."]);
        std::fs::write(repo.join("l.txt"), "lonely\n").expect("write");
        run_git(&repo, &["add", "l.txt"]);
        run_git(&repo, &["commit", "-q", "-m", "lonely init"]);
        let runner = GitRunner::new();
        let svc = HistoryService::new(&runner, &repo);
        let mb = svc
            .merge_base("HEAD", &base_branch, CancellationToken::new())
            .await
            .expect("merge_base");
        assert_eq!(mb, None, "orphan 分支不应有公共祖先");
    }

    #[tokio::test]
    async fn option_like_revisions_are_rejected_at_service_boundaries() {
        let runner = GitRunner::new();
        let svc = HistoryService::new(&runner, Path::new("."));

        let log_error = svc
            .log(
                &LogOptions {
                    range: Some("--all".into()),
                    ..Default::default()
                },
                CancellationToken::new(),
            )
            .await
            .expect_err("option-like range must be rejected");
        assert!(matches!(
            log_error,
            GitError::InvalidPositionArgument { name: "range", .. }
        ));

        let show_error = svc
            .show("--stat", CancellationToken::new())
            .await
            .expect_err("option-like revision must be rejected");
        assert!(matches!(
            show_error,
            GitError::InvalidPositionArgument {
                name: "revision",
                ..
            }
        ));

        for (a, b) in [("--octopus", "HEAD"), ("HEAD", "--all")] {
            let error = svc
                .merge_base(a, b, CancellationToken::new())
                .await
                .expect_err("option-like merge-base revision must be rejected");
            assert!(matches!(
                error,
                GitError::InvalidPositionArgument {
                    name: "revision",
                    ..
                }
            ));
        }
    }
}
