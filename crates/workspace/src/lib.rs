//! pawork-workspace：roots 管理、相对路径解析与文件索引。
//!
//! 路径安全校验（symlink / `.git` / TOCTOU）委托 `pawork-policy`，
//! `resolve_relative_path` 对外签名保持不变。

pub mod config;
pub mod import;
pub mod resources;

mod file_index;
mod path;

use std::{
    collections::{BTreeMap, HashSet},
    fs,
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
};

use pawork_domain::WorkspaceId;
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub use file_index::{
    ChangeKind, DebouncedUpdateHandle, FileIndex, FileIndexError, FileKey, IndexOptions,
    IndexSnapshot, IndexedFile, PathChange, WorkspaceWatcher,
};
pub use path::{resolve_relative_path, ResolvedPath, WorkspacePathError};

/// 已登记的工作区。`roots` 已经 canonicalize + `dunce::simplified`。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Workspace {
    pub id: WorkspaceId,
    pub name: String,
    pub roots: Vec<PathBuf>,
}

/// 进程内工作区目录服务。
#[derive(Clone, Default)]
pub struct WorkspaceService {
    workspaces: Arc<RwLock<BTreeMap<WorkspaceId, Workspace>>>,
}

impl WorkspaceService {
    pub fn new() -> Self {
        Self::default()
    }

    /// 以与 [`Self::add`] 相同的规则 canonicalize 单个 root。
    pub fn canonicalize_root(root: impl Into<PathBuf>) -> Result<PathBuf, WorkspaceError> {
        normalize_root(root.into())
    }

    pub fn add(
        &self,
        id: WorkspaceId,
        name: impl Into<String>,
        roots: impl IntoIterator<Item = impl Into<PathBuf>>,
    ) -> Result<Workspace, WorkspaceError> {
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
        };
        guard.insert(id, workspace.clone());
        Ok(workspace)
    }

    pub fn get(&self, id: &WorkspaceId) -> Result<Option<Workspace>, WorkspaceError> {
        Ok(self
            .workspaces
            .read()
            .map_err(|_| WorkspaceError::Poisoned)?
            .get(id)
            .cloned())
    }
}

fn normalize_roots(
    roots: impl IntoIterator<Item = PathBuf>,
) -> Result<Vec<PathBuf>, WorkspaceError> {
    let mut normalized = Vec::new();
    let mut seen = HashSet::new();
    for path in roots {
        let root = normalize_root(path)?;
        if seen.insert(path_key(&root)) {
            normalized.push(root);
        }
    }
    if normalized.is_empty() {
        return Err(WorkspaceError::NoRoots);
    }
    Ok(normalized)
}

fn normalize_root(path: PathBuf) -> Result<PathBuf, WorkspaceError> {
    let canonical =
        canonicalize_simplified(&path).map_err(|source| WorkspaceError::InvalidRoot {
            path: path.clone(),
            source,
        })?;
    if !canonical.is_dir() {
        return Err(WorkspaceError::RootIsNotDirectory(canonical));
    }
    Ok(canonical)
}

/// Canonicalize 工作区 root，并去掉 Windows verbatim 前缀 `\\?\`。
fn canonicalize_simplified(path: &Path) -> std::io::Result<PathBuf> {
    fs::canonicalize(path).map(|canonical| dunce::simplified(&canonical).to_path_buf())
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
    #[error("workspace root is not a directory: {0}")]
    RootIsNotDirectory(PathBuf),
    #[error("workspace root is invalid at {path}: {source}")]
    InvalidRoot {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("workspace snapshot contains a duplicate id")]
    DuplicateWorkspaceId,
    #[error("workspace catalog lock is poisoned")]
    Poisoned,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn simplified_canonical(path: &Path) -> PathBuf {
        let canonical = fs::canonicalize(path).expect("canonicalize test path");
        dunce::simplified(&canonical).to_path_buf()
    }

    #[test]
    fn add_deduplicates_and_canonicalizes_multiple_roots() {
        let first = tempfile::tempdir().expect("first root");
        let second = tempfile::tempdir().expect("second root");
        let service = WorkspaceService::new();
        let id = WorkspaceId::from("workspace-1");
        let workspace = service
            .add(
                id.clone(),
                "demo",
                [
                    first.path().to_path_buf(),
                    second.path().to_path_buf(),
                    first.path().to_path_buf(),
                ],
            )
            .expect("add workspace");

        assert_eq!(workspace.roots.len(), 2);
        assert_eq!(workspace.roots[0], simplified_canonical(first.path()));
        assert_eq!(workspace.roots[1], simplified_canonical(second.path()));
        assert_eq!(
            WorkspaceService::canonicalize_root(first.path()).expect("canonical root"),
            workspace.roots[0]
        );
        assert!(workspace
            .roots
            .iter()
            .all(|root| *root == simplified_canonical(root)));
        #[cfg(windows)]
        assert!(workspace
            .roots
            .iter()
            .all(|root| !root.to_string_lossy().starts_with(r"\\?\")));

        let loaded = service.get(&id).expect("get").expect("present");
        assert_eq!(loaded, workspace);
    }

    #[cfg(windows)]
    #[test]
    fn windows_dedupes_roots_case_insensitively() {
        let dir = tempfile::tempdir().expect("root");
        let upper = PathBuf::from(dir.path().to_string_lossy().to_uppercase());
        let workspace = WorkspaceService::new()
            .add(
                WorkspaceId::from("workspace-case"),
                "demo",
                [dir.path().to_path_buf(), upper],
            )
            .expect("add workspace");
        assert_eq!(workspace.roots.len(), 1);
        assert!(!workspace.roots[0].to_string_lossy().starts_with(r"\\?\"));
    }
}
