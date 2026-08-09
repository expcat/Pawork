//! 工作区文件索引。
//!
//! 全量扫描在 `spawn_blocking` 中完成，结果构建成功后原子替换；增量事件经有界通道
//! 去抖后批量应用。索引只保存 workspace root 序号与相对路径，不接受模型提供绝对路径。

use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex, RwLock,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use agent_domain::WorkspaceId;
use ignore::{gitignore::GitignoreBuilder, WalkBuilder};
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use thiserror::Error;
use tokio::{
    sync::{mpsc, oneshot},
    task::JoinHandle,
};
use workspace_service::Workspace;

/// watcher / 去抖错误环形缓冲上限。
const MAX_WATCHER_ERRORS: usize = 1024;
const ERRORS_TRUNCATED_MARKER: &str = "[truncated: oldest watcher errors discarded]";

#[derive(Clone, Debug)]
pub struct IndexOptions {
    pub global_ignore_files: Vec<PathBuf>,
    pub workspace_ignore_files: Vec<PathBuf>,
    pub excluded_directories: BTreeSet<String>,
    pub binary_probe_bytes: u64,
}

impl Default for IndexOptions {
    fn default() -> Self {
        Self {
            global_ignore_files: Vec::new(),
            workspace_ignore_files: Vec::new(),
            excluded_directories: [
                ".git",
                ".hg",
                ".svn",
                "node_modules",
                "target",
                ".next",
                "dist",
                "build",
                ".cache",
                "vendor",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect(),
            binary_probe_bytes: 8 * 1024,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct FileKey {
    pub root_index: usize,
    pub relative_path: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IndexedFile {
    pub key: FileKey,
    pub size: u64,
    pub modified_at_ms: u64,
    pub language: Option<String>,
    pub binary: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IndexSnapshot {
    pub workspace_id: WorkspaceId,
    pub roots: Vec<PathBuf>,
    pub generation: u64,
    pub scan_duration_ms: u64,
    pub files: Vec<IndexedFile>,
}

#[derive(Clone)]
pub struct FileIndex {
    options: IndexOptions,
    states: Arc<RwLock<BTreeMap<WorkspaceId, WorkspaceIndex>>>,
}

#[derive(Clone, Debug)]
struct WorkspaceIndex {
    roots: Vec<PathBuf>,
    generation: u64,
    scan_duration_ms: u64,
    files: BTreeMap<FileKey, IndexedFile>,
}

impl FileIndex {
    pub fn new(options: IndexOptions) -> Self {
        Self {
            options,
            states: Arc::new(RwLock::new(BTreeMap::new())),
        }
    }

    /// 在 blocking 池完成扫描，完整结果就绪后一次性替换旧索引。
    pub async fn scan_workspace(
        &self,
        workspace: &Workspace,
    ) -> Result<IndexSnapshot, FileIndexError> {
        let workspace = workspace.clone();
        let workspace_id = workspace.id.clone();
        let roots = workspace
            .roots
            .iter()
            .map(|root| root.path.clone())
            .collect::<Vec<_>>();
        let options = self.options.clone();
        let started = Instant::now();
        let files = tokio::task::spawn_blocking(move || scan(&workspace, &options))
            .await
            .map_err(|error| FileIndexError::Task(error.to_string()))??;
        let duration = started.elapsed().as_millis().try_into().unwrap_or(u64::MAX);
        let mut guard = self.states.write().map_err(|_| FileIndexError::Poisoned)?;
        let generation = guard
            .get(&workspace_id)
            .map_or(1, |state| state.generation.saturating_add(1));
        guard.insert(
            workspace_id.clone(),
            WorkspaceIndex {
                roots,
                generation,
                scan_duration_ms: duration,
                files,
            },
        );
        drop(guard);
        self.snapshot(&workspace_id)?
            .ok_or_else(|| FileIndexError::WorkspaceNotIndexed(workspace_id.to_string()))
    }

    pub fn snapshot(
        &self,
        workspace_id: &WorkspaceId,
    ) -> Result<Option<IndexSnapshot>, FileIndexError> {
        let guard = self.states.read().map_err(|_| FileIndexError::Poisoned)?;
        Ok(guard.get(workspace_id).map(|state| IndexSnapshot {
            workspace_id: workspace_id.clone(),
            roots: state.roots.clone(),
            generation: state.generation,
            scan_duration_ms: state.scan_duration_ms,
            files: state.files.values().cloned().collect(),
        }))
    }

    pub fn search(
        &self,
        workspace_id: &WorkspaceId,
        query: &str,
        limit: usize,
    ) -> Result<Vec<IndexedFile>, FileIndexError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let query = query.to_ascii_lowercase();
        let guard = self.states.read().map_err(|_| FileIndexError::Poisoned)?;
        let state = guard
            .get(workspace_id)
            .ok_or_else(|| FileIndexError::WorkspaceNotIndexed(workspace_id.to_string()))?;
        let mut matches = state
            .files
            .values()
            .filter_map(|file| {
                fuzzy_score(&file.key.relative_path, &query).map(|score| (score, file.clone()))
            })
            .collect::<Vec<_>>();
        matches.sort_by(|(left_score, left), (right_score, right)| {
            right_score
                .cmp(left_score)
                .then_with(|| left.key.cmp(&right.key))
        });
        matches.truncate(limit);
        Ok(matches.into_iter().map(|(_, file)| file).collect())
    }

    pub async fn apply_changes(
        &self,
        workspace: &Workspace,
        changes: &[PathChange],
    ) -> Result<IndexSnapshot, FileIndexError> {
        if changes.is_empty() {
            return self
                .snapshot(&workspace.id)?
                .ok_or_else(|| FileIndexError::WorkspaceNotIndexed(workspace.id.to_string()));
        }
        if self.requires_rescan(workspace, changes)? {
            return self.scan_workspace(workspace).await;
        }
        let workspace = workspace.clone();
        let workspace_id = workspace.id.clone();
        let options = self.options.clone();
        let changes = changes.to_vec();
        let updates = tokio::task::spawn_blocking(move || {
            incremental_updates(&workspace, &options, &changes)
        })
        .await
        .map_err(|error| FileIndexError::Task(error.to_string()))??;
        let mut guard = self.states.write().map_err(|_| FileIndexError::Poisoned)?;
        let state = guard
            .get_mut(&workspace_id)
            .ok_or_else(|| FileIndexError::WorkspaceNotIndexed(workspace_id.to_string()))?;
        for (key, file) in updates {
            match file {
                Some(file) => {
                    state.files.insert(key, file);
                }
                None => {
                    state.files.remove(&key);
                }
            }
        }
        state.generation = state.generation.saturating_add(1);
        drop(guard);
        self.snapshot(&workspace_id)?
            .ok_or_else(|| FileIndexError::WorkspaceNotIndexed(workspace_id.to_string()))
    }

    pub fn start_debounced_updates(
        &self,
        workspace: Workspace,
        debounce: Duration,
    ) -> DebouncedUpdateHandle {
        DebouncedUpdateHandle::start(self.clone(), workspace, debounce)
    }

    pub fn watch_workspace(
        &self,
        workspace: Workspace,
        debounce: Duration,
    ) -> Result<WorkspaceWatcher, FileIndexError> {
        let updates = self.start_debounced_updates(workspace.clone(), debounce);
        let sender = updates.sender.clone();
        let errors = updates.errors.clone();
        let dropped_events = updates.dropped_events.clone();
        let mut watcher =
            notify::recommended_watcher(move |result: notify::Result<Event>| match result {
                Ok(event) => {
                    let kind = if matches!(event.kind, EventKind::Remove(_)) {
                        ChangeKind::Remove
                    } else {
                        ChangeKind::Upsert
                    };
                    for path in event.paths {
                        if !enqueue_watcher_change(
                            &sender,
                            &errors,
                            &dropped_events,
                            PathChange { path, kind },
                        ) {
                            break;
                        }
                    }
                }
                Err(error) => push_error(&errors, error.to_string()),
            })?;
        for root in &workspace.roots {
            watcher.watch(&root.path, RecursiveMode::Recursive)?;
        }
        Ok(WorkspaceWatcher {
            _watcher: watcher,
            updates,
        })
    }

    fn requires_rescan(
        &self,
        workspace: &Workspace,
        changes: &[PathChange],
    ) -> Result<bool, FileIndexError> {
        let guard = self.states.read().map_err(|_| FileIndexError::Poisoned)?;
        let state = guard
            .get(&workspace.id)
            .ok_or_else(|| FileIndexError::WorkspaceNotIndexed(workspace.id.to_string()))?;
        for change in changes {
            let normalized = normalize_event_path(&change.path);
            if normalized.is_dir() {
                return Ok(true);
            }
            if !normalized.exists() {
                for file in state.files.values() {
                    if let Some(absolute) = absolute_path(&state.roots, &file.key) {
                        if absolute.starts_with(&normalized) && absolute != normalized {
                            return Ok(true);
                        }
                    }
                }
            }
        }
        Ok(false)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChangeKind {
    Upsert,
    Remove,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PathChange {
    pub path: PathBuf,
    pub kind: ChangeKind,
}

pub struct DebouncedUpdateHandle {
    sender: mpsc::Sender<PathChange>,
    stop: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<()>>,
    errors: Arc<Mutex<ErrorLog>>,
    dropped_events: Arc<AtomicU64>,
}

impl DebouncedUpdateHandle {
    fn start(index: FileIndex, workspace: Workspace, debounce: Duration) -> Self {
        let (sender, receiver) = mpsc::channel(256);
        let (stop_tx, stop_rx) = oneshot::channel();
        let errors = Arc::new(Mutex::new(ErrorLog::default()));
        let dropped_events = Arc::new(AtomicU64::new(0));
        let task_errors = errors.clone();
        let task = tokio::spawn(debounce_loop(
            index,
            workspace,
            debounce,
            receiver,
            stop_rx,
            task_errors,
        ));
        Self {
            sender,
            stop: Some(stop_tx),
            task: Some(task),
            errors,
            dropped_events,
        }
    }

    pub async fn submit(&self, change: PathChange) -> Result<(), FileIndexError> {
        self.sender
            .send(change)
            .await
            .map_err(|_| FileIndexError::WatcherStopped)
    }

    pub fn errors(&self) -> Vec<String> {
        self.errors
            .lock()
            .map(|errors| errors.snapshot())
            .unwrap_or_else(|_| vec!["watcher error lock poisoned".into()])
    }

    /// 错误缓冲是否因超过上限发生过截断。
    pub fn errors_truncated(&self) -> bool {
        self.errors
            .lock()
            .map(|errors| errors.truncated)
            .unwrap_or(false)
    }

    /// watcher 回调因有界通道满而丢弃的事件总数。
    pub fn dropped_events(&self) -> u64 {
        self.dropped_events.load(Ordering::Relaxed)
    }

    pub async fn shutdown(mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(task) = self.task.take() {
            let _ = task.await;
        }
    }
}

impl Drop for DebouncedUpdateHandle {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
    }
}

pub struct WorkspaceWatcher {
    _watcher: RecommendedWatcher,
    updates: DebouncedUpdateHandle,
}

impl WorkspaceWatcher {
    pub fn errors(&self) -> Vec<String> {
        self.updates.errors()
    }

    pub fn errors_truncated(&self) -> bool {
        self.updates.errors_truncated()
    }

    pub fn dropped_events(&self) -> u64 {
        self.updates.dropped_events()
    }

    pub async fn shutdown(self) {
        self.updates.shutdown().await;
    }
}

async fn debounce_loop(
    index: FileIndex,
    workspace: Workspace,
    debounce: Duration,
    mut receiver: mpsc::Receiver<PathChange>,
    mut stop: oneshot::Receiver<()>,
    errors: Arc<Mutex<ErrorLog>>,
) {
    loop {
        let first = tokio::select! {
            _ = &mut stop => return,
            change = receiver.recv() => match change { Some(change) => change, None => return },
        };
        let mut changes = BTreeMap::new();
        changes.insert(path_sort_key(&first.path), first);
        let sleep = tokio::time::sleep(debounce);
        tokio::pin!(sleep);
        loop {
            tokio::select! {
                _ = &mut stop => return,
                _ = &mut sleep => break,
                change = receiver.recv() => match change {
                    Some(change) => { changes.insert(path_sort_key(&change.path), change); }
                    None => break,
                },
            }
        }
        let batch = changes.into_values().collect::<Vec<_>>();
        if let Err(error) = index.apply_changes(&workspace, &batch).await {
            push_error(&errors, error.to_string());
        }
    }
}

fn scan(
    workspace: &Workspace,
    options: &IndexOptions,
) -> Result<BTreeMap<FileKey, IndexedFile>, FileIndexError> {
    let mut files = BTreeMap::new();
    for (root_index, root) in workspace.roots.iter().enumerate() {
        let excluded = options.excluded_directories.clone();
        let mut builder = WalkBuilder::new(&root.path);
        builder
            .hidden(false)
            .parents(true)
            .git_ignore(true)
            .git_exclude(true)
            .require_git(false)
            .follow_links(false)
            .filter_entry(move |entry| {
                entry.depth() == 0
                    || !entry.file_type().is_some_and(|kind| kind.is_dir())
                    || !excluded.contains(entry.file_name().to_string_lossy().as_ref())
            });
        for ignore_file in options
            .global_ignore_files
            .iter()
            .chain(options.workspace_ignore_files.iter())
        {
            if let Some(error) = builder.add_ignore(ignore_file) {
                return Err(FileIndexError::Ignore(error.to_string()));
            }
        }
        for entry in builder.build() {
            let entry = entry.map_err(|error| FileIndexError::Ignore(error.to_string()))?;
            if !entry.file_type().is_some_and(|kind| kind.is_file()) {
                continue;
            }
            let relative = entry
                .path()
                .strip_prefix(&root.path)
                .map_err(|_| FileIndexError::OutsideRoot(entry.path().to_path_buf()))?;
            let key = FileKey {
                root_index,
                relative_path: normalize_relative(relative),
            };
            files.insert(
                key.clone(),
                inspect_file(entry.path(), key, options.binary_probe_bytes)?,
            );
        }
    }
    Ok(files)
}

fn incremental_updates(
    workspace: &Workspace,
    options: &IndexOptions,
    changes: &[PathChange],
) -> Result<Vec<(FileKey, Option<IndexedFile>)>, FileIndexError> {
    let mut updates = BTreeMap::new();
    for change in changes {
        let Some((root_index, root, relative, absolute)) = locate_root(workspace, &change.path)
        else {
            continue;
        };
        let key = FileKey {
            root_index,
            relative_path: normalize_relative(&relative),
        };
        let remove = change.kind == ChangeKind::Remove
            || !absolute.is_file()
            || has_excluded_component(&relative, &options.excluded_directories)
            || is_ignored(root, &absolute, options)?;
        let value = if remove {
            None
        } else {
            Some(inspect_file(
                &absolute,
                key.clone(),
                options.binary_probe_bytes,
            )?)
        };
        updates.insert(key, value);
    }
    Ok(updates.into_iter().collect())
}

fn locate_root<'a>(
    workspace: &'a Workspace,
    path: &Path,
) -> Option<(usize, &'a Path, PathBuf, PathBuf)> {
    let absolute = normalize_event_path(path);
    workspace
        .roots
        .iter()
        .enumerate()
        .find_map(|(index, root)| {
            absolute.strip_prefix(&root.path).ok().map(|relative| {
                (
                    index,
                    root.path.as_path(),
                    relative.to_path_buf(),
                    absolute.clone(),
                )
            })
        })
}

fn normalize_event_path(path: &Path) -> PathBuf {
    if let Ok(canonical) = fs::canonicalize(path) {
        return dunce::simplified(&canonical).to_path_buf();
    }
    if let (Some(parent), Some(name)) = (path.parent(), path.file_name()) {
        if let Ok(parent) = fs::canonicalize(parent) {
            return dunce::simplified(&parent).join(name);
        }
    }
    path.to_path_buf()
}

fn is_ignored(root: &Path, path: &Path, options: &IndexOptions) -> Result<bool, FileIndexError> {
    let mut builder = GitignoreBuilder::new(root);
    for external in options
        .global_ignore_files
        .iter()
        .chain(options.workspace_ignore_files.iter())
    {
        if external.is_file() {
            if let Some(error) = builder.add(external) {
                return Err(FileIndexError::Ignore(error.to_string()));
            }
        }
    }
    let mut current = Some(root);
    let parent = path.parent().unwrap_or(root);
    while let Some(directory) = current {
        let ignore_file = directory.join(".gitignore");
        if ignore_file.is_file() {
            if let Some(error) = builder.add(ignore_file) {
                return Err(FileIndexError::Ignore(error.to_string()));
            }
        }
        if directory == parent {
            break;
        }
        current = next_child_on_path(directory, parent);
    }
    let matcher = builder
        .build()
        .map_err(|error| FileIndexError::Ignore(error.to_string()))?;
    Ok(matcher.matched_path_or_any_parents(path, false).is_ignore())
}

fn next_child_on_path<'a>(current: &'a Path, target: &'a Path) -> Option<&'a Path> {
    target
        .ancestors()
        .take_while(|ancestor| *ancestor != current)
        .last()
}

fn inspect_file(
    path: &Path,
    key: FileKey,
    probe_bytes: u64,
) -> Result<IndexedFile, FileIndexError> {
    let metadata = fs::metadata(path).map_err(|source| FileIndexError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let mut sample = Vec::new();
    File::open(path)
        .map_err(|source| FileIndexError::Io {
            path: path.to_path_buf(),
            source,
        })?
        .take(probe_bytes)
        .read_to_end(&mut sample)
        .map_err(|source| FileIndexError::Io {
            path: path.to_path_buf(),
            source,
        })?;
    let modified_at_ms = metadata
        .modified()
        .unwrap_or(SystemTime::UNIX_EPOCH)
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX);
    Ok(IndexedFile {
        key,
        size: metadata.len(),
        modified_at_ms,
        language: language_for(path),
        binary: is_binary(&sample),
    })
}

fn is_binary(sample: &[u8]) -> bool {
    if sample.contains(&0) {
        return true;
    }
    let suspicious = sample
        .iter()
        .filter(|byte| **byte < 0x09 || (**byte > 0x0d && **byte < 0x20))
        .count();
    !sample.is_empty() && suspicious * 10 > sample.len()
}

fn language_for(path: &Path) -> Option<String> {
    let language = match path
        .extension()?
        .to_string_lossy()
        .to_ascii_lowercase()
        .as_str()
    {
        "rs" => "rust",
        "toml" => "toml",
        "md" => "markdown",
        "json" => "json",
        "js" | "mjs" | "cjs" => "javascript",
        "ts" | "tsx" => "typescript",
        "py" => "python",
        "go" => "go",
        "java" => "java",
        "c" | "h" => "c",
        "cc" | "cpp" | "hpp" => "cpp",
        "yml" | "yaml" => "yaml",
        "sh" => "shell",
        "ps1" => "powershell",
        _ => return None,
    };
    Some(language.into())
}

fn fuzzy_score(path: &str, query: &str) -> Option<i64> {
    if query.is_empty() {
        return Some(0);
    }
    let candidate = path.to_ascii_lowercase();
    if let Some(position) = candidate.find(query) {
        let file_name_bonus = Path::new(path)
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.to_ascii_lowercase().contains(query));
        return Some(10_000 - position as i64 + if file_name_bonus { 5_000 } else { 0 });
    }
    let mut cursor = 0usize;
    let mut score = 0i64;
    for needle in query.chars() {
        let found = candidate[cursor..].find(needle)?;
        cursor += found + needle.len_utf8();
        score += 100 - found.min(100) as i64;
    }
    Some(score)
}

fn has_excluded_component(path: &Path, excluded: &BTreeSet<String>) -> bool {
    path.components()
        .any(|component| excluded.contains(component.as_os_str().to_string_lossy().as_ref()))
}

fn normalize_relative(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn absolute_path(roots: &[PathBuf], key: &FileKey) -> Option<PathBuf> {
    roots
        .get(key.root_index)
        .map(|root| root.join(&key.relative_path))
}

fn path_sort_key(path: &Path) -> String {
    let value = path.to_string_lossy().replace('\\', "/");
    if cfg!(windows) {
        value.to_ascii_lowercase()
    } else {
        value
    }
}

#[derive(Clone, Debug, Default)]
struct ErrorLog {
    entries: VecDeque<String>,
    truncated: bool,
}

impl ErrorLog {
    fn push(&mut self, error: String) {
        if self.entries.len() >= MAX_WATCHER_ERRORS {
            self.entries.pop_front();
            self.truncated = true;
        }
        self.entries.push_back(error);
    }

    fn snapshot(&self) -> Vec<String> {
        if !self.truncated {
            return self.entries.iter().cloned().collect();
        }

        // 截断标记也计入 1024 上限，避免导出端重新变成无界集合。
        let mut out = Vec::with_capacity(MAX_WATCHER_ERRORS);
        out.push(ERRORS_TRUNCATED_MARKER.into());
        out.extend(
            self.entries
                .iter()
                .skip(self.entries.len().saturating_sub(MAX_WATCHER_ERRORS - 1))
                .cloned(),
        );
        out
    }
}

fn enqueue_watcher_change(
    sender: &mpsc::Sender<PathChange>,
    errors: &Mutex<ErrorLog>,
    dropped_events: &AtomicU64,
    change: PathChange,
) -> bool {
    match sender.try_send(change) {
        Ok(()) => true,
        Err(mpsc::error::TrySendError::Full(_)) => {
            let previous = dropped_events
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                    Some(current.saturating_add(1))
                })
                .unwrap_or(u64::MAX);
            let total = previous.saturating_add(1);
            // notify 回调不得等待锁；计数由 Atomic 保证，文本诊断在锁竞争时 best-effort。
            if let Ok(mut errors) = errors.try_lock() {
                errors.push(format!(
                    "file-index update channel full; event dropped (total: {total})"
                ));
            }
            true
        }
        Err(mpsc::error::TrySendError::Closed(_)) => {
            if let Ok(mut errors) = errors.try_lock() {
                errors.push("file-index update channel closed".into());
            }
            false
        }
    }
}

fn push_error(errors: &Mutex<ErrorLog>, error: String) {
    if let Ok(mut errors) = errors.lock() {
        errors.push(error);
    }
}

#[derive(Debug, Error)]
pub enum FileIndexError {
    #[error("workspace is not indexed: {0}")]
    WorkspaceNotIndexed(String),
    #[error("file index lock is poisoned")]
    Poisoned,
    #[error("file is outside all workspace roots: {0}")]
    OutsideRoot(PathBuf),
    #[error("I/O error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("ignore rule error: {0}")]
    Ignore(String),
    #[error("index worker failed: {0}")]
    Task(String),
    #[error("file watcher stopped")]
    WatcherStopped,
    #[error(transparent)]
    Notify(#[from] notify::Error),
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use agent_domain::Timestamp;
    use workspace_service::WorkspaceService;

    use super::*;

    static NEXT_TEMP: AtomicU64 = AtomicU64::new(1);

    fn temp_dir(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "pawork-file-index-{}-{}-{name}",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).expect("create temp directory");
        path
    }

    fn workspace(root: &Path) -> Workspace {
        WorkspaceService::new()
            .add(
                WorkspaceId::from("workspace-1"),
                "test",
                [root],
                Timestamp::from_unix_millis(1),
            )
            .expect("workspace")
    }

    #[tokio::test]
    async fn scan_respects_gitignore_and_large_directory_exclusions() {
        let root = temp_dir("scan");
        fs::create_dir_all(root.join("src")).expect("src");
        fs::create_dir_all(root.join("node_modules").join("pkg")).expect("node modules");
        fs::create_dir_all(root.join("target")).expect("target");
        fs::write(root.join("src").join("lib.rs"), "fn main() {}\n").expect("source");
        fs::write(root.join("ignored.log"), "ignore\n").expect("ignored");
        fs::write(root.join(".gitignore"), "*.log\n").expect("gitignore");
        fs::write(root.join("node_modules").join("pkg").join("index.js"), "x").expect("noise");
        fs::write(root.join("target").join("artifact"), "x").expect("target file");
        let workspace = workspace(&root);
        let index = FileIndex::new(IndexOptions::default());
        let snapshot = index.scan_workspace(&workspace).await.expect("scan");
        let paths = snapshot
            .files
            .iter()
            .map(|file| file.key.relative_path.as_str())
            .collect::<Vec<_>>();
        assert!(paths.contains(&"src/lib.rs"));
        assert!(!paths.iter().any(|path| path.contains("node_modules")
            || path.starts_with("target/")
            || path.ends_with(".log")));
        let source = snapshot
            .files
            .iter()
            .find(|file| file.key.relative_path == "src/lib.rs")
            .expect("source");
        assert_eq!(source.language.as_deref(), Some("rust"));
        assert!(!source.binary);
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn incremental_and_debounced_updates_add_modify_and_remove() {
        let root = temp_dir("incremental");
        let workspace = workspace(&root);
        let index = FileIndex::new(IndexOptions::default());
        index.scan_workspace(&workspace).await.expect("scan");
        let file = root.join("new.rs");
        fs::write(&file, "fn first() {}\n").expect("write");
        let handle = index.start_debounced_updates(workspace.clone(), Duration::from_millis(20));
        handle
            .submit(PathChange {
                path: file.clone(),
                kind: ChangeKind::Upsert,
            })
            .await
            .expect("submit");
        tokio::time::sleep(Duration::from_millis(60)).await;
        let matches = index.search(&workspace.id, "new.rs", 10).expect("search");
        assert_eq!(matches.len(), 1);
        fs::write(&file, [0, 1, 2]).expect("binary");
        handle
            .submit(PathChange {
                path: file.clone(),
                kind: ChangeKind::Upsert,
            })
            .await
            .expect("submit");
        tokio::time::sleep(Duration::from_millis(60)).await;
        let snapshot = index
            .snapshot(&workspace.id)
            .expect("snapshot")
            .expect("indexed");
        assert!(
            snapshot
                .files
                .iter()
                .find(|entry| entry.key.relative_path == "new.rs")
                .expect("entry")
                .binary
        );
        fs::remove_file(&file).expect("remove");
        handle
            .submit(PathChange {
                path: file,
                kind: ChangeKind::Remove,
            })
            .await
            .expect("submit");
        tokio::time::sleep(Duration::from_millis(60)).await;
        assert!(index
            .search(&workspace.id, "new.rs", 10)
            .expect("search")
            .is_empty());
        handle.shutdown().await;
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn try_send_does_not_block_when_channel_full() {
        // 直接调用 watch_workspace 回调使用的 helper，验证满通道路径不阻塞且可计数。
        // 不能挂 debounce 消费者，否则窗口内会持续 drain，通道填不满。
        let (sender, _receiver) = mpsc::channel::<PathChange>(256);
        for i in 0..256 {
            sender
                .try_send(PathChange {
                    path: PathBuf::from(format!("file-{i}.rs")),
                    kind: ChangeKind::Upsert,
                })
                .expect("fill channel");
        }

        let errors = Mutex::new(ErrorLog::default());
        let dropped_events = AtomicU64::new(0);
        let started = Instant::now();
        let should_continue = enqueue_watcher_change(
            &sender,
            &errors,
            &dropped_events,
            PathChange {
                path: PathBuf::from("overflow.rs"),
                kind: ChangeKind::Upsert,
            },
        );
        assert!(
            started.elapsed() < Duration::from_millis(50),
            "try_send must not block"
        );
        assert!(should_continue);

        let errors = errors.lock().expect("errors lock");
        assert_eq!(dropped_events.load(Ordering::Relaxed), 1);
        let snapshot = errors.snapshot();
        assert!(
            snapshot.iter().any(|error| error.contains("channel full")),
            "full-channel drops must be observable: {snapshot:?}"
        );
    }

    #[test]
    fn watcher_errors_cap_at_1024_and_mark_truncation() {
        let mut log = ErrorLog::default();
        for i in 0..1100 {
            log.push(format!("error-{i}"));
        }
        assert!(log.truncated);
        assert_eq!(log.entries.len(), MAX_WATCHER_ERRORS);
        assert_eq!(log.entries.front().map(String::as_str), Some("error-76"));
        assert_eq!(log.entries.back().map(String::as_str), Some("error-1099"));
        let snapshot = log.snapshot();
        assert_eq!(snapshot[0], ERRORS_TRUNCATED_MARKER);
        assert_eq!(snapshot.len(), MAX_WATCHER_ERRORS);
        assert_eq!(snapshot[1], "error-77");
        assert_eq!(snapshot.last().map(String::as_str), Some("error-1099"));
    }
}
