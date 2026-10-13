//! Git 仓库检测与 HEAD 状态（[`GitService`]）。
//!
//! [`GitService::open`] 定位 work tree，[`GitService::current_head`] 解析 HEAD。
//! status / worktree 模块复用 [`crate::process::GitRunner`]。

use std::path::{Path, PathBuf};

use pawork_domain::CancellationToken;

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

    /// 解析 HEAD 状态。
    ///
    /// `symbolic-ref --short HEAD` 给出分支名；`rev-parse --verify HEAD` 判断该
    /// 提交是否存在。有名且可解析 → `Branch`；有名但 HEAD 不存在 → `Unborn`；
    /// 无名且可解析 → `Detached`；两者都失败 → `Unborn`。
    pub async fn current_head(&self, cancel: CancellationToken) -> Result<Head, GitError> {
        let branch_name = match self
            .runner
            .run_with_stderr(
                self.work_dir(),
                &["symbolic-ref", "--short", "HEAD"],
                cancel.clone(),
            )
            .await
        {
            Ok((stdout, _)) => {
                let name = stdout.trim();
                if name.is_empty() {
                    None
                } else {
                    Some(name.to_string())
                }
            }
            Err(GitError::GitFailed { .. }) => None,
            Err(other) => return Err(other),
        };

        match self
            .runner
            .run(self.work_dir(), &["rev-parse", "--verify", "HEAD"], cancel)
            .await
        {
            Ok(rev) => {
                let sha = rev.trim();
                if sha.is_empty() {
                    Ok(Head::Unborn)
                } else if let Some(name) = branch_name {
                    Ok(Head::Branch(name))
                } else {
                    Ok(Head::Detached(sha.to_string()))
                }
            }
            Err(GitError::GitFailed { .. }) => Ok(Head::Unborn),
            Err(other) => Err(other),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
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

        let head = svc.current_head(cancel).await.expect("head");
        assert!(
            matches!(head, Head::Branch(ref name) if !name.is_empty()),
            "已提交仓库的 HEAD 应为 Branch，实际 = {head:?}"
        );
    }

    #[tokio::test]
    async fn current_head_distinguishes_detached_and_unborn() {
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
        let detached_head = detached_service
            .current_head(CancellationToken::new())
            .await
            .expect("detached current_head");
        assert!(matches!(detached_head, Head::Detached(ref sha) if sha.len() == 40));

        let unborn = TempDir::new().expect("unborn tempdir");
        let mut init = Command::new("git");
        init.args(["init", "-q"]).current_dir(unborn.path());
        git_env(&mut init);
        assert!(init.status().expect("init unborn repo").success());
        let unborn_service = GitService::open(unborn.path(), CancellationToken::new())
            .await
            .expect("open unborn repo");
        let unborn_head = unborn_service
            .current_head(CancellationToken::new())
            .await
            .expect("unborn current_head");
        assert_eq!(unborn_head, Head::Unborn);
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
