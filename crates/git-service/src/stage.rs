//! stage / unstage / discard 操作（[`StageService`]）。
//!
//! - stage：`git add -- <paths>`；stage_all：`git add -A`。
//! - unstage：`git reset -q -- <paths>`。
//! - discard：`git checkout -- <paths>`，**会丢失工作区改动**，故 [`StageService::classify`]
//!   将 discard 标记为 [`StageRisk::Dangerous`]，供上层强制审批。
//!
//! 路径经 `git` 的 `--` 字面参数传入，天然防 shell 注入，绝不拼接 shell 字符串。

use std::path::{Path, PathBuf};

use agent_domain::CancellationToken;

use crate::error::GitError;
use crate::process::GitRunner;

/// stage 操作的写风险判定结果（供 UI 决定是否审批）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StageRisk {
    #[default]
    Safe,
    Dangerous,
}

/// stage 请求：相对 work_dir 的路径列表。
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StageRequest {
    pub paths: Vec<String>,
}

impl StageRequest {
    pub fn new(paths: Vec<String>) -> Self {
        Self { paths }
    }
}

/// 受支持的 stage 类操作。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StageOp {
    Stage,
    Unstage,
    Discard,
    StageAll,
}

/// stage / unstage / discard 服务。
pub struct StageService<'a> {
    runner: &'a GitRunner,
    work_dir: PathBuf,
}

impl<'a> StageService<'a> {
    pub fn new(runner: &'a GitRunner, work_dir: &Path) -> Self {
        Self {
            runner,
            work_dir: work_dir.to_path_buf(),
        }
    }

    /// 暂存：`git add -- <paths>`。空路径列表为无操作直接返回 Ok。
    pub async fn stage(
        &self,
        req: &StageRequest,
        cancel: CancellationToken,
    ) -> Result<(), GitError> {
        if req.paths.is_empty() {
            return Ok(());
        }
        self.run_path_op(&["add", "--"], &req.paths, cancel).await
    }

    /// 取消暂存：`git reset -q -- <paths>`。
    pub async fn unstage(
        &self,
        req: &StageRequest,
        cancel: CancellationToken,
    ) -> Result<(), GitError> {
        if req.paths.is_empty() {
            return Ok(());
        }
        self.run_path_op(&["reset", "-q", "--"], &req.paths, cancel)
            .await
    }

    /// 丢弃工作区改动：`git checkout -- <paths>`（高风险，会丢失未提交改动）。
    pub async fn discard(
        &self,
        req: &StageRequest,
        cancel: CancellationToken,
    ) -> Result<(), GitError> {
        if req.paths.is_empty() {
            return Ok(());
        }
        self.run_path_op(&["checkout", "--"], &req.paths, cancel)
            .await
    }

    /// 暂存全部：`git add -A`。
    pub async fn stage_all(&self, cancel: CancellationToken) -> Result<(), GitError> {
        self.runner
            .run(&self.work_dir, &["add", "-A"], cancel)
            .await?;
        Ok(())
    }

    /// 预判某操作的风险等级（不改状态）。discard→Dangerous；其余→Safe。
    pub fn classify(&self, op: StageOp) -> StageRisk {
        match op {
            StageOp::Discard => StageRisk::Dangerous,
            StageOp::Stage | StageOp::Unstage | StageOp::StageAll => StageRisk::Safe,
        }
    }

    /// 执行形如 `git <prefix...> -- <path1> <path2> ...` 的命令。
    async fn run_path_op(
        &self,
        prefix: &[&str],
        paths: &[String],
        cancel: CancellationToken,
    ) -> Result<(), GitError> {
        let mut args: Vec<String> = prefix.iter().map(|s| (*s).to_string()).collect();
        for p in paths {
            args.push(p.clone());
        }
        let arg_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        self.runner.run(&self.work_dir, &arg_refs, cancel).await?;
        Ok(())
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
        std::fs::write(repo.join("a.txt"), "line1\n").expect("write");
        run_git(&repo, &["add", "a.txt"]);
        run_git(&repo, &["commit", "-q", "-m", "init"]);
        (dir, repo)
    }

    /// porcelain v1 状态字符串（便于断言 index/worktree 列）。
    fn porcelain(cwd: &Path) -> String {
        run_git(cwd, &["status", "--porcelain"])
    }

    #[tokio::test]
    async fn stage_moves_change_to_index() {
        let (_dir, repo) = make_repo();
        let runner = GitRunner::new();
        let svc = StageService::new(&runner, &repo);
        // 改 a.txt（未暂存）。
        std::fs::write(repo.join("a.txt"), "line1\nline2\n").expect("write");
        assert_eq!(porcelain(&repo), " M a.txt\n");
        // 暂存后应变为 "M  a.txt"（index 列 M，worktree 列空格）。
        svc.stage(
            &StageRequest::new(vec!["a.txt".into()]),
            CancellationToken::new(),
        )
        .await
        .expect("stage");
        assert_eq!(porcelain(&repo), "M  a.txt\n");
    }

    #[tokio::test]
    async fn unstage_moves_change_back_to_worktree() {
        let (_dir, repo) = make_repo();
        let runner = GitRunner::new();
        let svc = StageService::new(&runner, &repo);
        std::fs::write(repo.join("a.txt"), "line1\nline2\n").expect("write");
        svc.stage(
            &StageRequest::new(vec!["a.txt".into()]),
            CancellationToken::new(),
        )
        .await
        .expect("stage");
        svc.unstage(
            &StageRequest::new(vec!["a.txt".into()]),
            CancellationToken::new(),
        )
        .await
        .expect("unstage");
        assert_eq!(porcelain(&repo), " M a.txt\n");
    }

    #[tokio::test]
    async fn stage_all_stages_multiple_files() {
        let (_dir, repo) = make_repo();
        let runner = GitRunner::new();
        let svc = StageService::new(&runner, &repo);
        std::fs::write(repo.join("a.txt"), "line1\nline2\n").expect("write");
        std::fs::write(repo.join("b.txt"), "new\n").expect("write");
        svc.stage_all(CancellationToken::new())
            .await
            .expect("stage_all");
        let p = porcelain(&repo);
        // 两文件均进入 index：M  a.txt、A  b.txt。
        assert!(p.contains("M  a.txt"), "{p}");
        assert!(p.contains("A  b.txt"), "{p}");
    }

    #[tokio::test]
    async fn discard_restores_file_to_head() {
        let (_dir, repo) = make_repo();
        let runner = GitRunner::new();
        let svc = StageService::new(&runner, &repo);
        std::fs::write(repo.join("a.txt"), "changed\n").expect("write");
        svc.discard(
            &StageRequest::new(vec!["a.txt".into()]),
            CancellationToken::new(),
        )
        .await
        .expect("discard");
        // 内容回到 HEAD（"line1\n"）。
        assert_eq!(
            std::fs::read_to_string(repo.join("a.txt")).unwrap(),
            "line1\n"
        );
        assert_eq!(porcelain(&repo), "");
    }

    #[test]
    fn classify_discard_is_dangerous_others_safe() {
        let runner = GitRunner::new();
        let svc = StageService::new(&runner, Path::new("."));
        assert_eq!(svc.classify(StageOp::Discard), StageRisk::Dangerous);
        assert_eq!(svc.classify(StageOp::Stage), StageRisk::Safe);
        assert_eq!(svc.classify(StageOp::Unstage), StageRisk::Safe);
        assert_eq!(svc.classify(StageOp::StageAll), StageRisk::Safe);
    }

    #[tokio::test]
    async fn empty_paths_is_noop() {
        let (_dir, repo) = make_repo();
        let runner = GitRunner::new();
        let svc = StageService::new(&runner, &repo);
        svc.stage(&StageRequest::new(vec![]), CancellationToken::new())
            .await
            .expect("noop stage");
        assert_eq!(porcelain(&repo), "");
    }
}
