//! branch / checkout 操作（[`BranchService`]）。
//!
//! - create：`git branch <name> [<start_point>]`（可 `-f` 强制覆盖）。
//! - delete：`git branch -d|-D <name>`；未合并的分支默认拒绝删除。
//! - checkout：`git checkout <branch>`；checkout_new：`git checkout -b`。
//!
//! 常见失败归一：分支已存在 → [`GitError::BranchAlreadyExists`]；分支不存在 →
//! [`GitError::BranchNotFound`] / [`GitError::ReferenceNotFound`]；未合并删除 →
//! [`GitError::BranchNotMerged`]；本地改动会被覆盖 →
//! [`GitError::LocalChangesWouldBeOverwritten`]（并从 stderr 解析路径列表）。

use std::path::{Path, PathBuf};

use pawork_domain::CancellationToken;

use crate::error::GitError;
use crate::process::{validate_position_arg, GitRunner};

/// branch 创建/删除/切换服务。
pub struct BranchService<'a> {
    runner: &'a GitRunner,
    work_dir: PathBuf,
}

impl<'a> BranchService<'a> {
    pub fn new(runner: &'a GitRunner, work_dir: &Path) -> Self {
        Self {
            runner,
            work_dir: work_dir.to_path_buf(),
        }
    }

    /// 创建分支：`git branch <name> [<start_point>]`。
    pub async fn create(
        &self,
        name: &str,
        start_point: Option<&str>,
        force: bool,
        cancel: CancellationToken,
    ) -> Result<(), GitError> {
        validate_position_arg("branch", name)?;
        if let Some(start_point) = start_point {
            validate_position_arg("start_point", start_point)?;
        }
        let mut args: Vec<String> = vec!["branch".into()];
        if force {
            args.push("-f".into());
        }
        args.push(name.to_string());
        if let Some(sp) = start_point {
            args.push(sp.to_string());
        }
        let arg_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        match self.runner.run(&self.work_dir, &arg_refs, cancel).await {
            Ok(_) => Ok(()),
            Err(GitError::GitFailed { ref stderr, .. }) => {
                if stderr.contains("already exists") {
                    Err(GitError::BranchAlreadyExists(name.to_string()))
                } else if stderr.contains("not a valid branch name")
                    || stderr.contains("unknown revision")
                {
                    Err(GitError::ReferenceNotFound(name.to_string()))
                } else {
                    Err(GitError::GitFailed {
                        code: None,
                        stderr: stderr.clone(),
                    })
                }
            }
            Err(other) => Err(other),
        }
    }

    /// 删除分支：`git branch -d <name>`；`force` 时用 `-D`（未合并也删）。
    pub async fn delete(
        &self,
        name: &str,
        force: bool,
        cancel: CancellationToken,
    ) -> Result<(), GitError> {
        validate_position_arg("branch", name)?;
        let flag = if force { "-D" } else { "-d" };
        match self
            .runner
            .run(&self.work_dir, &["branch", flag, name], cancel)
            .await
        {
            Ok(_) => Ok(()),
            Err(GitError::GitFailed { ref stderr, .. }) => {
                if stderr.contains("not found") {
                    Err(GitError::BranchNotFound(name.to_string()))
                } else if stderr.contains("not fully merged") {
                    Err(GitError::BranchNotMerged(name.to_string()))
                } else {
                    Err(GitError::GitFailed {
                        code: None,
                        stderr: stderr.clone(),
                    })
                }
            }
            Err(other) => Err(other),
        }
    }

    /// 切换到已有分支：`git checkout <name>`。
    pub async fn checkout(&self, name: &str, cancel: CancellationToken) -> Result<(), GitError> {
        validate_position_arg("branch", name)?;
        match self
            .runner
            .run(&self.work_dir, &["checkout", name], cancel)
            .await
        {
            Ok(_) => Ok(()),
            Err(GitError::GitFailed { ref stderr, .. }) => {
                if let Some(paths) = parse_overwritten_paths(stderr) {
                    return Err(GitError::LocalChangesWouldBeOverwritten(paths));
                }
                if stderr.contains("did not match any file(s)") || stderr.contains("pathspec") {
                    return Err(GitError::ReferenceNotFound(name.to_string()));
                }
                if stderr.contains("is not a commit") || stderr.contains("unknown revision") {
                    return Err(GitError::ReferenceNotFound(name.to_string()));
                }
                Err(GitError::GitFailed {
                    code: None,
                    stderr: stderr.clone(),
                })
            }
            Err(other) => Err(other),
        }
    }

    /// 创建并切换：`git checkout -b <name> [<start_point>]`。
    pub async fn checkout_new(
        &self,
        name: &str,
        start_point: Option<&str>,
        force: bool,
        cancel: CancellationToken,
    ) -> Result<(), GitError> {
        validate_position_arg("branch", name)?;
        if let Some(start_point) = start_point {
            validate_position_arg("start_point", start_point)?;
        }
        let mut args: Vec<String> = vec!["checkout".into()];
        args.push(if force { "-B".into() } else { "-b".into() });
        args.push(name.to_string());
        if let Some(sp) = start_point {
            args.push(sp.to_string());
        }
        let arg_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        match self.runner.run(&self.work_dir, &arg_refs, cancel).await {
            Ok(_) => Ok(()),
            Err(GitError::GitFailed { ref stderr, .. }) => {
                if stderr.contains("already exists") {
                    return Err(GitError::BranchAlreadyExists(name.to_string()));
                }
                if let Some(paths) = parse_overwritten_paths(stderr) {
                    return Err(GitError::LocalChangesWouldBeOverwritten(paths));
                }
                Err(GitError::GitFailed {
                    code: None,
                    stderr: stderr.clone(),
                })
            }
            Err(other) => Err(other),
        }
    }
}

/// 从 checkout 失败 stderr 中解析「会被覆盖的本地改动」路径列表。
///
/// git 输出形如：
/// ```text
/// error: Your local changes to the following files would be overwritten by checkout:
/// \ta.txt
/// \tsub/b.txt
/// Please commit your changes or stash them before you switch branches.
/// ```
fn parse_overwritten_paths(stderr: &str) -> Option<Vec<String>> {
    if !stderr.contains("would be overwritten") {
        return None;
    }
    let mut paths = Vec::new();
    let mut in_list = false;
    for line in stderr.lines() {
        if line.contains("would be overwritten") {
            in_list = true;
            continue;
        }
        if !in_list {
            continue;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with("Please") || trimmed.starts_with("Aborting") {
            break;
        }
        paths.push(trimmed.to_string());
    }
    if paths.is_empty() {
        None
    } else {
        Some(paths)
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

    fn current_branch(cwd: &Path) -> String {
        run_git(cwd, &["symbolic-ref", "--short", "HEAD"])
            .trim()
            .to_string()
    }

    #[tokio::test]
    async fn create_and_checkout_branch() {
        let (_dir, repo) = make_repo();
        let runner = GitRunner::new();
        let svc = BranchService::new(&runner, &repo);
        svc.create("feature", None, false, CancellationToken::new())
            .await
            .expect("create");
        svc.checkout("feature", CancellationToken::new())
            .await
            .expect("checkout");
        assert_eq!(current_branch(&repo), "feature");
    }

    #[tokio::test]
    async fn create_existing_branch_is_error() {
        let (_dir, repo) = make_repo();
        let runner = GitRunner::new();
        let svc = BranchService::new(&runner, &repo);
        svc.create("dup", None, false, CancellationToken::new())
            .await
            .expect("create");
        let err = svc
            .create("dup", None, false, CancellationToken::new())
            .await
            .expect_err("重复创建应报错");
        assert!(
            matches!(err, GitError::BranchAlreadyExists(ref n) if n == "dup"),
            "err = {err:?}"
        );
    }

    #[tokio::test]
    async fn checkout_missing_branch_is_reference_not_found() {
        let (_dir, repo) = make_repo();
        let runner = GitRunner::new();
        let svc = BranchService::new(&runner, &repo);
        let err = svc
            .checkout("no-such-branch", CancellationToken::new())
            .await
            .expect_err("不存在的分支应报错");
        assert!(
            matches!(err, GitError::ReferenceNotFound(_)),
            "err = {err:?}"
        );
    }

    #[tokio::test]
    async fn checkout_with_local_changes_maps_to_overwritten() {
        let (_dir, repo) = make_repo();
        let runner = GitRunner::new();
        let svc = BranchService::new(&runner, &repo);
        let base = current_branch(&repo);
        // other 分支上同一路径有不同内容。
        svc.checkout_new("other", None, false, CancellationToken::new())
            .await
            .expect("checkout_new");
        std::fs::write(repo.join("a.txt"), "other content\n").expect("write");
        run_git(&repo, &["add", "a.txt"]);
        run_git(&repo, &["commit", "-q", "-m", "other change"]);
        // 回到基准分支并制造未提交改动（与 other 上的 a.txt 内容不同）。
        svc.checkout(&base, CancellationToken::new())
            .await
            .expect("back to base");
        std::fs::write(repo.join("a.txt"), "local uncommitted\n").expect("write");
        let err = svc
            .checkout("other", CancellationToken::new())
            .await
            .expect_err("带本地改动 checkout 冲突分支应报错");
        assert!(
            matches!(err, GitError::LocalChangesWouldBeOverwritten(ref paths) if paths.iter().any(|p| p.contains("a.txt"))),
            "err = {err:?}"
        );
    }

    #[tokio::test]
    async fn delete_unmerged_branch_requires_force() {
        let (_dir, repo) = make_repo();
        let runner = GitRunner::new();
        let svc = BranchService::new(&runner, &repo);
        let base = current_branch(&repo);
        svc.checkout_new("side", None, false, CancellationToken::new())
            .await
            .expect("checkout_new");
        std::fs::write(repo.join("b.txt"), "side\n").expect("write");
        run_git(&repo, &["add", "b.txt"]);
        run_git(&repo, &["commit", "-q", "-m", "side commit"]);
        svc.checkout(&base, CancellationToken::new())
            .await
            .expect("back to base");
        // side 未合并进 base：-d 应拒绝。
        let err = svc
            .delete("side", false, CancellationToken::new())
            .await
            .expect_err("未合并分支 -d 应报错");
        assert!(
            matches!(err, GitError::BranchNotMerged(ref n) if n == "side"),
            "err = {err:?}"
        );
        // -D 强制删除成功。
        svc.delete("side", true, CancellationToken::new())
            .await
            .expect("force delete");
    }

    #[tokio::test]
    async fn delete_missing_branch_is_not_found() {
        let (_dir, repo) = make_repo();
        let runner = GitRunner::new();
        let svc = BranchService::new(&runner, &repo);
        let err = svc
            .delete("ghost", false, CancellationToken::new())
            .await
            .expect_err("删除不存在的分支应报错");
        assert!(
            matches!(err, GitError::BranchNotFound(ref n) if n == "ghost"),
            "err = {err:?}"
        );
    }

    #[test]
    fn parse_overwritten_paths_extracts_indented_list() {
        let stderr =
            "error: Your local changes to the following files would be overwritten by checkout:\n\
\ta.txt\n\
\tsub/b.txt\n\
Please commit your changes or stash them before you switch branches.\nAborting";
        let paths = parse_overwritten_paths(stderr).expect("should parse");
        assert_eq!(paths, vec!["a.txt".to_string(), "sub/b.txt".to_string()]);
        assert!(parse_overwritten_paths("some other error").is_none());
    }

    #[tokio::test]
    async fn option_like_branch_arguments_are_rejected_at_service_boundaries() {
        let runner = GitRunner::new();
        let svc = BranchService::new(&runner, Path::new("."));

        let errors = [
            svc.create("--help", None, false, CancellationToken::new())
                .await
                .expect_err("create branch name"),
            svc.create("safe", Some("--help"), false, CancellationToken::new())
                .await
                .expect_err("create start point"),
            svc.delete("--merged", false, CancellationToken::new())
                .await
                .expect_err("delete branch name"),
            svc.checkout("-b", CancellationToken::new())
                .await
                .expect_err("checkout branch name"),
            svc.checkout_new("--orphan", None, false, CancellationToken::new())
                .await
                .expect_err("checkout_new branch name"),
            svc.checkout_new("safe", Some("--detach"), false, CancellationToken::new())
                .await
                .expect_err("checkout_new start point"),
        ];

        assert!(errors
            .iter()
            .all(|error| matches!(error, GitError::InvalidPositionArgument { .. })));
    }
}
