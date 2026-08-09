//! `AGENTS.md` 根与路径层级发现。
//!
//! 从工作区根到当前路径所在目录逐层查找 `AGENTS.md`，按深度稳定排序（根在前、
//! 靠近当前路径的在后，优先级最高）。单个文件的 I/O、UTF-8、超限或越界 symlink
//! 错误被隔离为 [`ResourceIssue`]，不影响其他层级加载。来源只保留 `root_index`
//! 与相对路径，绝不泄漏宿主绝对路径。

use std::path::{Path, PathBuf};

use config_service::ConfigTier;

use crate::{
    error::ResourceFileError,
    io,
    request::{CurrentPathKind, ResourceLimits, WorkspaceRelativePath},
    source::{ResourceIssue, ResourceKind, ResourceOrigin, ResourceProvenance},
};

const AGENTS_FILE_NAME: &str = "AGENTS.md";
const ISSUE_CODE_OUT_OF_BOUNDS: &str = "agents_symlink_out_of_bounds";

/// 单个已发现的 `AGENTS.md` 文档。
///
/// 只暴露相对路径与正文：[`ResourceProvenance`] / [`ResourceOrigin`] 仅携带
/// `root_index` 与相对路径，调用方无法据此重建宿主绝对路径。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentsDocument {
    pub provenance: ResourceProvenance,
    pub body: String,
}

impl AgentsDocument {
    /// 该文档相对于工作区根的稳定相对路径（正斜杠分隔）。
    pub fn relative_path(&self) -> &str {
        match &self.provenance.origin {
            ResourceOrigin::Workspace { relative_path, .. } => relative_path,
            _ => "",
        }
    }
}

/// 按路径层级排序的 `AGENTS.md` 聚合。
///
/// [`AgentsHierarchy::documents`] 返回根 → 当前路径的顺序；最后一个元素最靠近
/// 当前路径、优先级最高，可通过 [`AgentsHierarchy::nearest`] 直接获取。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AgentsHierarchy {
    documents: Vec<AgentsDocument>,
}

impl AgentsHierarchy {
    pub fn from_documents(documents: Vec<AgentsDocument>) -> Self {
        Self { documents }
    }

    pub fn documents(&self) -> &[AgentsDocument] {
        &self.documents
    }

    /// 层级中的文档数（`bundle.agents.len()` 在 loader 中使用）。`is_empty` 因零调用
    /// 已删除；clippy `len_without_is_empty` 在此显式允许。
    #[allow(clippy::len_without_is_empty)]
    pub fn len(&self) -> usize {
        self.documents.len()
    }

    /// 优先级最高的文档（最靠近当前路径）；为空时返回 `None`。
    pub fn nearest(&self) -> Option<&AgentsDocument> {
        self.documents.last()
    }
}

/// 从工作区根到当前路径所在目录，发现并聚合 `AGENTS.md` 层级。
///
/// - 目录类型以当前路径自身为终点；文件类型以其所在目录为终点。
/// - `root_index` 用于标注来源 origin，不参与路径解析。
///
/// 返回排序后的层级与逐文件问题列表。问题消息只引用相对路径，不泄漏绝对路径。
pub(crate) fn load_agents_hierarchy(
    root: &Path,
    root_index: usize,
    current_path: &WorkspaceRelativePath,
    current_path_kind: CurrentPathKind,
    limits: ResourceLimits,
) -> (AgentsHierarchy, Vec<ResourceIssue>) {
    let canonical_root = policy_engine::canonicalize_platform(root).ok();
    let chain = target_directory_chain(current_path, current_path_kind);

    let mut found: Vec<(usize, AgentsDocument)> = Vec::new();
    let mut issues = Vec::new();

    for (depth, directory) in chain.iter().enumerate() {
        let relative = directory.join(AGENTS_FILE_NAME);
        let absolute = root.join(&relative);
        match load_one(
            &absolute,
            canonical_root.as_deref(),
            root_index,
            depth,
            &relative,
            limits.max_file_bytes,
        ) {
            Ok(Some(document)) => found.push((depth, document)),
            Ok(None) => {}
            Err(issue) => issues.push(issue),
        }
    }

    found.sort_by(|(depth_a, document_a), (depth_b, document_b)| {
        depth_a
            .cmp(depth_b)
            .then_with(|| document_a.relative_path().cmp(document_b.relative_path()))
    });
    let documents = found.into_iter().map(|(_, document)| document).collect();

    (AgentsHierarchy { documents }, issues)
}

/// 从工作区根到目标目录的目录链（含根），按深度升序排列。
fn target_directory_chain(
    current_path: &WorkspaceRelativePath,
    kind: CurrentPathKind,
) -> Vec<PathBuf> {
    let components: Vec<_> = current_path.as_path().iter().collect();
    let directory_component_count = match kind {
        CurrentPathKind::Directory => components.len(),
        CurrentPathKind::File => components.len().saturating_sub(1),
    };

    let mut chain = Vec::with_capacity(directory_component_count + 1);
    for end in 0..=directory_component_count {
        let mut directory = PathBuf::new();
        for component in components[..end].iter().copied() {
            directory.push(component);
        }
        chain.push(directory);
    }
    chain
}

fn load_one(
    absolute: &Path,
    canonical_root: Option<&Path>,
    root_index: usize,
    depth: usize,
    relative: &Path,
    max_file_bytes: u64,
) -> Result<Option<AgentsDocument>, ResourceIssue> {
    // 通过 canonicalize 同时完成「存在性」与「越界 symlink」校验：缺失文件返回
    // NotFound（静默跳过该层级），解析后的目标若逃出 canonical 根则视为越界。
    let canonical_target = match policy_engine::canonicalize_platform(absolute) {
        Ok(path) => path,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(file_issue(relative, ResourceFileError::Io(error))),
    };

    if let Some(root) = canonical_root {
        if !policy_engine::path_within_root(&canonical_target, root) {
            return Err(out_of_bounds_issue(relative));
        }
    }

    let body = match io::read_utf8_bounded(&canonical_target, max_file_bytes) {
        Ok(body) => body,
        Err(ResourceFileError::NotFound) => return Ok(None),
        Err(error) => return Err(file_issue(relative, error)),
    };

    let relative_key = io::path_key(relative);
    let source_key = format!("workspace:{root_index:08}:agents:{depth:08}:{relative_key}");
    Ok(Some(AgentsDocument {
        provenance: ResourceProvenance::new(
            ConfigTier::Workspace,
            source_key,
            ResourceOrigin::Workspace {
                root_index,
                relative_path: relative_key,
            },
        ),
        body,
    }))
}

fn file_issue(relative: &Path, error: ResourceFileError) -> ResourceIssue {
    let relative_key = io::path_key(relative);
    ResourceIssue::error(error.code(), file_issue_message(&relative_key, &error)).for_resource(
        ResourceKind::AgentsFile,
        relative_key.clone(),
        relative_key,
    )
}

fn file_issue_message(relative_key: &str, error: &ResourceFileError) -> String {
    match error {
        ResourceFileError::NotFound => format!("AGENTS.md at '{relative_key}' was not found"),
        ResourceFileError::TooLarge { limit, actual } => {
            format!("AGENTS.md at '{relative_key}' exceeds the {limit}-byte limit ({actual} bytes)")
        }
        ResourceFileError::InvalidUtf8 => {
            format!("AGENTS.md at '{relative_key}' is not valid UTF-8")
        }
        ResourceFileError::NotRegularFile => {
            format!("AGENTS.md at '{relative_key}' is not a regular file")
        }
        ResourceFileError::OutsideRoot => {
            format!("AGENTS.md at '{relative_key}' resolves outside the workspace root")
        }
        ResourceFileError::Io(_) => format!("AGENTS.md at '{relative_key}' could not be read"),
    }
}

fn out_of_bounds_issue(relative: &Path) -> ResourceIssue {
    let relative_key = io::path_key(relative);
    ResourceIssue::error(
        ISSUE_CODE_OUT_OF_BOUNDS,
        format!("AGENTS.md at '{relative_key}' is a symlink that escapes the workspace root"),
    )
    .for_resource(ResourceKind::AgentsFile, relative_key.clone(), relative_key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write_agents(root: &Path, relative: &str, body: &[u8]) {
        let path = root.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create dirs");
        }
        fs::write(path, body).expect("write agents file");
    }

    fn relative_paths(hierarchy: &AgentsHierarchy) -> Vec<String> {
        hierarchy
            .documents()
            .iter()
            .map(|document| document.relative_path().to_string())
            .collect()
    }

    #[test]
    fn discovers_root_agents_md_for_file_path() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path();
        write_agents(root, "AGENTS.md", b"# root instructions");

        let current = WorkspaceRelativePath::new("src/lib.rs").expect("relative path");
        let (hierarchy, issues) = load_agents_hierarchy(
            root,
            0,
            &current,
            CurrentPathKind::File,
            ResourceLimits::default(),
        );

        assert!(issues.is_empty(), "no issues expected: {issues:?}");
        let documents = hierarchy.documents();
        assert_eq!(documents.len(), 1);
        assert_eq!(documents[0].body, "# root instructions");
        assert_eq!(documents[0].relative_path(), "AGENTS.md");
        assert_eq!(documents[0].provenance.tier, ConfigTier::Workspace);
        assert_eq!(
            documents[0].provenance.origin,
            ResourceOrigin::Workspace {
                root_index: 0,
                relative_path: "AGENTS.md".into(),
            }
        );
        assert_eq!(hierarchy.nearest().unwrap().relative_path(), "AGENTS.md");
    }

    #[test]
    fn orders_from_root_to_nearest_for_multi_level_paths() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path();
        write_agents(root, "AGENTS.md", b"root");
        write_agents(root, "src/AGENTS.md", b"src");
        write_agents(root, "src/deep/AGENTS.md", b"deep");

        let current = WorkspaceRelativePath::new("src/deep/mod.rs").expect("relative path");
        let (hierarchy, issues) = load_agents_hierarchy(
            root,
            1,
            &current,
            CurrentPathKind::File,
            ResourceLimits::default(),
        );

        assert!(issues.is_empty(), "no issues expected: {issues:?}");
        assert_eq!(
            relative_paths(&hierarchy),
            vec![
                "AGENTS.md".to_string(),
                "src/AGENTS.md".to_string(),
                "src/deep/AGENTS.md".to_string(),
            ]
        );
        assert_eq!(hierarchy.nearest().unwrap().body, "deep");
        assert_eq!(
            hierarchy.nearest().unwrap().provenance.origin,
            ResourceOrigin::Workspace {
                root_index: 1,
                relative_path: "src/deep/AGENTS.md".into(),
            }
        );
    }

    #[test]
    fn hierarchy_is_independent_of_write_order() {
        let first = tempfile::tempdir().expect("tempdir");
        let second = tempfile::tempdir().expect("tempdir");

        // 第一个工作区按 root → mid → leaf 顺序写入。
        write_agents(first.path(), "AGENTS.md", b"x");
        write_agents(first.path(), "a/AGENTS.md", b"x");
        write_agents(first.path(), "a/b/AGENTS.md", b"x");
        // 第二个工作区按 leaf → mid → root 顺序写入。
        write_agents(second.path(), "a/b/AGENTS.md", b"x");
        write_agents(second.path(), "a/AGENTS.md", b"x");
        write_agents(second.path(), "AGENTS.md", b"x");

        let current = WorkspaceRelativePath::new("a/b/mod.rs").expect("relative path");
        let (first_hierarchy, first_issues) = load_agents_hierarchy(
            first.path(),
            0,
            &current,
            CurrentPathKind::File,
            ResourceLimits::default(),
        );
        let (second_hierarchy, second_issues) = load_agents_hierarchy(
            second.path(),
            0,
            &current,
            CurrentPathKind::File,
            ResourceLimits::default(),
        );

        assert!(first_issues.is_empty());
        assert!(second_issues.is_empty());
        assert_eq!(
            relative_paths(&first_hierarchy),
            relative_paths(&second_hierarchy)
        );
    }

    #[test]
    fn directory_target_includes_current_directory_level() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path();
        write_agents(root, "AGENTS.md", b"root");
        write_agents(root, "pkg/AGENTS.md", b"pkg");

        let current = WorkspaceRelativePath::new("pkg").expect("relative path");
        let (hierarchy, issues) = load_agents_hierarchy(
            root,
            2,
            &current,
            CurrentPathKind::Directory,
            ResourceLimits::default(),
        );

        assert!(issues.is_empty(), "no issues expected: {issues:?}");
        assert_eq!(
            relative_paths(&hierarchy),
            vec!["AGENTS.md".to_string(), "pkg/AGENTS.md".to_string()]
        );
        assert_eq!(
            hierarchy.documents()[0].provenance.origin,
            ResourceOrigin::Workspace {
                root_index: 2,
                relative_path: "AGENTS.md".into(),
            }
        );
    }

    #[test]
    fn invalid_utf8_is_isolated_and_other_levels_continue() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path();
        write_agents(root, "AGENTS.md", b"# root");
        write_agents(root, "src/AGENTS.md", &[0xff, 0xfe]);

        let current = WorkspaceRelativePath::new("src/lib.rs").expect("relative path");
        let (hierarchy, issues) = load_agents_hierarchy(
            root,
            0,
            &current,
            CurrentPathKind::File,
            ResourceLimits::default(),
        );

        assert_eq!(relative_paths(&hierarchy), vec!["AGENTS.md".to_string()]);
        assert_eq!(issues.len(), 1);
        let issue = &issues[0];
        assert_eq!(issue.code, "resource_invalid_utf8");
        assert_eq!(issue.kind, Some(ResourceKind::AgentsFile));
        assert_eq!(issue.resource_id.as_deref(), Some("src/AGENTS.md"));
        assert_eq!(issue.source_key.as_deref(), Some("src/AGENTS.md"));

        let absolute = root.to_string_lossy().into_owned();
        assert!(!issue.message.contains(&absolute));
        assert!(hierarchy
            .documents()
            .iter()
            .all(|document| !document.relative_path().contains(&absolute)));
    }

    #[test]
    fn oversized_file_is_isolated_as_too_large() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path();
        write_agents(root, "AGENTS.md", b"root");
        write_agents(root, "big/AGENTS.md", &[b'.'; 100]);

        let current = WorkspaceRelativePath::new("big/file.rs").expect("relative path");
        let limits = ResourceLimits {
            max_file_bytes: 16,
            ..ResourceLimits::default()
        };
        let (hierarchy, issues) =
            load_agents_hierarchy(root, 0, &current, CurrentPathKind::File, limits);

        assert_eq!(relative_paths(&hierarchy), vec!["AGENTS.md".to_string()]);
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].code, "resource_too_large");
        assert_eq!(issues[0].kind, Some(ResourceKind::AgentsFile));
    }

    #[cfg(unix)]
    #[test]
    fn out_of_bounds_symlink_is_isolated_and_root_continues() {
        use std::os::unix::fs::symlink;

        let workspace = tempfile::tempdir().expect("tempdir");
        let outside = tempfile::tempdir().expect("tempdir");
        let root = workspace.path();

        write_agents(root, "AGENTS.md", b"# root");
        fs::create_dir_all(root.join("src")).expect("create src");
        fs::write(outside.path().join("AGENTS.md"), b"secret from outside").expect("write outside");
        symlink(
            outside.path().join("AGENTS.md"),
            root.join("src").join("AGENTS.md"),
        )
        .expect("create symlink");

        let current = WorkspaceRelativePath::new("src/lib.rs").expect("relative path");
        let (hierarchy, issues) = load_agents_hierarchy(
            root,
            0,
            &current,
            CurrentPathKind::File,
            ResourceLimits::default(),
        );

        assert_eq!(relative_paths(&hierarchy), vec!["AGENTS.md".to_string()]);
        // 越界 symlink 绝不能被读入正文。
        assert!(hierarchy
            .documents()
            .iter()
            .all(|document| !document.body.contains("secret from outside")));
        assert_eq!(issues.len(), 1);
        let issue = &issues[0];
        assert_eq!(issue.code, "agents_symlink_out_of_bounds");
        assert_eq!(issue.kind, Some(ResourceKind::AgentsFile));
        assert_eq!(issue.resource_id.as_deref(), Some("src/AGENTS.md"));

        let absolute = root.to_string_lossy().into_owned();
        assert!(!issue.message.contains(&absolute));
    }

    #[test]
    fn missing_levels_are_silently_skipped() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path();
        write_agents(root, "src/AGENTS.md", b"only mid");

        let current = WorkspaceRelativePath::new("src/lib.rs").expect("relative path");
        let (hierarchy, issues) = load_agents_hierarchy(
            root,
            0,
            &current,
            CurrentPathKind::File,
            ResourceLimits::default(),
        );

        assert!(issues.is_empty());
        assert_eq!(
            relative_paths(&hierarchy),
            vec!["src/AGENTS.md".to_string()]
        );
    }
}
