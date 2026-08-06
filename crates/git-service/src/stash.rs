//! stash 操作（[`StashService`]）。
//!
//! - push：`git stash push [-u] [-m <msg>] [-- <paths>]`；无可暂存内容时返回
//!   [`StashPushOutcome::NoChanges`]（git 输出 "No local changes to save"）。
//! - list：`git stash list --pretty=format:%gd<US>%gs` 解析出序号与说明。
//! - pop / apply / drop：作用于 `stash@{<index>}`；pop/apply 产生冲突时归一为
//!   [`GitError::Conflict`]，序号不存在归一为 [`GitError::ReferenceNotFound`]。

use std::path::{Path, PathBuf};

use agent_domain::CancellationToken;

use crate::error::GitError;
use crate::process::GitRunner;

/// stash push 的结果。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StashPushOutcome {
    /// 新建了一条 stash。
    Created,
    /// 没有可保存的本地改动（"No local changes to save"）。
    NoChanges,
}

/// 一条 stash 记录。
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StashEntry {
    /// `stash@{<index>}` 中的序号（0 为最新）。
    pub index: u32,
    /// stash 说明（默认形如 `WIP on <branch>: <sha> <subject>`）。
    pub message: String,
}

/// stash 服务。
pub struct StashService<'a> {
    runner: &'a GitRunner,
    work_dir: PathBuf,
}

impl<'a> StashService<'a> {
    pub fn new(runner: &'a GitRunner, work_dir: &Path) -> Self {
        Self {
            runner,
            work_dir: work_dir.to_path_buf(),
        }
    }

    /// 保存当前本地改动到 stash。
    pub async fn push(
        &self,
        message: Option<&str>,
        include_untracked: bool,
        paths: &[String],
        cancel: CancellationToken,
    ) -> Result<StashPushOutcome, GitError> {
        let mut args: Vec<String> = vec!["stash".into(), "push".into()];
        if include_untracked {
            args.push("-u".into());
        }
        if let Some(msg) = message {
            args.push("-m".into());
            args.push(msg.to_string());
        }
        if !paths.is_empty() {
            args.push("--".into());
            args.extend(paths.iter().cloned());
        }
        let arg_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        let stdout = self.runner.run(&self.work_dir, &arg_refs, cancel).await?;
        if stdout.contains("No local changes to save") {
            Ok(StashPushOutcome::NoChanges)
        } else {
            Ok(StashPushOutcome::Created)
        }
    }

    /// 列出全部 stash（新→旧）。
    pub async fn list(&self, cancel: CancellationToken) -> Result<Vec<StashEntry>, GitError> {
        let stdout = self
            .runner
            .run(
                &self.work_dir,
                &["stash", "list", "--pretty=format:%gd%x1f%gs"],
                cancel,
            )
            .await?;
        Ok(parse_stash_list(&stdout))
    }

    /// 弹出指定 stash（应用并删除）；冲突归一为 [`GitError::Conflict`]。
    pub async fn pop(&self, index: u32, cancel: CancellationToken) -> Result<(), GitError> {
        self.run_stash_op("pop", index, cancel).await
    }

    /// 应用指定 stash（保留记录）。
    pub async fn apply(&self, index: u32, cancel: CancellationToken) -> Result<(), GitError> {
        self.run_stash_op("apply", index, cancel).await
    }

    /// 删除指定 stash 记录。
    pub async fn drop(&self, index: u32, cancel: CancellationToken) -> Result<(), GitError> {
        self.run_stash_op("drop", index, cancel).await
    }

    async fn run_stash_op(
        &self,
        op: &str,
        index: u32,
        cancel: CancellationToken,
    ) -> Result<(), GitError> {
        let reference = format!("stash@{{{index}}}");
        match self
            .runner
            .run(&self.work_dir, &["stash", op, &reference], cancel)
            .await
        {
            Ok(_) => Ok(()),
            Err(GitError::GitFailed { code, stderr }) => {
                if stderr.contains("CONFLICT") {
                    return Err(GitError::Conflict(stderr.trim().to_string()));
                }
                // pop/apply 冲突时 CONFLICT 明细走 stdout，GitError 只剩空
                // stderr：退出码 1 + 空 stderr 归一为冲突。
                if matches!(op, "pop" | "apply") && code == Some(1) && stderr.trim().is_empty() {
                    return Err(GitError::Conflict(format!(
                        "stash {op} produced conflicts ({reference})"
                    )));
                }
                if stderr.contains("does not exist")
                    || stderr.contains("not found")
                    || stderr.contains("not a valid reference")
                {
                    return Err(GitError::ReferenceNotFound(reference));
                }
                Err(GitError::GitFailed { code, stderr })
            }
            Err(other) => Err(other),
        }
    }
}

/// 解析 `git stash list --pretty=format:%gd<US>%gs` 输出。
fn parse_stash_list(stdout: &str) -> Vec<StashEntry> {
    let mut entries = Vec::new();
    for line in stdout.lines() {
        let line = line.trim_end_matches('\r');
        if line.is_empty() {
            continue;
        }
        let (gd, message) = line.split_once('\x1f').unwrap_or((line, ""));
        // gd 形如 `stash@{0}`：取最后花括号内数字。
        let index = gd
            .rsplit_once('{')
            .and_then(|(_, rest)| rest.trim_end_matches('}').parse::<u32>().ok())
            .unwrap_or(0);
        entries.push(StashEntry {
            index,
            message: message.to_string(),
        });
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
    async fn push_list_pop_roundtrip() {
        let (_dir, repo) = make_repo();
        let runner = GitRunner::new();
        let svc = StashService::new(&runner, &repo);
        std::fs::write(repo.join("a.txt"), "dirty\n").expect("write");
        let out = svc
            .push(Some("wip-a"), false, &[], CancellationToken::new())
            .await
            .expect("push");
        assert_eq!(out, StashPushOutcome::Created);
        // 工作区已还原。
        assert_eq!(
            std::fs::read_to_string(repo.join("a.txt")).unwrap(),
            "line1\n"
        );
        let list = svc.list(CancellationToken::new()).await.expect("list");
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].index, 0);
        assert!(list[0].message.contains("wip-a"), "{:?}", list);
        svc.pop(0, CancellationToken::new()).await.expect("pop");
        assert_eq!(
            std::fs::read_to_string(repo.join("a.txt")).unwrap(),
            "dirty\n"
        );
        let list = svc.list(CancellationToken::new()).await.expect("list");
        assert!(list.is_empty());
    }

    #[tokio::test]
    async fn push_without_changes_is_no_changes() {
        let (_dir, repo) = make_repo();
        let runner = GitRunner::new();
        let svc = StashService::new(&runner, &repo);
        let out = svc
            .push(None, false, &[], CancellationToken::new())
            .await
            .expect("push");
        assert_eq!(out, StashPushOutcome::NoChanges);
    }

    #[tokio::test]
    async fn push_with_paths_stashes_only_listed() {
        let (_dir, repo) = make_repo();
        let runner = GitRunner::new();
        let svc = StashService::new(&runner, &repo);
        // b.txt 先提交为已跟踪文件（按路径 stash 会把路径还原到 HEAD）。
        std::fs::write(repo.join("b.txt"), "orig-b\n").expect("write");
        run_git(&repo, &["add", "b.txt"]);
        run_git(&repo, &["commit", "-q", "-m", "add b"]);
        std::fs::write(repo.join("a.txt"), "dirty-a\n").expect("write");
        std::fs::write(repo.join("b.txt"), "dirty-b\n").expect("write");
        let out = svc
            .push(
                None,
                false,
                &["b.txt".to_string()],
                CancellationToken::new(),
            )
            .await
            .expect("push paths");
        assert_eq!(out, StashPushOutcome::Created);
        // a.txt 的改动保留在工作区，b.txt 还原到 HEAD。
        assert_eq!(
            std::fs::read_to_string(repo.join("a.txt")).unwrap(),
            "dirty-a\n"
        );
        assert_eq!(
            std::fs::read_to_string(repo.join("b.txt")).unwrap(),
            "orig-b\n"
        );
    }

    #[tokio::test]
    async fn apply_keeps_entry_drop_removes_it() {
        let (_dir, repo) = make_repo();
        let runner = GitRunner::new();
        let svc = StashService::new(&runner, &repo);
        std::fs::write(repo.join("a.txt"), "dirty\n").expect("write");
        svc.push(None, false, &[], CancellationToken::new())
            .await
            .expect("push");
        svc.apply(0, CancellationToken::new()).await.expect("apply");
        assert_eq!(
            std::fs::read_to_string(repo.join("a.txt")).unwrap(),
            "dirty\n"
        );
        let list = svc.list(CancellationToken::new()).await.expect("list");
        assert_eq!(list.len(), 1, "apply 不应删除记录");
        svc.drop(0, CancellationToken::new()).await.expect("drop");
        let list = svc.list(CancellationToken::new()).await.expect("list");
        assert!(list.is_empty());
    }

    #[tokio::test]
    async fn pop_missing_index_is_reference_not_found() {
        let (_dir, repo) = make_repo();
        let runner = GitRunner::new();
        let svc = StashService::new(&runner, &repo);
        let err = svc
            .pop(3, CancellationToken::new())
            .await
            .expect_err("空 stash 列表 pop 应报错");
        assert!(
            matches!(err, GitError::ReferenceNotFound(_)),
            "err = {err:?}"
        );
    }

    #[tokio::test]
    async fn pop_with_conflict_maps_to_conflict() {
        let (_dir, repo) = make_repo();
        let runner = GitRunner::new();
        let svc = StashService::new(&runner, &repo);
        // stash 里的改动与工作区当前改动冲突。
        std::fs::write(repo.join("a.txt"), "stash version\n").expect("write");
        svc.push(None, false, &[], CancellationToken::new())
            .await
            .expect("push");
        std::fs::write(repo.join("a.txt"), "conflicting version\n").expect("write");
        run_git(&repo, &["add", "a.txt"]);
        run_git(&repo, &["commit", "-q", "-m", "conflicting change"]);
        let err = svc
            .pop(0, CancellationToken::new())
            .await
            .expect_err("冲突 pop 应报错");
        assert!(matches!(err, GitError::Conflict(_)), "err = {err:?}");
    }

    #[test]
    fn parse_stash_list_parses_indices() {
        let stdout = "stash@{0}\x1fWIP on main: abc init\nstash@{1}\x1fon feature: def x\n";
        let entries = parse_stash_list(stdout);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].index, 0);
        assert_eq!(entries[1].index, 1);
        assert!(entries[0].message.starts_with("WIP on main"));
    }
}
