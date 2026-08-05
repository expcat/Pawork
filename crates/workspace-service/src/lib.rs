//! 工作区目录服务。
//!
//! 本 crate 只管理工作区语义与快照，不执行 Git 命令，也不访问 Provider、Tool 或 GUI。

use std::{
    collections::{BTreeMap, HashSet},
    fs,
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
};

use agent_domain::{Timestamp, WorkspaceId};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrustState {
    #[default]
    Untrusted,
    Trusted,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitRepository {
    /// 普通仓库/worktree 的工作树；bare 仓库为 `None`。
    pub work_tree: Option<PathBuf>,
    pub git_dir: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceRoot {
    pub path: PathBuf,
    pub git: Option<GitRepository>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Workspace {
    pub id: WorkspaceId,
    pub name: String,
    pub roots: Vec<WorkspaceRoot>,
    pub trust: TrustState,
    pub last_accessed_at: Timestamp,
    pub revision: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceSnapshot {
    pub workspaces: Vec<Workspace>,
}

#[derive(Clone, Default)]
pub struct WorkspaceService {
    workspaces: Arc<RwLock<BTreeMap<WorkspaceId, Workspace>>>,
}

impl WorkspaceService {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_snapshot(snapshot: WorkspaceSnapshot) -> Result<Self, WorkspaceError> {
        let mut workspaces = BTreeMap::new();
        for mut workspace in snapshot.workspaces {
            if workspace.roots.is_empty() {
                return Err(WorkspaceError::NoRoots);
            }
            workspace.roots = normalize_roots(workspace.roots.into_iter().map(|root| root.path))?;
            if workspaces.insert(workspace.id.clone(), workspace).is_some() {
                return Err(WorkspaceError::DuplicateWorkspaceId);
            }
        }
        Ok(Self {
            workspaces: Arc::new(RwLock::new(workspaces)),
        })
    }

    pub fn add<I, P>(
        &self,
        id: WorkspaceId,
        name: impl Into<String>,
        roots: I,
        now: Timestamp,
    ) -> Result<Workspace, WorkspaceError>
    where
        I: IntoIterator<Item = P>,
        P: Into<PathBuf>,
    {
        let roots = normalize_roots(roots.into_iter().map(Into::into))?;
        let mut guard = self
            .workspaces
            .write()
            .map_err(|_| WorkspaceError::Poisoned)?;
        if guard.contains_key(&id) {
            return Err(WorkspaceError::AlreadyExists(id.to_string()));
        }
        let workspace = Workspace {
            id: id.clone(),
            name: name.into(),
            roots,
            trust: TrustState::Untrusted,
            last_accessed_at: now,
            revision: 1,
        };
        guard.insert(id, workspace.clone());
        Ok(workspace)
    }

    pub fn remove(&self, id: &WorkspaceId) -> Result<Workspace, WorkspaceError> {
        self.workspaces
            .write()
            .map_err(|_| WorkspaceError::Poisoned)?
            .remove(id)
            .ok_or_else(|| WorkspaceError::NotFound(id.to_string()))
    }

    pub fn rename(&self, id: &WorkspaceId, name: impl Into<String>) -> Result<u64, WorkspaceError> {
        self.update(id, |workspace| workspace.name = name.into())
    }

    pub fn set_trust(&self, id: &WorkspaceId, trust: TrustState) -> Result<u64, WorkspaceError> {
        self.update(id, |workspace| workspace.trust = trust)
    }

    pub fn touch(&self, id: &WorkspaceId, now: Timestamp) -> Result<u64, WorkspaceError> {
        self.update(id, |workspace| workspace.last_accessed_at = now)
    }

    pub fn add_root(
        &self,
        id: &WorkspaceId,
        path: impl Into<PathBuf>,
    ) -> Result<u64, WorkspaceError> {
        let root = normalize_root(path.into())?;
        self.update(id, |workspace| {
            if !workspace
                .roots
                .iter()
                .any(|existing| paths_equal(&existing.path, &root.path))
            {
                workspace.roots.push(root);
                workspace.roots.sort_by_key(|root| path_key(&root.path));
            }
        })
    }

    pub fn remove_root(&self, id: &WorkspaceId, path: &Path) -> Result<u64, WorkspaceError> {
        let canonical = fs::canonicalize(path).map_err(|source| WorkspaceError::InvalidRoot {
            path: path.to_path_buf(),
            source,
        })?;
        let mut guard = self
            .workspaces
            .write()
            .map_err(|_| WorkspaceError::Poisoned)?;
        let workspace = guard
            .get_mut(id)
            .ok_or_else(|| WorkspaceError::NotFound(id.to_string()))?;
        let position = workspace
            .roots
            .iter()
            .position(|root| paths_equal(&root.path, &canonical))
            .ok_or_else(|| WorkspaceError::RootNotFound(canonical.clone()))?;
        if workspace.roots.len() == 1 {
            return Err(WorkspaceError::CannotRemoveLastRoot);
        }
        workspace.roots.remove(position);
        workspace.revision = workspace
            .revision
            .checked_add(1)
            .ok_or(WorkspaceError::RevisionOverflow)?;
        Ok(workspace.revision)
    }

    pub fn get(&self, id: &WorkspaceId) -> Result<Option<Workspace>, WorkspaceError> {
        Ok(self
            .workspaces
            .read()
            .map_err(|_| WorkspaceError::Poisoned)?
            .get(id)
            .cloned())
    }

    pub fn list(&self) -> Result<Vec<Workspace>, WorkspaceError> {
        let mut values: Vec<_> = self
            .workspaces
            .read()
            .map_err(|_| WorkspaceError::Poisoned)?
            .values()
            .cloned()
            .collect();
        values.sort_by(|left, right| {
            right
                .last_accessed_at
                .cmp(&left.last_accessed_at)
                .then_with(|| left.id.cmp(&right.id))
        });
        Ok(values)
    }

    pub fn snapshot(&self) -> Result<WorkspaceSnapshot, WorkspaceError> {
        let workspaces = self
            .workspaces
            .read()
            .map_err(|_| WorkspaceError::Poisoned)?
            .values()
            .cloned()
            .collect();
        Ok(WorkspaceSnapshot { workspaces })
    }

    fn update(
        &self,
        id: &WorkspaceId,
        mutation: impl FnOnce(&mut Workspace),
    ) -> Result<u64, WorkspaceError> {
        let mut guard = self
            .workspaces
            .write()
            .map_err(|_| WorkspaceError::Poisoned)?;
        let workspace = guard
            .get_mut(id)
            .ok_or_else(|| WorkspaceError::NotFound(id.to_string()))?;
        mutation(workspace);
        workspace.revision = workspace
            .revision
            .checked_add(1)
            .ok_or(WorkspaceError::RevisionOverflow)?;
        Ok(workspace.revision)
    }
}

pub fn detect_git(path: &Path) -> Result<Option<GitRepository>, WorkspaceError> {
    let canonical = fs::canonicalize(path).map_err(|source| WorkspaceError::InvalidRoot {
        path: path.to_path_buf(),
        source,
    })?;
    for candidate in canonical.ancestors() {
        let marker = candidate.join(".git");
        if marker.is_dir() {
            return Ok(Some(GitRepository {
                work_tree: Some(candidate.to_path_buf()),
                git_dir: fs::canonicalize(&marker).map_err(|source| {
                    WorkspaceError::InvalidGitDir {
                        path: marker,
                        source,
                    }
                })?,
            }));
        }
        if marker.is_file() {
            let content =
                fs::read_to_string(&marker).map_err(|source| WorkspaceError::InvalidGitDir {
                    path: marker.clone(),
                    source,
                })?;
            let raw = content
                .lines()
                .next()
                .and_then(|line| line.strip_prefix("gitdir:"))
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| WorkspaceError::MalformedGitFile(marker.clone()))?;
            let raw = PathBuf::from(raw);
            let git_dir = if raw.is_absolute() {
                raw
            } else {
                candidate.join(raw)
            };
            let git_dir =
                fs::canonicalize(&git_dir).map_err(|source| WorkspaceError::InvalidGitDir {
                    path: git_dir,
                    source,
                })?;
            return Ok(Some(GitRepository {
                work_tree: Some(candidate.to_path_buf()),
                git_dir,
            }));
        }
        if candidate.join("HEAD").is_file() && candidate.join("objects").is_dir() {
            return Ok(Some(GitRepository {
                work_tree: None,
                git_dir: candidate.to_path_buf(),
            }));
        }
    }
    Ok(None)
}

fn normalize_roots<I>(roots: I) -> Result<Vec<WorkspaceRoot>, WorkspaceError>
where
    I: IntoIterator<Item = PathBuf>,
{
    let mut normalized = Vec::new();
    let mut seen = HashSet::new();
    for path in roots {
        let root = normalize_root(path)?;
        if seen.insert(path_key(&root.path)) {
            normalized.push(root);
        }
    }
    if normalized.is_empty() {
        return Err(WorkspaceError::NoRoots);
    }
    normalized.sort_by_key(|root| path_key(&root.path));
    Ok(normalized)
}

fn normalize_root(path: PathBuf) -> Result<WorkspaceRoot, WorkspaceError> {
    let canonical = fs::canonicalize(&path).map_err(|source| WorkspaceError::InvalidRoot {
        path: path.clone(),
        source,
    })?;
    if !canonical.is_dir() {
        return Err(WorkspaceError::RootIsNotDirectory(canonical));
    }
    let git = detect_git(&canonical)?;
    Ok(WorkspaceRoot {
        path: canonical,
        git,
    })
}

fn paths_equal(left: &Path, right: &Path) -> bool {
    path_key(left) == path_key(right)
}

fn path_key(path: &Path) -> String {
    let value = path.to_string_lossy().replace('\\', "/");
    if cfg!(windows) {
        value.to_ascii_lowercase()
    } else {
        value
    }
}

#[derive(Debug, Error)]
pub enum WorkspaceError {
    #[error("workspace already exists: {0}")]
    AlreadyExists(String),
    #[error("workspace not found: {0}")]
    NotFound(String),
    #[error("workspace must contain at least one root")]
    NoRoots,
    #[error("workspace snapshot contains a duplicate id")]
    DuplicateWorkspaceId,
    #[error("workspace root is invalid at {path}: {source}")]
    InvalidRoot {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("workspace root is not a directory: {0}")]
    RootIsNotDirectory(PathBuf),
    #[error("workspace root is not registered: {0}")]
    RootNotFound(PathBuf),
    #[error("cannot remove the last workspace root")]
    CannotRemoveLastRoot,
    #[error("invalid Git directory at {path}: {source}")]
    InvalidGitDir {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("malformed .git file: {0}")]
    MalformedGitFile(PathBuf),
    #[error("workspace revision overflow")]
    RevisionOverflow,
    #[error("workspace catalog lock is poisoned")]
    Poisoned,
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    static NEXT_TEMP: AtomicU64 = AtomicU64::new(1);

    fn temp_dir(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "pawork-workspace-{}-{}-{name}",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).expect("create temp directory");
        path
    }

    #[test]
    fn multiple_roots_are_canonical_deduplicated_and_snapshot_stable() {
        let first = temp_dir("first");
        let second = temp_dir("second");
        let service = WorkspaceService::new();
        let id = WorkspaceId::from("workspace-1");
        let workspace = service
            .add(
                id.clone(),
                "demo",
                [first.clone(), second.clone(), first.clone()],
                Timestamp::from_unix_millis(1),
            )
            .expect("add workspace");
        assert_eq!(workspace.roots.len(), 2);
        assert_eq!(workspace.trust, TrustState::Untrusted);
        service.set_trust(&id, TrustState::Trusted).expect("trust");
        service.rename(&id, "renamed").expect("rename");
        let snapshot = service.snapshot().expect("snapshot");
        let restored = WorkspaceService::from_snapshot(snapshot.clone()).expect("restore");
        assert_eq!(restored.snapshot().expect("snapshot"), snapshot);
        assert_eq!(
            serde_json::to_string(&snapshot).expect("serialize"),
            serde_json::to_string(&restored.snapshot().expect("snapshot")).expect("serialize")
        );
        let _ = fs::remove_dir_all(first);
        let _ = fs::remove_dir_all(second);
    }

    #[test]
    fn detects_directory_git_and_worktree_gitfile() {
        let repository = temp_dir("repository");
        fs::create_dir_all(repository.join(".git").join("objects")).expect("git dir");
        let nested = repository.join("src");
        fs::create_dir_all(&nested).expect("nested");
        let detected = detect_git(&nested)
            .expect("detect")
            .expect("git repository");
        assert_eq!(
            detected.work_tree,
            Some(fs::canonicalize(&repository).expect("canonical"))
        );

        let common = temp_dir("common-git-dir");
        let worktree = temp_dir("worktree");
        fs::write(
            worktree.join(".git"),
            format!("gitdir: {}\n", common.display()),
        )
        .expect("gitfile");
        let detected = detect_git(&worktree).expect("detect").expect("worktree");
        assert_eq!(
            detected.git_dir,
            fs::canonicalize(&common).expect("canonical")
        );
        let _ = fs::remove_dir_all(repository);
        let _ = fs::remove_dir_all(common);
        let _ = fs::remove_dir_all(worktree);
    }
}
