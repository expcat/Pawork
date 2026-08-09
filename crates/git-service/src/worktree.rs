//! Git Worktree 管理（[`WorktreeService`]）。
//!
//! 封装 `git worktree` 的 list / add / remove / prune。**核心安全约束**：删除
//! 绝不触及用户数据——`remove` 必须先用 `list()` 校验目标是 git 管理的 worktree，
//! 校验失败直接返回错误；文件清理只交给 `git worktree remove`，绝不使用
//! `std::fs` 递归删除目录（ADR-007）。

use std::path::{Path, PathBuf};

use agent_domain::CancellationToken;

use crate::error::GitError;
use crate::process::{validate_position_arg, GitRunner};

/// 一个 Git Worktree。
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Worktree {
    /// 该 worktree 的工作树路径。
    pub path: PathBuf,
    /// checkout 的分支（短名），detached 时为 `None`。
    pub branch: Option<String>,
    /// 是否为 bare 仓库。
    pub bare: bool,
}

/// Worktree 管理服务。
pub struct WorktreeService<'a> {
    runner: &'a GitRunner,
    main_work_dir: PathBuf,
}

impl<'a> WorktreeService<'a> {
    pub fn new(runner: &'a GitRunner, main_work_dir: &Path) -> Self {
        Self {
            runner,
            main_work_dir: main_work_dir.to_path_buf(),
        }
    }

    /// 规范化路径用于与 git 返回的绝对路径比较（解析 symlink，如 macOS /var→/private/var）。
    fn canon(path: &Path) -> PathBuf {
        std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
    }

    /// 列出全部 worktree（含主工作树）。解析 `git worktree list --porcelain -z`。
    pub async fn list(&self, cancel: CancellationToken) -> Result<Vec<Worktree>, GitError> {
        let out = self
            .runner
            .run(
                &self.main_work_dir,
                &["worktree", "list", "--porcelain", "-z"],
                cancel,
            )
            .await?;
        Ok(parse_worktree_list(&out)
            .into_iter()
            .map(|mut wt| {
                wt.path = Self::canon(&wt.path);
                wt
            })
            .collect())
    }

    /// 创建新 worktree：`git worktree add -b <branch> <new_path> [start_point]`。
    /// `new_path` 已存在且非空则报错。
    pub async fn add(
        &self,
        new_path: &Path,
        branch_ref: &str,
        start_point: Option<&str>,
        cancel: CancellationToken,
    ) -> Result<Worktree, GitError> {
        validate_position_arg("branch", branch_ref)?;
        if let Some(start_point) = start_point {
            validate_position_arg("start_point", start_point)?;
        }
        let new_path_str = new_path
            .to_str()
            .ok_or_else(|| GitError::Other("worktree path is not valid UTF-8".into()))?;
        validate_position_arg("worktree_path", new_path_str)?;
        if new_path.exists() && path_is_non_empty(new_path) {
            return Err(GitError::Other(format!(
                "worktree target already exists and is non-empty: {}",
                new_path.display()
            )));
        }
        let mut args: Vec<String> = vec![
            "worktree".into(),
            "add".into(),
            "-b".into(),
            branch_ref.into(),
            new_path_str.into(),
        ];
        if let Some(sp) = start_point {
            args.push(sp.into());
        }
        let arg_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        self.runner
            .run(&self.main_work_dir, &arg_refs, cancel.clone())
            .await?;
        let target = Self::canon(new_path);
        self.list(cancel)
            .await?
            .into_iter()
            .find(|wt| wt.path == target)
            .ok_or_else(|| GitError::Other("worktree was added but not found in listing".into()))
    }

    /// 删除 worktree。`force=true` 时加 `--force`。先 `list()` 校验为受管理 worktree，
    /// 否则直接报错；绝不调用 std::fs 删除用户数据。
    pub async fn remove(
        &self,
        path: &Path,
        force: bool,
        cancel: CancellationToken,
    ) -> Result<(), GitError> {
        let path_str = path
            .to_str()
            .ok_or_else(|| GitError::Other("worktree path is not valid UTF-8".into()))?;
        validate_position_arg("worktree_path", path_str)?;
        let target = Self::canon(path);
        let managed = self.list(cancel.clone()).await?;
        if !managed.iter().any(|wt| wt.path == target) {
            return Err(GitError::Other(format!(
                "path is not a managed worktree: {}",
                path.display()
            )));
        }
        let mut args: Vec<String> = vec!["worktree".into(), "remove".into()];
        if force {
            args.push("--force".into());
        }
        args.push(path_str.into());
        let arg_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        self.runner
            .run(&self.main_work_dir, &arg_refs, cancel.clone())
            .await?;
        Ok(())
    }

    /// 清理 worktree 元数据（`git worktree prune`），不动文件。
    pub async fn prune(&self, cancel: CancellationToken) -> Result<(), GitError> {
        self.runner
            .run(&self.main_work_dir, &["worktree", "prune"], cancel)
            .await?;
        Ok(())
    }
}

/// 解析 `git worktree list --porcelain -z` 输出。
///
/// `-z` 模式下每个字段（`worktree`/`HEAD`/`branch`/`detached`/`bare`）均以 NUL 结尾，
/// 条目间额外多一个空 NUL。这里按 NUL 切分所有 token，遇 `worktree ` 前缀即开新条目。
fn parse_worktree_list(out: &str) -> Vec<Worktree> {
    let mut worktrees = Vec::new();
    let mut current: Option<Worktree> = None;
    for token in out.split('\0') {
        if token.is_empty() {
            // 空段：条目分隔。把累积中的条目收尾。
            if let Some(wt) = current.take() {
                worktrees.push(wt);
            }
            continue;
        }
        if let Some(rest) = token.strip_prefix("worktree ") {
            // 新条目开始：先把上一个收尾。
            if let Some(wt) = current.take() {
                worktrees.push(wt);
            }
            current = Some(Worktree {
                path: PathBuf::from(rest),
                branch: None,
                bare: false,
            });
        } else if token == "bare" {
            if let Some(wt) = current.as_mut() {
                wt.bare = true;
            }
        } else if let Some(rest) = token.strip_prefix("branch ") {
            if let Some(wt) = current.as_mut() {
                wt.branch = Some(strip_branch_prefix(rest).to_string());
            }
        }
        // HEAD / detached 等字段不改变对外暴露的字段，忽略。
    }
    // 收尾最后一个条目。
    if let Some(wt) = current.take() {
        worktrees.push(wt);
    }
    worktrees
}

/// `refs/heads/main` → `main`，已是短名则原样返回。
fn strip_branch_prefix(s: &str) -> &str {
    s.strip_prefix("refs/heads/").unwrap_or(s)
}

/// 目录非空判定（仅用于 add 前置检查）。
fn path_is_non_empty(path: &Path) -> bool {
    path.read_dir()
        .map(|mut it| it.next().is_some())
        .unwrap_or(false)
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
        std::fs::write(repo.join("README.md"), "hello\n").expect("write");
        run_git(&repo, &["add", "README.md"]);
        run_git(&repo, &["commit", "-q", "-m", "init"]);
        (dir, repo)
    }

    #[tokio::test]
    async fn add_then_list_then_remove() {
        let (_dir, repo) = make_repo();
        let runner = GitRunner::new();
        let svc = WorktreeService::new(&runner, &repo);
        let wt_dir = repo.join("linked-wt");
        let wt = svc
            .add(&wt_dir, "feature-x", None, CancellationToken::new())
            .await
            .expect("add");
        assert_eq!(wt.path, std::fs::canonicalize(&wt_dir).unwrap());
        assert_eq!(wt.branch.as_deref(), Some("feature-x"));
        let listed = svc.list(CancellationToken::new()).await.expect("list");
        let canon_wt_dir = std::fs::canonicalize(&wt_dir).unwrap();
        assert!(listed.iter().any(|w| w.path == canon_wt_dir));
        svc.remove(&wt_dir, false, CancellationToken::new())
            .await
            .expect("remove");
        let listed = svc.list(CancellationToken::new()).await.expect("list2");
        assert!(!listed.iter().any(|w| w.path == wt_dir));
    }

    #[tokio::test]
    async fn remove_dirty_worktree_without_force_fails_and_keeps_data() {
        let (_dir, repo) = make_repo();
        let runner = GitRunner::new();
        let svc = WorktreeService::new(&runner, &repo);
        let wt_dir = repo.join("dirty-wt");
        let _ = svc
            .add(&wt_dir, "feature-dirty", None, CancellationToken::new())
            .await
            .expect("add");
        let untracked = wt_dir.join("untracked.txt");
        std::fs::write(&untracked, "user data\n").expect("write");
        let res = svc.remove(&wt_dir, false, CancellationToken::new()).await;
        assert!(res.is_err(), "remove should fail on dirty worktree");
        assert!(untracked.exists(), "untracked file must be preserved");
        assert_eq!(
            std::fs::read_to_string(&untracked).expect("read"),
            "user data\n"
        );
    }

    #[tokio::test]
    async fn remove_non_managed_path_returns_error_and_keeps_dir() {
        let (_dir, repo) = make_repo();
        let runner = GitRunner::new();
        let svc = WorktreeService::new(&runner, &repo);
        let outside = repo.join("not-a-worktree");
        std::fs::create_dir_all(&outside).expect("mkdir");
        std::fs::write(outside.join("keep.txt"), "keep\n").expect("write");
        let res = svc.remove(&outside, false, CancellationToken::new()).await;
        assert!(res.is_err(), "removing non-managed path must error");
        assert!(outside.is_dir());
        assert!(outside.join("keep.txt").exists());
    }

    #[tokio::test]
    async fn option_like_worktree_refs_are_rejected() {
        let runner = GitRunner::new();
        let svc = WorktreeService::new(&runner, Path::new("."));

        let branch_error = svc
            .add(
                Path::new("unused-worktree"),
                "--detach",
                None,
                CancellationToken::new(),
            )
            .await
            .expect_err("option-like branch must be rejected");
        assert!(matches!(
            branch_error,
            GitError::InvalidPositionArgument { name: "branch", .. }
        ));

        let start_error = svc
            .add(
                Path::new("unused-worktree"),
                "safe",
                Some("--checkout"),
                CancellationToken::new(),
            )
            .await
            .expect_err("option-like start point must be rejected");
        assert!(matches!(
            start_error,
            GitError::InvalidPositionArgument {
                name: "start_point",
                ..
            }
        ));

        let add_path_error = svc
            .add(Path::new("--force"), "safe", None, CancellationToken::new())
            .await
            .expect_err("option-like worktree path must be rejected");
        assert!(matches!(
            add_path_error,
            GitError::InvalidPositionArgument {
                name: "worktree_path",
                ..
            }
        ));

        let remove_path_error = svc
            .remove(Path::new("--force"), false, CancellationToken::new())
            .await
            .expect_err("option-like remove path must be rejected");
        assert!(matches!(
            remove_path_error,
            GitError::InvalidPositionArgument {
                name: "worktree_path",
                ..
            }
        ));
    }
}
