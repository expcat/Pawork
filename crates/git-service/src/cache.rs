//! Git status 缓存与文件监听失效（[`StatusCache`] / [`CachedStatusService`]）。
//!
//! - [`StatusCache`]：进程内 `parking_lot::RwLock<HashMap>`，命中路径为纯内存读，
//!   满足「已缓存 status 切换 < 50ms」。
//! - [`CachedStatusService`]：先查缓存，未命中再跑 `git status` 写回。
//! - [`spawn_invalidator`]：用 notify 监听 worktree（含 `.git`）变更，去抖后
//!   `invalidate` 对应 work_dir 的缓存。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use agent_domain::CancellationToken;

use crate::error::GitError;
use crate::process::GitRunner;
use crate::status::{StatusService, StatusSnapshot};

/// 缓存的视角范围。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CacheScope {
    /// 工作区视角（staged + unstaged + untracked）。
    Worktree,
    /// 暂存区视角（仅 index）。
    Staged,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct CacheKey {
    work_dir: PathBuf,
    scope: CacheScope,
}

#[derive(Clone, Debug)]
struct CacheEntry {
    snapshot: StatusSnapshot,
    #[allow(dead_code)]
    computed_at: Instant,
}

/// Git status 缓存：进程内读写锁保护的 HashMap。
#[derive(Default)]
pub struct StatusCache {
    inner: parking_lot::RwLock<HashMap<CacheKey, CacheEntry>>,
}

impl StatusCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// 读缓存（命中返回快照拷贝）。未命中返回 None。
    pub fn get(&self, work_dir: &Path, scope: CacheScope) -> Option<StatusSnapshot> {
        let key = CacheKey {
            work_dir: work_dir.to_path_buf(),
            scope,
        };
        self.inner.read().get(&key).map(|e| e.snapshot.clone())
    }

    /// 写入缓存。
    pub fn put(&self, work_dir: &Path, scope: CacheScope, snapshot: StatusSnapshot) {
        let key = CacheKey {
            work_dir: work_dir.to_path_buf(),
            scope,
        };
        self.inner.write().insert(
            key,
            CacheEntry {
                snapshot,
                computed_at: Instant::now(),
            },
        );
    }

    /// 失效指定 work_dir 的全部 scope。
    pub fn invalidate(&self, work_dir: &Path) {
        let mut map = self.inner.write();
        let prefix = work_dir.to_path_buf();
        map.retain(|k, _| k.work_dir != prefix);
    }

    /// 清空全部缓存。
    pub fn clear(&self) {
        self.inner.write().clear();
    }
}

/// 组合「缓存 + 按需刷新」的 status 服务。
pub struct CachedStatusService {
    runner: GitRunner,
    work_dir: PathBuf,
    cache: Arc<StatusCache>,
}

impl CachedStatusService {
    pub fn new(runner: GitRunner, work_dir: &Path, cache: Arc<StatusCache>) -> Self {
        Self {
            runner,
            work_dir: work_dir.to_path_buf(),
            cache,
        }
    }

    /// 优先返回缓存；未命中则跑 `git status` 写入缓存后返回。命中为纯内存读。
    pub async fn status(
        &self,
        scope: CacheScope,
        cancel: CancellationToken,
    ) -> Result<StatusSnapshot, GitError> {
        if let Some(hit) = self.cache.get(&self.work_dir, scope) {
            return Ok(hit);
        }
        self.refresh(scope, cancel).await
    }

    /// 强制刷新并返回最新（绕过缓存）。
    pub async fn refresh(
        &self,
        scope: CacheScope,
        cancel: CancellationToken,
    ) -> Result<StatusSnapshot, GitError> {
        let svc = StatusService::new(&self.runner, &self.work_dir);
        // 当前 status 解析器统一返回 staged+unstaged+untracked 视角；
        // scope 仅影响缓存槽位区分，语义与 worktree 视图一致。
        let _ = scope;
        let snapshot = svc.status(cancel).await?;
        self.cache.put(&self.work_dir, scope, snapshot.clone());
        Ok(snapshot)
    }
}

/// watcher 句柄：drop 即停止监听。
pub struct WatcherGuard {
    // 持有 debouncer 以保证其在 guard 存活期间持续监听。
    _debouncer: notify_debouncer_full::Debouncer<
        notify_debouncer_full::notify::RecommendedWatcher,
        notify_debouncer_full::RecommendedCache,
    >,
}

/// 启动后台 watcher：监听 `work_dir`（含 `.git`）文件变更，去抖（300ms）后对
/// 该 work_dir 调 `cache.invalidate`。返回 guard，drop 即停止监听。
///
/// 任何 watch 错误仅记录日志、不影响主流程。
pub fn spawn_invalidator(
    work_dir: &Path,
    cache: Arc<StatusCache>,
) -> std::io::Result<WatcherGuard> {
    use notify_debouncer_full::{new_debouncer, notify::RecursiveMode};

    let wd = work_dir.to_path_buf();
    let debouncer = new_debouncer(
        Duration::from_millis(300),
        None,
        move |_res: notify_debouncer_full::DebounceEventResult| {
            // 去抖后的任何变更事件都令对应 work_dir 缓存失效。
            cache.invalidate(&wd);
        },
    )
    .map_err(std::io::Error::other)?;

    let mut debouncer = debouncer;
    if let Err(e) = debouncer.watch(work_dir, RecursiveMode::Recursive) {
        tracing::warn!(error = ?e, dir = %work_dir.display(), "watch work_dir failed");
    }
    let git_dir = work_dir.join(".git");
    if git_dir.exists() {
        if let Err(e) = debouncer.watch(&git_dir, RecursiveMode::Recursive) {
            tracing::warn!(error = ?e, dir = %git_dir.display(), "watch .git failed");
        }
    }

    Ok(WatcherGuard {
        _debouncer: debouncer,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process::GitRunner;
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
    async fn cache_hit_miss_and_invalidate() {
        let (_dir, repo) = make_repo();
        let cache = Arc::new(StatusCache::new());
        let svc = CachedStatusService::new(GitRunner::new(), &repo, cache.clone());

        // 未命中：refresh 填充。
        assert!(cache.get(&repo, CacheScope::Worktree).is_none());
        let snap = svc
            .refresh(CacheScope::Worktree, CancellationToken::new())
            .await
            .expect("refresh");
        assert!(cache.get(&repo, CacheScope::Worktree).is_some());
        // status 命中应返回同一快照。
        let hit = svc
            .status(CacheScope::Worktree, CancellationToken::new())
            .await
            .expect("status");
        assert_eq!(hit, snap);

        // invalidate 后失效。
        cache.invalidate(&repo);
        assert!(cache.get(&repo, CacheScope::Worktree).is_none());
    }

    #[tokio::test]
    async fn cached_status_hit_is_fast() {
        let (_dir, repo) = make_repo();
        let cache = Arc::new(StatusCache::new());
        let svc = CachedStatusService::new(GitRunner::new(), &repo, cache.clone());
        // 预热缓存。
        svc.refresh(CacheScope::Worktree, CancellationToken::new())
            .await
            .expect("refresh");

        // 连续命中 1000 次，单次应远低于 50ms（断言总耗时 < 50ms，宽松防抖动）。
        let start = Instant::now();
        for _ in 0..1000 {
            let _ = svc
                .status(CacheScope::Worktree, CancellationToken::new())
                .await
                .expect("status");
        }
        let elapsed = start.elapsed();
        assert!(
            elapsed.as_millis() < 50,
            "1000 cached hits took {:?}, expected < 50ms",
            elapsed
        );
    }
}
