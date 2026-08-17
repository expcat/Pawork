//! Git 工作区状态解析（[`StatusService`] / [`read_status`]）。
//!
//! 通过 `git status --porcelain=v1 -z --untracked-files=all` 获取机器可解析的
//! NUL 分隔输出，解析为 [`FileChange`] 列表，并归一 index / worktree 两列状态码。
//! rename / copy 条目附带原始路径（`previous_path`）。

use std::path::{Path, PathBuf};

use pawork_domain::CancellationToken;

use crate::error::GitError;
use crate::process::GitRunner;

/// 单个文件的状态（对应 porcelain 的单字符状态码）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileStatus {
    /// 未修改（' '）。
    #[default]
    Unmodified,
    /// 新增（'A'，已暂存）。
    Added,
    /// 修改（'M'）。
    Modified,
    /// 删除（'D'）。
    Deleted,
    /// 重命名（'R'）。
    Renamed,
    /// 复制（'C'）。
    Copied,
    /// 冲突 / 未合并（'U'）。
    Unmerged,
    /// 类型变更（'T'，如文件↔符号链接）。
    TypeChanged,
    /// 未跟踪（'?'）。
    Untracked,
}

/// 单个文件的变更描述。
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct FileChange {
    /// 当前路径。
    pub path: String,
    /// rename / copy 时的原始路径。
    pub previous_path: Option<String>,
    /// 暂存区（index）状态，对应 porcelain 的 X 列。
    pub index_status: FileStatus,
    /// 工作区（worktree）状态，对应 porcelain 的 Y 列。
    pub worktree_status: FileStatus,
    /// 便捷标记：整条为 "??" 即未跟踪。
    pub untracked: bool,
}

/// 一次 status 快照（全部变更条目）。
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StatusSnapshot {
    /// 变更条目列表（已暂存 + 未暂存 + 未跟踪）。
    pub changes: Vec<FileChange>,
}

/// 对单个仓库工作区做 status 解析的服务。
///
/// 持有 [`GitRunner`] 引用与 work_dir，[`StatusService::status`] /
/// [`StatusService::changed_files`] 复用 runner 调用系统 git。
pub struct StatusService<'a> {
    runner: &'a GitRunner,
    work_dir: PathBuf,
}

impl<'a> StatusService<'a> {
    /// 构造服务。
    pub fn new(runner: &'a GitRunner, work_dir: &Path) -> Self {
        Self {
            runner,
            work_dir: work_dir.to_path_buf(),
        }
    }

    /// 完整 status（已暂存 + 未暂存 + 未跟踪）。
    ///
    /// 实现：`git status --porcelain=v1 -z --untracked-files=all`，按 NUL 分隔
    /// 解析。X 为 index 状态、Y 为 worktree 状态；"??" 为未跟踪；rename / copy
    /// 条目 path 后跟独立的 NUL 再接原始路径。
    pub async fn status(&self, cancel: CancellationToken) -> Result<StatusSnapshot, GitError> {
        let output = self
            .runner
            .run(
                &self.work_dir,
                &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
                cancel,
            )
            .await?;
        Ok(StatusSnapshot {
            changes: parse_porcelain(&output),
        })
    }

    /// 仅变更文件（剔除未跟踪与未修改），供 diff / UI 使用。
    pub async fn changed_files(
        &self,
        cancel: CancellationToken,
    ) -> Result<Vec<FileChange>, GitError> {
        let snapshot = self.status(cancel).await?;
        Ok(snapshot
            .changes
            .into_iter()
            .filter(|c| {
                !(c.untracked
                    || (c.index_status == FileStatus::Unmodified
                        && c.worktree_status == FileStatus::Unmodified))
            })
            .collect())
    }
}

/// 在 `work_dir` 直接读取 status 快照（无需先构造 [`StatusService`]）。
pub async fn read_status(
    work_dir: &Path,
    cancel: CancellationToken,
) -> Result<StatusSnapshot, GitError> {
    let runner = GitRunner::new();
    StatusService::new(&runner, work_dir).status(cancel).await
}

/// 把 porcelain 单字符状态码映射为 [`FileStatus`]。
fn status_from_code(c: u8) -> FileStatus {
    match c {
        b' ' => FileStatus::Unmodified,
        b'M' => FileStatus::Modified,
        b'A' => FileStatus::Added,
        b'D' => FileStatus::Deleted,
        b'R' => FileStatus::Renamed,
        b'C' => FileStatus::Copied,
        b'U' => FileStatus::Unmerged,
        b'T' => FileStatus::TypeChanged,
        b'?' => FileStatus::Untracked,
        // 未识别字符（含 '--ignored' 才会出现的 '!'）保守映射为未修改。
        _ => FileStatus::Unmodified,
    }
}

/// 解析 `git status --porcelain=v1 -z` 的 NUL 分隔输出。
///
/// 每条形如 `XY PATH\0`；当 X 为 R/C 时，PATH 后紧跟原始路径：`XY PATH\0ORIG\0`。
fn parse_porcelain(output: &str) -> Vec<FileChange> {
    let parts: Vec<&str> = output.split('\0').collect();
    let mut changes = Vec::new();
    let mut i = 0;

    while i < parts.len() {
        let entry = parts[i];
        let bytes = entry.as_bytes();
        // 每条至少 "XY "（两个状态码 + 一个分隔空格），否则为末尾空字段 / 异常输入。
        if bytes.len() < 3 || bytes[2] != b' ' {
            break;
        }
        let x = bytes[0];
        let y = bytes[1];
        let path = entry[3..].to_string();
        // rename / copy 由 index（X 列）触发，附带原始路径字段。
        let needs_orig = x == b'R' || x == b'C';
        let previous_path = if needs_orig {
            i += 1;
            if i < parts.len() {
                Some(parts[i].to_string())
            } else {
                None
            }
        } else {
            None
        };
        changes.push(FileChange {
            path,
            previous_path,
            index_status: status_from_code(x),
            worktree_status: status_from_code(y),
            untracked: x == b'?' && y == b'?',
        });
        i += 1;
    }

    changes
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    use tempfile::TempDir;

    /// 为测试 git 注入确定性作者 / 提交者环境，并禁止任何交互提示。
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

    /// 在 `dir` 下执行 git 子命令，断言成功。
    fn git(dir: &Path, args: &[&str]) {
        let mut cmd = Command::new("git");
        cmd.args(args).current_dir(dir);
        git_env(&mut cmd);
        let status = cmd.status().expect("run git");
        assert!(status.success(), "git {args:?} 应成功");
    }

    /// 在 `dir` 下创建一次 commit。
    fn git_commit(dir: &Path, msg: &str) {
        git(dir, &["commit", "-q", "-m", msg]);
    }

    /// 创建临时仓库并提交 a.txt（内容 "v1\n"）。
    fn make_initial_repo() -> TempDir {
        let dir = TempDir::new().expect("tempdir");
        git(dir.path(), &["init", "-q"]);
        std::fs::write(dir.path().join("a.txt"), "v1\n").expect("write a.txt");
        git(dir.path(), &["add", "a.txt"]);
        git_commit(dir.path(), "init");
        dir
    }

    #[tokio::test]
    async fn status_clean_repo_has_no_changes() {
        let dir = make_initial_repo();
        let runner = GitRunner::new();
        let svc = StatusService::new(&runner, dir.path());
        let snap = svc.status(CancellationToken::new()).await.expect("status");
        assert!(snap.changes.is_empty(), "干净仓库应无变更");
    }

    #[tokio::test]
    async fn status_parses_modified_and_untracked() {
        let dir = make_initial_repo();

        // 改 a.txt → 暂存（index=Modified）→ 再改（worktree=Modified）。
        std::fs::write(dir.path().join("a.txt"), "v2\n").expect("write a.txt v2");
        git(dir.path(), &["add", "a.txt"]);
        std::fs::write(dir.path().join("a.txt"), "v3\n").expect("write a.txt v3");

        // 新建 b.txt（untracked）。
        std::fs::write(dir.path().join("b.txt"), "new\n").expect("write b.txt");

        let runner = GitRunner::new();
        let svc = StatusService::new(&runner, dir.path());
        let snap = svc.status(CancellationToken::new()).await.expect("status");

        let a = snap
            .changes
            .iter()
            .find(|c| c.path == "a.txt")
            .expect("应解析出 a.txt");
        assert_eq!(a.index_status, FileStatus::Modified, "a.txt index=Modified");
        assert_eq!(
            a.worktree_status,
            FileStatus::Modified,
            "a.txt worktree=Modified"
        );
        assert!(!a.untracked, "a.txt 非未跟踪");

        let b = snap
            .changes
            .iter()
            .find(|c| c.path == "b.txt")
            .expect("应解析出 b.txt");
        assert!(b.untracked, "b.txt 为未跟踪");
        assert_eq!(b.index_status, FileStatus::Untracked);
        assert_eq!(b.worktree_status, FileStatus::Untracked);

        // changed_files 含 a.txt、不含未跟踪 b.txt。
        let changed = svc
            .changed_files(CancellationToken::new())
            .await
            .expect("changed_files");
        assert!(
            changed.iter().any(|c| c.path == "a.txt"),
            "changed_files 应含 a.txt"
        );
        assert!(
            !changed.iter().any(|c| c.path == "b.txt"),
            "changed_files 不应含未跟踪 b.txt"
        );
    }

    #[tokio::test]
    async fn status_parses_rename_previous_path() {
        let dir = TempDir::new().expect("tempdir");
        git(dir.path(), &["init", "-q"]);
        std::fs::write(dir.path().join("x.txt"), "x\n").expect("write x.txt");
        git(dir.path(), &["add", "x.txt"]);
        git_commit(dir.path(), "init");
        git(dir.path(), &["mv", "x.txt", "y.txt"]);

        let runner = GitRunner::new();
        let svc = StatusService::new(&runner, dir.path());
        let snap = svc.status(CancellationToken::new()).await.expect("status");

        let r = snap
            .changes
            .iter()
            .find(|c| c.path == "y.txt")
            .expect("应解析出重命名后的 y.txt");
        assert_eq!(r.index_status, FileStatus::Renamed, "index=Renamed");
        assert_eq!(
            r.previous_path.as_deref(),
            Some("x.txt"),
            "previous_path=x.txt"
        );
        assert!(!r.untracked);
    }

    #[tokio::test]
    async fn read_status_convenience_works() {
        let dir = make_initial_repo();
        std::fs::write(dir.path().join("c.txt"), "c\n").expect("write c.txt");

        let snap = read_status(dir.path(), CancellationToken::new())
            .await
            .expect("read_status");
        assert!(
            snap.changes
                .iter()
                .any(|c| c.path == "c.txt" && c.untracked),
            "read_status 应解析出未跟踪 c.txt"
        );
    }

    #[test]
    fn parse_porcelain_handles_empty() {
        assert!(parse_porcelain("").is_empty());
        assert!(parse_porcelain("\0").is_empty());
    }

    #[test]
    fn parse_porcelain_single_modified() {
        // "MM a.txt" → index=Modified, worktree=Modified。
        let changes = parse_porcelain("MM a.txt\0");
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].path, "a.txt");
        assert_eq!(changes[0].index_status, FileStatus::Modified);
        assert_eq!(changes[0].worktree_status, FileStatus::Modified);
        assert!(!changes[0].untracked);
        assert!(changes[0].previous_path.is_none());
    }

    #[test]
    fn parse_porcelain_untracked_and_rename() {
        // "??" 未跟踪 + "R  y.txt" 重命名附带原始路径。
        let changes = parse_porcelain("?? b.txt\0R  y.txt\0x.txt\0");
        assert_eq!(changes.len(), 2);

        assert_eq!(changes[0].path, "b.txt");
        assert!(changes[0].untracked);
        assert_eq!(changes[0].index_status, FileStatus::Untracked);
        assert_eq!(changes[0].worktree_status, FileStatus::Untracked);

        assert_eq!(changes[1].path, "y.txt");
        assert_eq!(changes[1].index_status, FileStatus::Renamed);
        assert_eq!(changes[1].worktree_status, FileStatus::Unmodified);
        assert_eq!(changes[1].previous_path.as_deref(), Some("x.txt"));
        assert!(!changes[1].untracked);
    }
}
