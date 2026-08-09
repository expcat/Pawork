//! Git status 缓存与文件监听失效（[`StatusCache`] / [`CachedStatusService`]）。
//!
//! - [`StatusCache`]：进程内有 TTL 与容量上限的 `parking_lot::RwLock<HashMap>`，
//!   命中路径为纯内存读，满足「已缓存 status 切换 < 50ms」。
//! - [`CachedStatusService`]：先查缓存，未命中再跑 `git status` 写回。
//! - [`spawn_invalidator`]：按 ignore 规则枚举 worktree 目录作非递归监听，并通过
//!   `git rev-parse` 解析真实 git-dir；去抖后 `invalidate` 对应 work_dir 的缓存。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use agent_domain::CancellationToken;
use ignore::WalkBuilder;

use crate::error::GitError;
use crate::process::GitRunner;
use crate::repo::GitService;
use crate::status::{StatusService, StatusSnapshot};

const DEFAULT_CACHE_TTL: Duration = Duration::from_secs(30);
const DEFAULT_MAX_ENTRIES: usize = 128;

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
    computed_at: Instant,
    last_accessed: Instant,
}

/// Git status 缓存：进程内读写锁保护、带 TTL 与 LRU 容量上限的 HashMap。
pub struct StatusCache {
    inner: parking_lot::RwLock<HashMap<CacheKey, CacheEntry>>,
    ttl: Duration,
    max_entries: usize,
}

impl Default for StatusCache {
    fn default() -> Self {
        Self {
            inner: parking_lot::RwLock::new(HashMap::new()),
            ttl: DEFAULT_CACHE_TTL,
            max_entries: DEFAULT_MAX_ENTRIES,
        }
    }
}

impl StatusCache {
    pub fn new() -> Self {
        Self::default()
    }

    #[cfg(test)]
    fn with_limits(ttl: Duration, max_entries: usize) -> Self {
        Self {
            inner: parking_lot::RwLock::new(HashMap::new()),
            ttl,
            max_entries: max_entries.max(1),
        }
    }

    /// 读缓存（命中返回快照拷贝并刷新 LRU 时间）。过期或未命中返回 None。
    pub fn get(&self, work_dir: &Path, scope: CacheScope) -> Option<StatusSnapshot> {
        let key = CacheKey {
            work_dir: dunce::simplified(work_dir).to_path_buf(),
            scope,
        };
        let now = Instant::now();
        let mut map = self.inner.write();
        let expired = map
            .get(&key)
            .is_some_and(|entry| now.duration_since(entry.computed_at) >= self.ttl);
        if expired {
            map.remove(&key);
            return None;
        }
        map.get_mut(&key).map(|entry| {
            entry.last_accessed = now;
            entry.snapshot.clone()
        })
    }

    /// 写入缓存；先清理过期项，达到上限时淘汰最久未访问的条目。
    pub fn put(&self, work_dir: &Path, scope: CacheScope, snapshot: StatusSnapshot) {
        let key = CacheKey {
            work_dir: dunce::simplified(work_dir).to_path_buf(),
            scope,
        };
        let now = Instant::now();
        let mut map = self.inner.write();
        map.retain(|_, entry| now.duration_since(entry.computed_at) < self.ttl);
        if !map.contains_key(&key) && map.len() >= self.max_entries {
            if let Some(oldest) = map
                .iter()
                .min_by_key(|(_, entry)| entry.last_accessed)
                .map(|(key, _)| key.clone())
            {
                map.remove(&oldest);
            }
        }
        map.insert(
            key,
            CacheEntry {
                snapshot,
                computed_at: now,
                last_accessed: now,
            },
        );
    }

    /// 失效指定 work_dir 的全部 scope。
    pub fn invalidate(&self, work_dir: &Path) {
        let mut map = self.inner.write();
        let prefix = dunce::simplified(work_dir).to_path_buf();
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
        let mut snapshot = svc.status(cancel).await?;
        if scope == CacheScope::Staged {
            // porcelain 的 X 列就是 index 视图：仅保留 X 非空的条目，并清掉
            // worktree 列，避免同一文件的未暂存改动泄漏进 staged-only 结果。
            snapshot.changes.retain(|change| {
                change.index_status != crate::status::FileStatus::Unmodified
                    && change.index_status != crate::status::FileStatus::Untracked
            });
            for change in &mut snapshot.changes {
                change.worktree_status = crate::status::FileStatus::Unmodified;
                change.untracked = false;
            }
        }
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
    #[cfg(test)]
    watched_paths: Vec<PathBuf>,
}

/// 启动后台 watcher：先用 git 解析仓库根与真实 git-dir，再按 ignore 规则枚举
/// worktree 中需要监听的目录。worktree 目录逐个非递归监听，git-dir 单独递归监听，
/// 从而避开 node_modules/构建产物等 ignored 子树并兼容 linked worktree。
///
/// 任何 watch 错误仅记录日志、不影响主流程。
pub async fn spawn_invalidator(
    work_dir: &Path,
    cache: Arc<StatusCache>,
    cancel: CancellationToken,
) -> Result<WatcherGuard, GitError> {
    let repo = GitService::open(work_dir, cancel.clone()).await?;
    let git_dir = repo.git_dir(cancel).await?;
    spawn_invalidator_for_paths(repo.work_dir(), &git_dir, cache).map_err(GitError::Io)
}

fn spawn_invalidator_for_paths(
    work_dir: &Path,
    git_dir: &Path,
    cache: Arc<StatusCache>,
) -> std::io::Result<WatcherGuard> {
    use notify_debouncer_full::{new_debouncer, notify::RecursiveMode};

    let wd = dunce::simplified(work_dir).to_path_buf();
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
    #[cfg(test)]
    let mut watched_paths = Vec::new();
    for directory in collect_watch_directories(work_dir) {
        match debouncer.watch(&directory, RecursiveMode::NonRecursive) {
            Ok(()) => {
                #[cfg(test)]
                watched_paths.push(directory);
            }
            Err(error) => {
                tracing::warn!(error = ?error, dir = %directory.display(), "watch worktree directory failed");
            }
        }
    }

    let git_dir = dunce::simplified(git_dir).to_path_buf();
    if git_dir.is_dir() {
        match debouncer.watch(&git_dir, RecursiveMode::Recursive) {
            Ok(()) => {
                #[cfg(test)]
                watched_paths.push(git_dir.clone());
            }
            Err(error) => {
                tracing::warn!(error = ?error, dir = %git_dir.display(), "watch git-dir failed");
            }
        }
    }

    Ok(WatcherGuard {
        _debouncer: debouncer,
        #[cfg(test)]
        watched_paths,
    })
}

/// 用与 file-index 相同的 `ignore` walker 语义枚举非 ignored 目录；`.git`
/// 元数据不随 worktree 递归枚举，而由解析出的 git-dir 单独监听。
fn collect_watch_directories(work_dir: &Path) -> Vec<PathBuf> {
    let root = dunce::simplified(work_dir).to_path_buf();
    let filter_root = root.clone();
    let mut builder = WalkBuilder::new(&root);
    builder
        .hidden(false)
        .parents(true)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .follow_links(false)
        .filter_entry(move |entry| {
            entry.path() == filter_root || entry.file_name() != std::ffi::OsStr::new(".git")
        });

    let mut directories = Vec::new();
    for result in builder.build() {
        match result {
            Ok(entry) if entry.file_type().is_some_and(|kind| kind.is_dir()) => {
                directories.push(dunce::simplified(entry.path()).to_path_buf());
            }
            Ok(_) => {}
            Err(error) => {
                tracing::warn!(error = ?error, dir = %work_dir.display(), "enumerate watcher directories failed");
            }
        }
    }
    directories
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

    #[tokio::test]
    async fn staged_scope_excludes_unstaged_and_untracked_changes() {
        let (_dir, repo) = make_repo();
        std::fs::write(repo.join("staged.txt"), "index version\n").expect("write staged");
        run_git(&repo, &["add", "staged.txt"]);
        std::fs::write(repo.join("staged.txt"), "worktree version\n")
            .expect("add unstaged edit to staged file");
        std::fs::write(repo.join("README.md"), "unstaged tracked edit\n")
            .expect("write unstaged tracked file");
        std::fs::write(repo.join("untracked.txt"), "untracked\n").expect("write untracked");

        let cache = Arc::new(StatusCache::new());
        let svc = CachedStatusService::new(GitRunner::new(), &repo, cache);
        let staged = svc
            .refresh(CacheScope::Staged, CancellationToken::new())
            .await
            .expect("staged status");

        assert_eq!(staged.changes.len(), 1, "staged view = {staged:?}");
        let change = &staged.changes[0];
        assert_eq!(change.path, "staged.txt");
        assert_eq!(change.index_status, crate::status::FileStatus::Added);
        assert_eq!(
            change.worktree_status,
            crate::status::FileStatus::Unmodified,
            "staged-only view must not expose the worktree column"
        );
        assert!(!change.untracked);
    }

    #[test]
    fn cache_expires_entries_and_enforces_lru_capacity() {
        let expired = StatusCache::with_limits(Duration::ZERO, 2);
        expired.put(
            Path::new("expired"),
            CacheScope::Worktree,
            StatusSnapshot::default(),
        );
        assert!(expired
            .get(Path::new("expired"), CacheScope::Worktree)
            .is_none());
        assert!(expired.inner.read().is_empty());

        let bounded = StatusCache::with_limits(Duration::from_secs(60), 2);
        bounded.put(
            Path::new("a"),
            CacheScope::Worktree,
            StatusSnapshot::default(),
        );
        std::thread::sleep(Duration::from_millis(2));
        bounded.put(
            Path::new("b"),
            CacheScope::Worktree,
            StatusSnapshot::default(),
        );
        std::thread::sleep(Duration::from_millis(2));
        assert!(bounded.get(Path::new("a"), CacheScope::Worktree).is_some());
        std::thread::sleep(Duration::from_millis(2));
        bounded.put(
            Path::new("c"),
            CacheScope::Worktree,
            StatusSnapshot::default(),
        );

        assert_eq!(bounded.inner.read().len(), 2);
        assert!(bounded.get(Path::new("a"), CacheScope::Worktree).is_some());
        assert!(bounded.get(Path::new("b"), CacheScope::Worktree).is_none());
        assert!(bounded.get(Path::new("c"), CacheScope::Worktree).is_some());
    }

    #[tokio::test]
    async fn watcher_skips_ignored_tree_and_watches_linked_git_dir() {
        let (_dir, repo) = make_repo();
        std::fs::write(repo.join(".gitignore"), "ignored/\n").expect("write gitignore");
        run_git(&repo, &["add", ".gitignore"]);
        run_git(&repo, &["commit", "-q", "-m", "ignore rules"]);

        let linked = repo.join("linked-watch");
        let linked_arg = linked.to_string_lossy().into_owned();
        run_git(
            &repo,
            &["worktree", "add", "-q", "-b", "watch-branch", &linked_arg],
        );
        std::fs::create_dir_all(linked.join("ignored").join("nested"))
            .expect("create ignored tree");
        std::fs::create_dir_all(linked.join("kept").join("nested")).expect("create watched tree");

        let repo_service = GitService::open(&linked, CancellationToken::new())
            .await
            .expect("open linked worktree");
        let git_dir = repo_service
            .git_dir(CancellationToken::new())
            .await
            .expect("resolve linked git-dir");
        assert!(
            git_dir.to_string_lossy().contains("worktrees"),
            "expected linked worktree administrative dir: {}",
            git_dir.display()
        );

        let guard = spawn_invalidator(
            &linked,
            Arc::new(StatusCache::new()),
            CancellationToken::new(),
        )
        .await
        .expect("spawn watcher");
        let watched: Vec<PathBuf> = guard
            .watched_paths
            .iter()
            .map(|path| dunce::simplified(path).to_path_buf())
            .collect();
        assert!(watched.contains(&dunce::simplified(&git_dir).to_path_buf()));
        assert!(watched.contains(&dunce::simplified(&linked.join("kept")).to_path_buf()));
        assert!(!watched
            .iter()
            .any(|path| path.starts_with(linked.join("ignored"))));
    }
}
