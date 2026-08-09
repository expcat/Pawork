//! Git 仓库检测与基础元信息（[`GitService`]）。
//!
//! 通过系统 git 探测 work tree、git dir、HEAD 状态等，供后续 status/stage/
//! worktree 模块复用 [`crate::process::GitRunner`]。

use std::path::{Path, PathBuf};

use agent_domain::CancellationToken;

use crate::error::GitError;
use crate::process::GitRunner;

/// HEAD 的解析结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Head {
    /// 指向某分支（短名，如 `main`）。
    Branch(String),
    /// 处于 detached HEAD，附带当前 commit 的 SHA。
    Detached(String),
    /// 尚未有任何 commit（unborn HEAD，`HEAD` 指向不存在的分支）。
    Unborn,
}

/// 仓库基础元信息。
#[derive(Clone, Debug)]
pub struct RepoInfo {
    /// work tree 根目录（裸仓库为仓库路径本身）。
    pub work_dir: PathBuf,
    /// `.git` 目录路径。
    pub git_dir: PathBuf,
    /// HEAD 状态。
    pub head: Head,
    /// 是否为裸仓库。
    pub bare: bool,
}

/// 对单个 git 仓库的访问句柄。
///
/// [`GitService::open`] 沿父目录向上探测 work tree，后续方法复用 [`GitRunner`]
/// 调用 git。
#[derive(Debug)]
pub struct GitService {
    runner: GitRunner,
    work_dir: PathBuf,
}

impl GitService {
    /// 沿父目录向上检测 `git rev-parse --show-toplevel`，得到 work tree 根。
    ///
    /// 传入路径不是任何仓库的子目录时返回 [`GitError::NotARepository`]。
    pub async fn open(path: &Path, cancel: CancellationToken) -> Result<Self, GitError> {
        let runner = GitRunner::new();
        // --show-toplevel 对非仓库返回非零退出 + stderr（如 "not a git repository"）。
        match runner
            .run_with_stderr(path, &["rev-parse", "--show-toplevel"], cancel)
            .await
        {
            Ok((stdout, _stderr)) => {
                let work_dir = PathBuf::from(stdout.trim());
                Ok(Self { runner, work_dir })
            }
            // rev-parse 探测失败即视为非仓库（或该路径不可用作为仓库）。
            Err(GitError::GitFailed { .. }) => {
                Err(GitError::NotARepository(path.display().to_string()))
            }
            Err(other) => Err(other),
        }
    }

    /// work tree 根目录。
    pub fn work_dir(&self) -> &Path {
        &self.work_dir
    }

    /// 暴露内部 [`GitRunner`] 供下游模块复用。
    pub fn runner(&self) -> &GitRunner {
        &self.runner
    }

    /// `git rev-parse --absolute-git-dir`，始终返回绝对管理目录；linked worktree
    /// 会指向主仓库 `.git/worktrees/<name>`，而不是 worktree 根下的 `.git` 文件。
    pub async fn git_dir(&self, cancel: CancellationToken) -> Result<PathBuf, GitError> {
        let stdout = self
            .runner
            .run(
                self.work_dir(),
                &["rev-parse", "--absolute-git-dir"],
                cancel,
            )
            .await?;
        Ok(PathBuf::from(stdout.trim()))
    }

    /// `git rev-parse --is-bare-repository`。
    pub async fn is_bare(&self, cancel: CancellationToken) -> Result<bool, GitError> {
        let stdout = self
            .runner
            .run(
                self.work_dir(),
                &["rev-parse", "--is-bare-repository"],
                cancel,
            )
            .await?;
        Ok(stdout.trim() == "true")
    }

    /// 当前分支短名（`symbolic-ref --short HEAD`）。
    ///
    /// detached / unborn 时返回 `None`。
    pub async fn current_branch(
        &self,
        cancel: CancellationToken,
    ) -> Result<Option<String>, GitError> {
        match self.current_head(cancel).await? {
            Head::Branch(name) => Ok(Some(name)),
            _ => Ok(None),
        }
    }

    /// 解析 HEAD 状态。
    ///
    /// 先 `symbolic-ref --short HEAD`；失败且 stderr 含 `detached` →
    /// `Detached(rev-parse HEAD)`；输出为空 → `Unborn`。
    pub async fn current_head(&self, cancel: CancellationToken) -> Result<Head, GitError> {
        match self
            .runner
            .run_with_stderr(
                self.work_dir(),
                &["symbolic-ref", "--short", "HEAD"],
                cancel.clone(),
            )
            .await
        {
            Ok((stdout, _stderr)) => {
                let name = stdout.trim();
                if name.is_empty() {
                    Ok(Head::Unborn)
                } else {
                    Ok(Head::Branch(name.to_string()))
                }
            }
            Err(GitError::GitFailed { stderr, .. }) => {
                // detached HEAD：symbolic-ref 报 "HEAD is detached"。
                if stderr.contains("detached") {
                    let rev = self
                        .runner
                        .run(self.work_dir(), &["rev-parse", "HEAD"], cancel)
                        .await?;
                    Ok(Head::Detached(rev.trim().to_string()))
                } else {
                    // 例如 unborn：symbolic-ref 退出非零且 stderr 提示 ref 不存在。
                    Ok(Head::Unborn)
                }
            }
            Err(other) => Err(other),
        }
    }

    /// 汇总仓库元信息。
    pub async fn repo_info(&self, cancel: CancellationToken) -> Result<RepoInfo, GitError> {
        // 一次 rev-parse 同时读取路径、bare 标志与 HEAD SHA；正常/detached 仓库
        // 再用一次 symbolic-ref 区分 Branch/Detached，总计固定两次 spawn。
        let combined = self
            .runner
            .run_with_stderr(
                self.work_dir(),
                &[
                    "rev-parse",
                    "--show-toplevel",
                    "--absolute-git-dir",
                    "--is-bare-repository",
                    "HEAD",
                ],
                cancel.clone(),
            )
            .await;

        let (work_dir, git_dir, bare, head_sha) = match combined {
            Ok((stdout, _)) => parse_repo_metadata(&stdout, true)?,
            Err(first_error @ GitError::GitFailed { .. }) => {
                // unborn HEAD 会使同一 rev-parse 整体退出非零；回退到不含 HEAD 的
                // 合并查询，仍只需两次 spawn，且无需 symbolic-ref。
                match self
                    .runner
                    .run(
                        self.work_dir(),
                        &[
                            "rev-parse",
                            "--show-toplevel",
                            "--absolute-git-dir",
                            "--is-bare-repository",
                        ],
                        cancel.clone(),
                    )
                    .await
                {
                    Ok(stdout) => parse_repo_metadata(&stdout, false)?,
                    Err(_) => return Err(first_error),
                }
            }
            Err(error) => return Err(error),
        };

        let head = if let Some(sha) = head_sha {
            match self
                .runner
                .run_with_stderr(
                    self.work_dir(),
                    &["symbolic-ref", "--short", "HEAD"],
                    cancel,
                )
                .await
            {
                Ok((stdout, _)) if !stdout.trim().is_empty() => {
                    Head::Branch(stdout.trim().to_string())
                }
                Ok(_) | Err(GitError::GitFailed { .. }) => Head::Detached(sha),
                Err(error) => return Err(error),
            }
        } else {
            Head::Unborn
        };

        Ok(RepoInfo {
            work_dir,
            git_dir,
            head,
            bare,
        })
    }
}

fn parse_repo_metadata(
    stdout: &str,
    includes_head: bool,
) -> Result<(PathBuf, PathBuf, bool, Option<String>), GitError> {
    let fields: Vec<&str> = stdout
        .lines()
        .map(|line| line.trim_end_matches('\r'))
        .collect();
    let expected = if includes_head { 4 } else { 3 };
    if fields.len() != expected || fields.iter().any(|field| field.is_empty()) {
        return Err(GitError::Other(format!(
            "unexpected git rev-parse metadata output: expected {expected} fields, got {}",
            fields.len()
        )));
    }
    let bare = match fields[2] {
        "true" => true,
        "false" => false,
        value => {
            return Err(GitError::Other(format!(
                "unexpected --is-bare-repository value: {value}"
            )))
        }
    };
    Ok((
        PathBuf::from(fields[0]),
        PathBuf::from(fields[1]),
        bare,
        includes_head.then(|| fields[3].to_string()),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use tempfile::TempDir;

    /// 为测试 git 命令注入确定性的作者/提交者环境，并禁止任何交互提示。
    fn git_env(cmd: &mut Command) {
        const NAME: &str = "Tester";
        const EMAIL: &str = "tester@example.com";
        const DATE: &str = "2000-01-01T00:00:00Z";
        cmd.env("GIT_AUTHOR_NAME", NAME)
            .env("GIT_AUTHOR_EMAIL", EMAIL)
            .env("GIT_AUTHOR_DATE", DATE)
            .env("GIT_COMMITTER_NAME", NAME)
            .env("GIT_COMMITTER_EMAIL", EMAIL)
            .env("GIT_COMMITTER_DATE", DATE)
            .env("GIT_TERMINAL_PROMPT", "0");
    }

    /// 用真实 git 在临时目录创建一个带初始 commit 的仓库。
    fn make_repo() -> TempDir {
        let dir = TempDir::new().expect("create tempdir");

        let mut init = Command::new("git");
        init.args(["init", "-q"]).current_dir(dir.path());
        git_env(&mut init);
        assert!(
            init.status().expect("git init").success(),
            "git init 应成功"
        );

        std::fs::write(dir.path().join("README.md"), "hello pawork\n").expect("write readme");

        let mut add = Command::new("git");
        add.args(["add", "README.md"]).current_dir(dir.path());
        git_env(&mut add);
        assert!(add.status().expect("git add").success(), "git add 应成功");

        let mut commit = Command::new("git");
        commit
            .args(["commit", "-q", "-m", "init"])
            .current_dir(dir.path());
        git_env(&mut commit);
        assert!(
            commit.status().expect("git commit").success(),
            "git commit 应成功"
        );

        dir
    }

    #[tokio::test]
    async fn open_real_repo_and_inspect_head() {
        let dir = make_repo();
        let cancel = CancellationToken::new();

        let svc = GitService::open(dir.path(), cancel.clone())
            .await
            .expect("open 应成功");

        let branch = svc.current_branch(cancel.clone()).await.expect("branch");
        assert!(branch.is_some(), "应解析出分支名");
        assert!(!branch.as_ref().unwrap().is_empty(), "分支名应非空");

        let head = svc.current_head(cancel.clone()).await.expect("head");
        assert!(
            matches!(head, Head::Branch(ref name) if !name.is_empty()),
            "已提交仓库的 HEAD 应为 Branch，实际 = {head:?}"
        );

        let info = svc.repo_info(cancel).await.expect("repo_info");
        assert!(!info.bare, "非裸仓库");
        assert_eq!(info.head, head, "repo_info.head 应与 current_head 一致");
        assert!(info.work_dir.is_dir(), "work_dir 应存在");
        assert!(info.git_dir.is_absolute(), "git_dir 应为绝对路径");
    }

    #[tokio::test]
    async fn repo_info_uses_two_git_processes() {
        let dir = make_repo();
        let call_count = Arc::new(AtomicUsize::new(0));
        let svc = GitService {
            runner: GitRunner::new().with_call_count(call_count.clone()),
            work_dir: dir.path().to_path_buf(),
        };

        let info = svc
            .repo_info(CancellationToken::new())
            .await
            .expect("repo_info");
        assert!(matches!(info.head, Head::Branch(_)));
        assert_eq!(
            call_count.load(Ordering::Relaxed),
            2,
            "repo_info should use one combined rev-parse plus symbolic-ref"
        );
    }

    #[tokio::test]
    async fn repo_info_distinguishes_detached_and_unborn_head() {
        let detached = make_repo();
        let mut checkout = Command::new("git");
        checkout
            .args(["checkout", "-q", "--detach", "HEAD"])
            .current_dir(detached.path());
        git_env(&mut checkout);
        assert!(checkout.status().expect("detach HEAD").success());
        let detached_service = GitService::open(detached.path(), CancellationToken::new())
            .await
            .expect("open detached repo");
        let detached_info = detached_service
            .repo_info(CancellationToken::new())
            .await
            .expect("detached repo_info");
        assert!(matches!(detached_info.head, Head::Detached(ref sha) if sha.len() == 40));

        let unborn = TempDir::new().expect("unborn tempdir");
        let mut init = Command::new("git");
        init.args(["init", "-q"]).current_dir(unborn.path());
        git_env(&mut init);
        assert!(init.status().expect("init unborn repo").success());
        let unborn_service = GitService::open(unborn.path(), CancellationToken::new())
            .await
            .expect("open unborn repo");
        let unborn_info = unborn_service
            .repo_info(CancellationToken::new())
            .await
            .expect("unborn repo_info");
        assert_eq!(unborn_info.head, Head::Unborn);
    }

    #[tokio::test]
    async fn open_non_repo_returns_not_a_repository() {
        let dir = TempDir::new().expect("tempdir");
        let cancel = CancellationToken::new();
        let err = GitService::open(dir.path(), cancel)
            .await
            .expect_err("非仓库应报错");
        assert!(
            matches!(err, GitError::NotARepository(_)),
            "应为 NotARepository，实际 = {err:?}"
        );
    }
}
