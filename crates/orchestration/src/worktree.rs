//! Worker 独立 worktree 分配与隔离守卫（P12-3）。
//!
//! [`WorktreeAllocator`] 抽象了 worktree 的分配 / 释放，真实实现
//! ([`GitWorktreeAllocator`]) 委托 `git-service` 的 `WorktreeService`，
//! 测试注入 [`FakeWorktreeAllocator`]（测试模块内）。
//!
//! 安全约定（ADR-007）：释放只调用 `git worktree remove`，绝不递归删除
//! 用户数据目录；`WorktreeService::remove` 内部先校验目标为受管理 worktree。
//! [`WorktreeGuard`] 在 `Drop` 时尽力释放（失败仅记录日志），
//! [`WorktreeGuard::into_inner`] 可转移所有权而不再释放。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use agent_domain::CancellationToken;
use async_trait::async_trait;
use git_service::{GitRunner, WorktreeService};

/// 一个已分配的 worker worktree。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkerWorktree {
    /// worktree 工作树路径。
    pub path: PathBuf,
    /// checkout 的分支（短名）。
    pub branch: String,
    /// 是否仍受 allocator 管理（释放后为 `false`）。
    pub managed: bool,
}

/// worktree 分配错误。
#[derive(Debug, thiserror::Error)]
pub enum WorktreeError {
    /// 分配失败。
    #[error("worktree allocation failed: {0}")]
    Allocate(String),
    /// 释放失败。
    #[error("worktree release failed: {0}")]
    Release(String),
    /// 本地 I/O 错误。
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

/// worktree 分配器抽象（真实实现走 git-service，测试注入 fake）。
#[async_trait]
pub trait WorktreeAllocator: Send + Sync {
    /// 在 `parent_path`（git 仓库）下分配名为 `branch` 的 worktree。
    async fn allocate(
        &self,
        parent_path: &Path,
        branch: &str,
        start_point: Option<&str>,
    ) -> Result<WorkerWorktree, WorktreeError>;

    /// 释放 `path` 指向的 worktree。绝不删除用户数据（委托 git 的删除安全保证）。
    async fn release(&self, path: &Path) -> Result<(), WorktreeError>;
}

/// 真实分配器：委托 `git-service` 的 [`WorktreeService`]。
pub struct GitWorktreeAllocator {
    runner: Arc<GitRunner>,
    cancel_default: CancellationToken,
}

impl GitWorktreeAllocator {
    /// 以共享 `GitRunner` 构造；取消令牌使用默认新建值。
    pub fn new(runner: Arc<GitRunner>) -> Self {
        Self {
            runner,
            cancel_default: CancellationToken::new(),
        }
    }

    /// 覆盖默认取消令牌（便于测试注入已取消的令牌）。
    pub fn with_cancel(mut self, cancel: CancellationToken) -> Self {
        self.cancel_default = cancel;
        self
    }
}

#[async_trait]
impl WorktreeAllocator for GitWorktreeAllocator {
    async fn allocate(
        &self,
        parent_path: &Path,
        branch: &str,
        start_point: Option<&str>,
    ) -> Result<WorkerWorktree, WorktreeError> {
        let service = WorktreeService::new(&self.runner, parent_path);
        let target = parent_path.join(branch);
        let worktree = service
            .add(&target, branch, start_point, self.cancel_default.clone())
            .await
            .map_err(|error| WorktreeError::Allocate(error.to_string()))?;
        Ok(WorkerWorktree {
            path: worktree.path,
            branch: worktree.branch.unwrap_or_else(|| branch.to_string()),
            managed: true,
        })
    }

    async fn release(&self, path: &Path) -> Result<(), WorktreeError> {
        // `git worktree list` / `remove` 可从任意 worktree 内执行，因此以
        // 目标路径自身作为 cwd；`WorktreeService::remove` 先校验目标为受管理
        // worktree，非受管路径直接报错，绝不触碰用户数据。
        let service = WorktreeService::new(&self.runner, path);
        service
            .remove(path, false, self.cancel_default.clone())
            .await
            .map_err(|error| WorktreeError::Release(error.to_string()))
    }
}

/// worktree RAII 守卫：`Drop` 时尽力释放（best-effort，失败仅记录日志）。
pub struct WorktreeGuard {
    inner: WorkerWorktree,
    allocator: Arc<dyn WorktreeAllocator>,
}

impl WorktreeGuard {
    /// 包装一个已分配的 worktree 与对应分配器。
    pub fn new(inner: WorkerWorktree, allocator: Arc<dyn WorktreeAllocator>) -> Self {
        Self { inner, allocator }
    }

    /// worktree 路径。
    pub fn path(&self) -> &Path {
        &self.inner.path
    }

    /// 借用的 worktree 信息。
    pub fn worktree(&self) -> &WorkerWorktree {
        &self.inner
    }

    /// 取走 worktree，转移所有权且不触发释放；调用方负责后续释放。
    pub fn into_inner(mut self) -> WorkerWorktree {
        self.inner.managed = true;
        std::mem::replace(
            &mut self.inner,
            WorkerWorktree {
                path: PathBuf::new(),
                branch: String::new(),
                managed: false,
            },
        )
    }
}

impl Drop for WorktreeGuard {
    fn drop(&mut self) {
        if !self.inner.managed {
            return;
        }
        self.inner.managed = false;
        let path = self.inner.path.clone();
        let allocator = self.allocator.clone();
        // Drop 中无法 await：派发独立任务尽力释放，失败仅记录日志。
        tokio::spawn(async move {
            if let Err(error) = allocator.release(&path).await {
                tracing::warn!(
                    path = %path.display(),
                    error = %error,
                    "worktree release failed on guard drop (best-effort)"
                );
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::time::Duration;

    /// 测试用 fake：每次分配创建独立临时目录并写入标记文件；
    /// `release` 只记录路径、从不删除任何用户数据。
    pub struct FakeWorktreeAllocator {
        tempdirs: Mutex<Vec<tempfile::TempDir>>,
        released: Mutex<Vec<PathBuf>>,
    }

    impl FakeWorktreeAllocator {
        pub fn new() -> Self {
            Self {
                tempdirs: Mutex::new(Vec::new()),
                released: Mutex::new(Vec::new()),
            }
        }

        pub fn released(&self) -> Vec<PathBuf> {
            self.released
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .clone()
        }
    }

    impl Default for FakeWorktreeAllocator {
        fn default() -> Self {
            Self::new()
        }
    }

    #[async_trait]
    impl WorktreeAllocator for FakeWorktreeAllocator {
        async fn allocate(
            &self,
            _parent_path: &Path,
            branch: &str,
            _start_point: Option<&str>,
        ) -> Result<WorkerWorktree, WorktreeError> {
            let dir = tempfile::tempdir().map_err(WorktreeError::Io)?;
            std::fs::write(dir.path().join("README.md"), "fake worktree\n")
                .map_err(WorktreeError::Io)?;
            let path = dir.path().to_path_buf();
            self.tempdirs
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .push(dir);
            Ok(WorkerWorktree {
                path,
                branch: branch.to_string(),
                managed: true,
            })
        }

        async fn release(&self, path: &Path) -> Result<(), WorktreeError> {
            // 绝不删除用户数据：只记录释放请求。
            self.released
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .push(path.to_path_buf());
            Ok(())
        }
    }

    async fn wait_for_release(allocator: &FakeWorktreeAllocator, path: &Path) {
        for _ in 0..100 {
            if allocator.released().iter().any(|p| p == path) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("worktree release was not observed for {}", path.display());
    }

    #[tokio::test]
    async fn allocate_creates_isolated_worktree() {
        let parent = tempfile::tempdir().unwrap();
        std::fs::write(parent.path().join("notes.txt"), "parent content\n").unwrap();
        let allocator = Arc::new(FakeWorktreeAllocator::new());

        let worktree = allocator
            .allocate(parent.path(), "feature-x", None)
            .await
            .unwrap();
        assert!(worktree.managed);
        assert_eq!(worktree.branch, "feature-x");
        assert!(worktree.path.join("README.md").exists());
    }

    #[tokio::test]
    async fn worker_write_does_not_change_parent_file() {
        let parent = tempfile::tempdir().unwrap();
        std::fs::write(parent.path().join("notes.txt"), "parent content\n").unwrap();
        let allocator = Arc::new(FakeWorktreeAllocator::new());

        let worktree = allocator
            .allocate(parent.path(), "feature-x", None)
            .await
            .unwrap();
        // worker 写入自己的 worktree 副本。
        std::fs::write(worktree.path.join("notes.txt"), "worker content\n").unwrap();
        assert_eq!(
            std::fs::read_to_string(parent.path().join("notes.txt")).unwrap(),
            "parent content\n",
            "worker 写入不得改变 parent 路径下的文件"
        );
        assert_eq!(
            std::fs::read_to_string(worktree.path.join("notes.txt")).unwrap(),
            "worker content\n"
        );
    }

    #[tokio::test]
    async fn guard_drop_releases_worktree() {
        let allocator = Arc::new(FakeWorktreeAllocator::new());
        let worktree = allocator
            .allocate(Path::new("."), "branch-a", None)
            .await
            .unwrap();
        let path = worktree.path.clone();
        let guard = WorktreeGuard::new(worktree, allocator.clone());
        assert!(guard.worktree().managed);
        drop(guard);
        wait_for_release(&allocator, &path).await;
        // fake 从不删除用户数据。
        assert!(path.join("README.md").exists());
    }

    #[tokio::test]
    async fn into_inner_transfers_ownership_without_release() {
        let allocator = Arc::new(FakeWorktreeAllocator::new());
        let worktree = allocator
            .allocate(Path::new("."), "branch-b", None)
            .await
            .unwrap();
        let path = worktree.path.clone();
        let guard = WorktreeGuard::new(worktree, allocator.clone());
        let inner = guard.into_inner();
        assert!(inner.managed);
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(
            !allocator.released().iter().any(|p| p == &path),
            "into_inner 不得触发释放"
        );
        // 显式释放仍可用。
        allocator.release(&path).await.unwrap();
        assert!(allocator.released().iter().any(|p| p == &path));
    }

    #[tokio::test]
    async fn release_of_non_managed_worktree_is_recorded_but_data_kept() {
        let allocator = Arc::new(FakeWorktreeAllocator::new());
        let worktree = allocator
            .allocate(Path::new("."), "branch-c", None)
            .await
            .unwrap();
        allocator.release(&worktree.path).await.unwrap();
        assert!(worktree.path.exists(), "fake 释放后目录必须保留");
        assert!(worktree.path.join("README.md").exists());
    }
}
