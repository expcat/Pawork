use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Component, Path, PathBuf},
};

use agent_domain::WorkspaceId;
use serde::{Deserialize, Serialize};

use crate::ResourceLoadError;

/// 已校验的工作区相对路径；拒绝绝对路径、前缀与 `..`。
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct WorkspaceRelativePath(PathBuf);

impl WorkspaceRelativePath {
    pub fn new(path: impl Into<PathBuf>) -> Result<Self, ResourceLoadError> {
        let path = path.into();
        if path.is_absolute()
            || path.components().any(|component| {
                matches!(
                    component,
                    Component::ParentDir | Component::RootDir | Component::Prefix(_)
                )
            })
        {
            return Err(ResourceLoadError::InvalidRelativePath(path));
        }
        let normalized = path
            .components()
            .filter_map(|component| match component {
                Component::Normal(part) => Some(part),
                Component::CurDir => None,
                Component::ParentDir | Component::RootDir | Component::Prefix(_) => None,
            })
            .collect();
        Ok(Self(normalized))
    }

    pub fn as_path(&self) -> &Path {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.as_os_str().is_empty()
    }

    pub fn validate(&self) -> Result<(), ResourceLoadError> {
        Self::new(self.0.clone()).map(|_| ())
    }
}

impl<'de> Deserialize<'de> for WorkspaceRelativePath {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let path = PathBuf::deserialize(deserializer)?;
        Self::new(path).map_err(|_| {
            serde::de::Error::custom(
                "workspace path must be relative and may not contain parent traversal",
            )
        })
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CurrentPathKind {
    Directory,
    #[default]
    File,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceSelection {
    #[serde(default)]
    pub active_skills: BTreeSet<String>,
    #[serde(default)]
    pub disabled_skills: BTreeSet<String>,
    pub prompt_template: Option<String>,
    #[serde(default)]
    pub prompt_arguments: BTreeMap<String, String>,
    pub profile: Option<String>,
    pub session_instructions: Option<String>,
    pub run_instructions: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceRequest {
    pub workspace_id: WorkspaceId,
    pub root_index: usize,
    pub current_path: WorkspaceRelativePath,
    pub current_path_kind: CurrentPathKind,
    #[serde(default)]
    pub selection: ResourceSelection,
}

impl ResourceRequest {
    pub fn new(
        workspace_id: WorkspaceId,
        root_index: usize,
        current_path: WorkspaceRelativePath,
    ) -> Self {
        Self {
            workspace_id,
            root_index,
            current_path,
            current_path_kind: CurrentPathKind::File,
            selection: ResourceSelection::default(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResourceLimits {
    /// 单个资源文件最大字节数。
    pub max_file_bytes: u64,
    /// 每类最多发现的资源数量。
    pub max_resources_per_kind: usize,
    /// 为后续递归 include 语义预留的深度上限。
    pub max_include_depth: usize,
    /// 单个 Prompt 声明和渲染时最多使用的文件引用数。
    pub max_template_file_refs: usize,
    /// Prompt 参数与文件引用展开后的总字节上限。
    pub max_rendered_prompt_bytes: u64,
}

impl Default for ResourceLimits {
    fn default() -> Self {
        Self {
            max_file_bytes: 1024 * 1024,
            max_resources_per_kind: 1024,
            max_include_depth: 16,
            max_template_file_refs: 32,
            max_rendered_prompt_bytes: 4 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ResourceLoaderOptions {
    /// 宿主解析出的用户全局资源目录；不得来自模型输入。
    pub global_resource_dir: Option<PathBuf>,
    pub workspace_resource_dir: String,
    pub limits: ResourceLimits,
}

impl Default for ResourceLoaderOptions {
    fn default() -> Self {
        Self {
            global_resource_dir: None,
            workspace_resource_dir: ".pawork".into(),
            limits: ResourceLimits::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_relative_path_rejects_escape_and_absolute_paths() {
        assert!(WorkspaceRelativePath::new("src/lib.rs").is_ok());
        assert!(WorkspaceRelativePath::new("./src/./lib.rs").is_ok());
        assert!(WorkspaceRelativePath::new("../secret").is_err());
        assert!(WorkspaceRelativePath::new("/tmp/secret").is_err());
    }

    #[test]
    fn deserialization_preserves_relative_path_invariant() {
        assert!(serde_json::from_str::<WorkspaceRelativePath>(r#""src/lib.rs""#).is_ok());
        assert!(serde_json::from_str::<WorkspaceRelativePath>(r#""../secret""#).is_err());
        assert!(serde_json::from_str::<WorkspaceRelativePath>(r#""/tmp/secret""#).is_err());
    }
}
