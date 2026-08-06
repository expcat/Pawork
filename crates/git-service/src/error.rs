//! git-service 错误类型。
//!
//! 把 process-runtime 的 `ProcessError`、git 非零退出与超时/取消统一归一为
//! [`GitError`]，供上层（status/stage/worktree/cache）一致处理。

/// git 操作统一错误。
///
/// 将 ProcessError、非零退出码、超时与取消归一为单一枚举，便于上层 match 处理。
#[derive(Debug, thiserror::Error)]
pub enum GitError {
    #[error("git binary not found: {0}")]
    GitNotFound(String),
    #[error("not a git repository: {0}")]
    NotARepository(String),
    #[error("detached HEAD")]
    DetachedHead,
    #[error("git failed (exit code {code:?}): {stderr}")]
    GitFailed { code: Option<i32>, stderr: String },
    #[error("nothing to commit")]
    NothingToCommit,
    #[error("branch already exists: {0}")]
    BranchAlreadyExists(String),
    #[error("branch not found: {0}")]
    BranchNotFound(String),
    #[error("branch not fully merged: {0}")]
    BranchNotMerged(String),
    #[error("reference not found: {0}")]
    ReferenceNotFound(String),
    #[error("local changes would be overwritten: {0:?}")]
    LocalChangesWouldBeOverwritten(Vec<String>),
    #[error("patch does not apply (index changed since diff?)")]
    PatchDoesNotApply,
    #[error("merge conflict: {0}")]
    Conflict(String),
    #[error("git operation timed out")]
    Timeout,
    #[error("git operation cancelled")]
    Cancelled,
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Other(String),
}

impl From<process_runtime::ProcessError> for GitError {
    fn from(err: process_runtime::ProcessError) -> Self {
        match err {
            // spawn 失败通常意味着 git 二进制缺失：NotFound 记录程序名。
            process_runtime::ProcessError::Spawn { program, source } => {
                if source.kind() == std::io::ErrorKind::NotFound {
                    GitError::GitNotFound(program)
                } else {
                    GitError::Other(format!("failed to spawn `{program}`: {source}"))
                }
            }
            process_runtime::ProcessError::Io(io) => GitError::Io(io),
        }
    }
}
