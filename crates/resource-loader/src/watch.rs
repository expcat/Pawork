//! 资源目录的去抖监听与原子快照更新。

use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex, RwLock},
    time::Duration,
};

use diagnostics::Redactor;
use notify_debouncer_full::{
    new_debouncer,
    notify::{RecommendedWatcher, RecursiveMode},
    Debouncer, RecommendedCache,
};

use crate::ResourceLoadError;

type ReloadFn<T> = dyn Fn() -> Result<T, String> + Send + Sync + 'static;

#[derive(Clone, Debug)]
pub struct HotReloadSnapshot<T> {
    pub generation: u64,
    pub value: Arc<T>,
    pub last_error: Option<String>,
}

struct HotReloadState<T> {
    generation: u64,
    value: Arc<T>,
    last_error: Option<String>,
}

struct HotReloadInner<T> {
    state: RwLock<HotReloadState<T>>,
    loader: Arc<ReloadFn<T>>,
    redactor: Redactor,
    reload_lock: Mutex<()>,
}

/// 可克隆的资源快照仓库。重建在锁外执行，成功后一次性替换快照。
pub struct ResourceHotReload<T> {
    inner: Arc<HotReloadInner<T>>,
}

impl<T> Clone for ResourceHotReload<T> {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

/// 持有 notify debouncer；drop 即停止监听。
pub struct ResourceWatcher {
    _debouncer: Option<Debouncer<RecommendedWatcher, RecommendedCache>>,
    watched_paths: Vec<PathBuf>,
}

impl ResourceWatcher {
    pub fn watched_paths(&self) -> &[PathBuf] {
        &self.watched_paths
    }
}

impl<T> ResourceHotReload<T>
where
    T: Send + Sync + 'static,
{
    /// 初次同步加载资源并启动递归 watcher。空路径列表仍返回可手动 reload 的 store。
    pub fn start<I, P, F>(
        paths: I,
        debounce: Duration,
        loader: F,
    ) -> Result<(Self, ResourceWatcher), ResourceLoadError>
    where
        I: IntoIterator<Item = P>,
        P: AsRef<Path>,
        F: Fn() -> Result<T, String> + Send + Sync + 'static,
    {
        let loader: Arc<ReloadFn<T>> = Arc::new(loader);
        let initial = loader()
            .map_err(|error| ResourceLoadError::InitialLoad(Redactor::default().redact(&error)))?;
        let store = Self {
            inner: Arc::new(HotReloadInner {
                state: RwLock::new(HotReloadState {
                    generation: 1,
                    value: Arc::new(initial),
                    last_error: None,
                }),
                loader,
                redactor: Redactor::default(),
                reload_lock: Mutex::new(()),
            }),
        };

        let mut watched_paths = paths
            .into_iter()
            .map(|path| path.as_ref().to_path_buf())
            .collect::<Vec<_>>();
        watched_paths.sort_by(|left, right| left.to_string_lossy().cmp(&right.to_string_lossy()));
        watched_paths.dedup();
        if watched_paths.is_empty() {
            return Ok((
                store,
                ResourceWatcher {
                    _debouncer: None,
                    watched_paths,
                },
            ));
        }

        let callback_store = store.clone();
        let mut debouncer = new_debouncer(
            debounce,
            None,
            move |result: notify_debouncer_full::DebounceEventResult| match result {
                Ok(_events) => {
                    callback_store.reload_now();
                }
                Err(errors) => {
                    let message = errors
                        .into_iter()
                        .map(|error| error.to_string())
                        .collect::<Vec<_>>()
                        .join("; ");
                    callback_store.record_error(&format!("resource watch failed: {message}"));
                }
            },
        )
        .map_err(|error| ResourceLoadError::Watcher(error.to_string()))?;
        for path in &watched_paths {
            debouncer
                .watch(path, RecursiveMode::Recursive)
                .map_err(|error| {
                    ResourceLoadError::Watcher(format!(
                        "could not watch resource path '{}': {error}",
                        path.display()
                    ))
                })?;
        }
        // 覆盖「初次加载完成 → watcher 注册完成」之间的变更窗口。并发回调由
        // reload_lock 串行化，旧重建结果不会在较新的重建之后覆盖快照。
        store.reload_now();
        Ok((
            store,
            ResourceWatcher {
                _debouncer: Some(debouncer),
                watched_paths,
            },
        ))
    }

    pub fn snapshot(&self) -> HotReloadSnapshot<T> {
        let state = self
            .inner
            .state
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        HotReloadSnapshot {
            generation: state.generation,
            value: Arc::clone(&state.value),
            last_error: state.last_error.clone(),
        }
    }

    /// 立即重建。loader 在状态锁外运行；失败保留最后一个成功快照。
    pub fn reload_now(&self) -> bool {
        let _reload = self
            .inner
            .reload_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match (self.inner.loader)() {
            Ok(value) => {
                let mut state = self
                    .inner
                    .state
                    .write()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                state.value = Arc::new(value);
                state.generation = state.generation.saturating_add(1);
                state.last_error = None;
                true
            }
            Err(error) => {
                self.record_error(&error);
                false
            }
        }
    }

    fn record_error(&self, error: &str) {
        let redacted = self.inner.redactor.redact(error);
        let mut state = self
            .inner
            .state
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.last_error = Some(redacted);
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        sync::{
            atomic::{AtomicBool, AtomicUsize, Ordering},
            Barrier,
        },
        thread,
        time::Instant,
    };

    use super::*;

    fn wait_until(timeout: Duration, predicate: impl Fn() -> bool) -> bool {
        let started = Instant::now();
        while started.elapsed() < timeout {
            if predicate() {
                return true;
            }
            thread::sleep(Duration::from_millis(20));
        }
        false
    }

    #[test]
    fn manual_failure_keeps_previous_snapshot_and_redacts_error() {
        let fail = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&fail);
        let (store, _watcher) = ResourceHotReload::start(
            std::iter::empty::<&Path>(),
            Duration::from_millis(30),
            move || {
                if flag.load(Ordering::SeqCst) {
                    Err("Authorization: Bearer sk-abcdefghijklmnop".into())
                } else {
                    Ok(7_u64)
                }
            },
        )
        .expect("store");
        fail.store(true, Ordering::SeqCst);
        assert!(!store.reload_now());
        let snapshot = store.snapshot();
        assert_eq!(*snapshot.value, 7);
        assert_eq!(snapshot.generation, 1);
        let error = snapshot.last_error.expect("error");
        assert!(!error.contains("sk-abcdefghijklmnop"));
        assert!(error.contains("[REDACTED]"));
    }

    #[test]
    fn filesystem_change_debounces_and_replaces_snapshot() {
        let temp = tempfile::tempdir().expect("tempdir");
        let file = temp.path().join("resource.txt");
        fs::write(&file, "one").expect("write initial");
        let watched_file = file.clone();
        let (store, watcher) =
            ResourceHotReload::start([temp.path()], Duration::from_millis(80), move || {
                fs::read_to_string(&watched_file).map_err(|error| error.to_string())
            })
            .expect("watcher");
        assert_eq!(store.snapshot().value.as_str(), "one");
        fs::write(&file, "two").expect("write changed");
        assert!(wait_until(Duration::from_secs(5), || {
            store.snapshot().value.as_str() == "two"
        }));
        assert!(store.snapshot().generation >= 2);

        drop(watcher);
        let generation = store.snapshot().generation;
        fs::write(&file, "three").expect("write after drop");
        thread::sleep(Duration::from_millis(350));
        assert_eq!(store.snapshot().generation, generation);
        assert_eq!(store.snapshot().value.as_str(), "two");
    }

    #[test]
    fn empty_watch_set_supports_manual_reload() {
        let counter = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let load_counter = Arc::clone(&counter);
        let (store, watcher) = ResourceHotReload::start(
            std::iter::empty::<PathBuf>(),
            Duration::from_millis(10),
            move || Ok(load_counter.fetch_add(1, Ordering::SeqCst)),
        )
        .expect("store");
        assert!(watcher.watched_paths().is_empty());
        assert_eq!(*store.snapshot().value, 0);
        assert!(store.reload_now());
        assert_eq!(*store.snapshot().value, 1);
    }

    #[test]
    fn concurrent_manual_reloads_are_serialized() {
        let active = Arc::new(AtomicUsize::new(0));
        let maximum = Arc::new(AtomicUsize::new(0));
        let calls = Arc::new(AtomicUsize::new(0));
        let load_active = Arc::clone(&active);
        let load_maximum = Arc::clone(&maximum);
        let load_calls = Arc::clone(&calls);
        let (store, _watcher) = ResourceHotReload::start(
            std::iter::empty::<PathBuf>(),
            Duration::from_millis(10),
            move || {
                let now = load_active.fetch_add(1, Ordering::SeqCst) + 1;
                load_maximum.fetch_max(now, Ordering::SeqCst);
                thread::sleep(Duration::from_millis(30));
                load_active.fetch_sub(1, Ordering::SeqCst);
                Ok(load_calls.fetch_add(1, Ordering::SeqCst))
            },
        )
        .expect("store");
        maximum.store(0, Ordering::SeqCst);

        let barrier = Arc::new(Barrier::new(3));
        let left_store = store.clone();
        let left_barrier = Arc::clone(&barrier);
        let left = thread::spawn(move || {
            left_barrier.wait();
            left_store.reload_now()
        });
        let right_store = store.clone();
        let right_barrier = Arc::clone(&barrier);
        let right = thread::spawn(move || {
            right_barrier.wait();
            right_store.reload_now()
        });
        barrier.wait();
        assert!(left.join().expect("left"));
        assert!(right.join().expect("right"));
        assert_eq!(maximum.load(Ordering::SeqCst), 1);
        assert_eq!(store.snapshot().generation, 3);
    }
}
